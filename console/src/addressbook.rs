//! Adressbücher für den VibeDesk-Client (`/api/ab/...`).
//!
//! Jeder Benutzer hat ein persönliches Adressbuch. Dazu kommen geteilte
//! Adressbücher, die ein Admin in der Konsole anlegt und Benutzern mit einer
//! Berechtigung zuweist: 1 = lesen, 2 = lesen und schreiben, 3 = volle
//! Kontrolle. Ändernde Endpunkte antworten bei Erfolg mit 200 und leerem
//! Body, so verlangt es der Client.

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{delete, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    account::{page, paged, parse, ApiUser},
    auth::random_token,
    error::{ApiError, ApiResult},
    now, AppState,
};

const MAX_BODY: usize = 256 * 1024;
/// Höchstzahl Einträge pro Adressbuch.
pub const MAX_PEERS: i64 = 5000;
const MAX_TAGS: i64 = 500;

pub const RULE_READ: i64 = 1;
pub const RULE_WRITE: i64 = 2;
pub const RULE_FULL: i64 = 3;

/// Felder eines Eintrags, die gespeichert werden. Alles andere wird verworfen.
const STRING_FIELDS: &[&str] = &[
    "hash",
    "password",
    "username",
    "hostname",
    "platform",
    "alias",
    "note",
    "forceAlwaysRelay",
    "rdpPort",
    "rdpUsername",
    "loginName",
    "device_group_name",
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/ab/settings", post(settings))
        .route("/api/ab/personal", post(personal))
        .route("/api/ab/shared/profiles", post(shared_profiles))
        .route("/api/ab/peers", post(list_peers))
        .route("/api/ab/tags/{guid}", post(list_tags))
        .route("/api/ab/peer/add/{guid}", post(add_peer))
        .route("/api/ab/peer/update/{guid}", put(update_peer))
        .route("/api/ab/peer/{guid}", delete(delete_peers))
        .route("/api/ab/tag/add/{guid}", post(add_tag))
        .route("/api/ab/tag/rename/{guid}", put(rename_tag))
        .route("/api/ab/tag/update/{guid}", put(update_tag))
        .route("/api/ab/tag/{guid}", delete(delete_tags))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

pub fn new_guid() -> String {
    random_token(16)
}

/// Ein Adressbuch, auf das der Benutzer zugreift, mit seiner Berechtigung.
struct Book {
    id: i64,
    personal: bool,
    rule: i64,
}

impl Book {
    fn require_write(&self) -> ApiResult<()> {
        if self.rule >= RULE_WRITE {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }
}

async fn book(state: &AppState, user: &ApiUser, guid: &str) -> ApiResult<Book> {
    let row: Option<(i64, bool, i64, Option<i64>)> = sqlx::query_as(
        "select b.id, b.personal, b.owner_id, m.rule
         from address_books b
         left join ab_members m on m.ab_id = b.id and m.user_id = ?
         where b.guid = ?",
    )
    .bind(user.id)
    .bind(guid)
    .fetch_optional(&state.db)
    .await?;
    let no_access = || ApiError::new(StatusCode::FORBIDDEN, "Kein Zugriff auf dieses Adressbuch");
    let (id, personal, owner_id, rule) = row.ok_or_else(no_access)?;
    let rule = if owner_id == user.id {
        RULE_FULL
    } else if personal {
        return Err(no_access());
    } else {
        rule.ok_or_else(no_access)?
    };
    Ok(Book { id, personal, rule })
}

async fn settings(_: ApiUser) -> Json<Value> {
    // 0 heisst für den Client «keine Grenze». Die Grenze prüft der Server selbst.
    Json(json!({ "max_peer_one_ab": 0 }))
}

/// Liefert das persönliche Adressbuch und legt es beim ersten Aufruf an.
pub async fn personal_guid(state: &AppState, user_id: i64) -> ApiResult<String> {
    let existing: Option<String> =
        sqlx::query_scalar("select guid from address_books where owner_id = ? and personal = 1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
    if let Some(guid) = existing {
        return Ok(guid);
    }
    let guid = new_guid();
    sqlx::query(
        "insert into address_books (guid, name, owner_id, personal, created_at)
         values (?, 'personal', ?, 1, ?)
         on conflict do nothing",
    )
    .bind(&guid)
    .bind(user_id)
    .bind(now())
    .execute(&state.db)
    .await?;
    Ok(
        sqlx::query_scalar("select guid from address_books where owner_id = ? and personal = 1")
            .bind(user_id)
            .fetch_one(&state.db)
            .await?,
    )
}

async fn personal(State(state): State<AppState>, user: ApiUser) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "guid": personal_guid(&state, user.id).await? }),
    ))
}

async fn shared_profiles(
    State(state): State<AppState>,
    user: ApiUser,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let (limit, offset) = page(&query);
    let filter = "b.personal = 0 and (b.owner_id = ?1 or m.rule is not null)";
    let from = "from address_books b
                join users o on o.id = b.owner_id
                left join ab_members m on m.ab_id = b.id and m.user_id = ?1";
    let total: i64 = sqlx::query_scalar(&format!("select count(*) {from} where {filter}"))
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;
    let rows: Vec<(String, String, String, String, i64, Option<i64>)> = sqlx::query_as(&format!(
        "select b.guid, b.name, o.username, b.note, b.owner_id, m.rule {from} where {filter}
         order by b.name collate nocase limit ?2 offset ?3"
    ))
    .bind(user.id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    let data = rows
        .into_iter()
        .map(|(guid, name, owner, note, owner_id, rule)| {
            let rule = if owner_id == user.id {
                RULE_FULL
            } else {
                rule.unwrap_or(RULE_READ)
            };
            json!({ "guid": guid, "name": name, "owner": owner, "note": note, "info": {}, "rule": rule })
        })
        .collect();
    Ok(paged(total, data))
}

async fn list_peers(
    State(state): State<AppState>,
    user: ApiUser,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let guid = query.get("ab").map(String::as_str).unwrap_or_default();
    let book = book(&state, &user, guid).await?;
    let (limit, offset) = page(&query);
    let total: i64 = sqlx::query_scalar("select count(*) from ab_peers where ab_id = ?")
        .bind(book.id)
        .fetch_one(&state.db)
        .await?;
    let rows: Vec<String> = sqlx::query_scalar(
        "select data from ab_peers where ab_id = ? order by peer_id limit ? offset ?",
    )
    .bind(book.id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    let data = rows
        .iter()
        .filter_map(|d| serde_json::from_str(d).ok())
        .collect();
    Ok(paged(total, data))
}

async fn list_tags(
    State(state): State<AppState>,
    user: ApiUser,
    Path(guid): Path<String>,
) -> ApiResult<Json<Value>> {
    let book = book(&state, &user, &guid).await?;
    let rows: Vec<(String, i64)> =
        sqlx::query_as("select name, color from ab_tags where ab_id = ? order by name")
            .bind(book.id)
            .fetch_all(&state.db)
            .await?;
    Ok(Json(Value::Array(
        rows.into_iter()
            .map(|(name, color)| json!({ "name": name, "color": color }))
            .collect(),
    )))
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

async fn existing_tags(state: &AppState, ab_id: i64) -> ApiResult<Vec<String>> {
    Ok(
        sqlx::query_scalar("select name from ab_tags where ab_id = ?")
            .bind(ab_id)
            .fetch_all(&state.db)
            .await?,
    )
}

/// Übernimmt die bekannten Felder aus `input` in `peer`. Ein persönliches
/// Adressbuch speichert den Passwort-Hash, ein geteiltes das Klartext-Passwort,
/// nie beides. Tags, die es im Adressbuch nicht gibt, werden verworfen.
fn merge_peer(
    peer: &mut Map<String, Value>,
    input: &Map<String, Value>,
    personal: bool,
    tags: &[String],
) {
    for &field in STRING_FIELDS {
        if (personal && field == "password") || (!personal && field == "hash") {
            continue;
        }
        if let Some(Value::String(v)) = input.get(field) {
            peer.insert(field.to_owned(), Value::String(clip(v, 1000)));
        }
    }
    if let Some(Value::Bool(b)) = input.get("same_server") {
        peer.insert("same_server".to_owned(), Value::Bool(*b));
    }
    if let Some(Value::Array(list)) = input.get("tags") {
        let filtered: Vec<Value> = list
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| tags.iter().any(|known| known == t))
            .take(100)
            .map(|t| Value::String(t.to_owned()))
            .collect();
        peer.insert("tags".to_owned(), Value::Array(filtered));
    }
}

fn peer_id(input: &Map<String, Value>) -> ApiResult<String> {
    match input.get("id") {
        Some(Value::String(id)) if !id.is_empty() && id.len() <= 100 => Ok(id.clone()),
        _ => Err(ApiError::bad_request("Ungültige ID")),
    }
}

async fn add_peer(
    State(state): State<AppState>,
    user: ApiUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let book = book(&state, &user, &guid).await?;
    book.require_write()?;
    let input: Map<String, Value> = parse(&body)?;
    let id = peer_id(&input)?;
    let count: i64 = sqlx::query_scalar("select count(*) from ab_peers where ab_id = ?")
        .bind(book.id)
        .fetch_one(&state.db)
        .await?;
    if count >= MAX_PEERS {
        return Err(ApiError::bad_request("exceed_max_devices"));
    }
    let tags = existing_tags(&state, book.id).await?;
    let mut peer = Map::new();
    peer.insert("id".to_owned(), Value::String(id.clone()));
    peer.insert("tags".to_owned(), Value::Array(vec![]));
    merge_peer(&mut peer, &input, book.personal, &tags);
    let result = sqlx::query("insert into ab_peers (ab_id, peer_id, data) values (?, ?, ?)")
        .bind(book.id)
        .bind(&id)
        .bind(Value::Object(peer).to_string())
        .execute(&state.db)
        .await;
    match result {
        Ok(_) => Ok(StatusCode::OK),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(ApiError::new(
            StatusCode::CONFLICT,
            "Das Gerät ist schon im Adressbuch",
        )),
        Err(e) => Err(e.into()),
    }
}

async fn update_peer(
    State(state): State<AppState>,
    user: ApiUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let book = book(&state, &user, &guid).await?;
    book.require_write()?;
    let input: Map<String, Value> = parse(&body)?;
    let id = peer_id(&input)?;
    let mut tx = state.db.begin().await?;
    let data: Option<String> =
        sqlx::query_scalar("select data from ab_peers where ab_id = ? and peer_id = ?")
            .bind(book.id)
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?;
    let mut peer: Map<String, Value> = data
        .and_then(|d| serde_json::from_str(&d).ok())
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "Das Gerät ist nicht im Adressbuch"))?;
    let tags: Vec<String> = sqlx::query_scalar("select name from ab_tags where ab_id = ?")
        .bind(book.id)
        .fetch_all(&mut *tx)
        .await?;
    merge_peer(&mut peer, &input, book.personal, &tags);
    sqlx::query("update ab_peers set data = ? where ab_id = ? and peer_id = ?")
        .bind(Value::Object(peer).to_string())
        .bind(book.id)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::OK)
}

async fn delete_peers(
    State(state): State<AppState>,
    user: ApiUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let book = book(&state, &user, &guid).await?;
    book.require_write()?;
    let ids: Vec<String> = parse(&body)?;
    let mut tx = state.db.begin().await?;
    for id in ids.iter().take(MAX_PEERS as usize) {
        sqlx::query("delete from ab_peers where ab_id = ? and peer_id = ?")
            .bind(book.id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct TagInput {
    name: String,
    color: i64,
}

fn valid_tag(name: &str) -> ApiResult<()> {
    if name.trim().is_empty() || name.chars().count() > 100 {
        Err(ApiError::bad_request("Ungültiger Tag"))
    } else {
        Ok(())
    }
}

async fn add_tag(
    State(state): State<AppState>,
    user: ApiUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let book = book(&state, &user, &guid).await?;
    book.require_write()?;
    let tag: TagInput = parse(&body)?;
    valid_tag(&tag.name)?;
    let count: i64 = sqlx::query_scalar("select count(*) from ab_tags where ab_id = ?")
        .bind(book.id)
        .fetch_one(&state.db)
        .await?;
    if count >= MAX_TAGS {
        return Err(ApiError::bad_request("Zu viele Tags"));
    }
    sqlx::query(
        "insert into ab_tags (ab_id, name, color) values (?, ?, ?)
         on conflict(ab_id, name) do update set color = excluded.color",
    )
    .bind(book.id)
    .bind(&tag.name)
    .bind(tag.color)
    .execute(&state.db)
    .await?;
    Ok(StatusCode::OK)
}

async fn update_tag(
    State(state): State<AppState>,
    user: ApiUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let book = book(&state, &user, &guid).await?;
    book.require_write()?;
    let tag: TagInput = parse(&body)?;
    sqlx::query("update ab_tags set color = ? where ab_id = ? and name = ?")
        .bind(tag.color)
        .bind(book.id)
        .bind(&tag.name)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::OK)
}

/// Ersetzt oder entfernt Tags in allen Einträgen eines Adressbuchs.
async fn rewrite_peer_tags(
    tx: &mut sqlx::SqliteConnection,
    ab_id: i64,
    map: impl Fn(&str) -> Option<String>,
) -> ApiResult<()> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("select peer_id, data from ab_peers where ab_id = ?")
            .bind(ab_id)
            .fetch_all(&mut *tx)
            .await?;
    for (peer_id, data) in rows {
        let Ok(mut peer) = serde_json::from_str::<Map<String, Value>>(&data) else {
            continue;
        };
        let Some(Value::Array(tags)) = peer.get("tags") else {
            continue;
        };
        let mut new_tags: Vec<Value> = Vec::new();
        for t in tags.iter().filter_map(Value::as_str) {
            if let Some(n) = map(t) {
                if !new_tags.iter().any(|v| v.as_str() == Some(&n)) {
                    new_tags.push(Value::String(n));
                }
            }
        }
        if new_tags.len() == tags.len() && new_tags.iter().zip(tags).all(|(a, b)| a == b) {
            continue;
        }
        peer.insert("tags".to_owned(), Value::Array(new_tags));
        sqlx::query("update ab_peers set data = ? where ab_id = ? and peer_id = ?")
            .bind(Value::Object(peer).to_string())
            .bind(ab_id)
            .bind(&peer_id)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct RenameTag {
    old: String,
    new: String,
}

async fn rename_tag(
    State(state): State<AppState>,
    user: ApiUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let book = book(&state, &user, &guid).await?;
    book.require_write()?;
    let req: RenameTag = parse(&body)?;
    valid_tag(&req.new)?;
    if req.old == req.new {
        return Ok(StatusCode::OK);
    }
    let mut tx = state.db.begin().await?;
    let taken: Option<i64> =
        sqlx::query_scalar("select 1 from ab_tags where ab_id = ? and name = ?")
            .bind(book.id)
            .bind(&req.new)
            .fetch_optional(&mut *tx)
            .await?;
    if taken.is_some() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "Diesen Tag gibt es schon",
        ));
    }
    let renamed = sqlx::query("update ab_tags set name = ? where ab_id = ? and name = ?")
        .bind(&req.new)
        .bind(book.id)
        .bind(&req.old)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if renamed == 0 {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "Unbekannter Tag"));
    }
    rewrite_peer_tags(&mut tx, book.id, |t| {
        Some(if t == req.old {
            req.new.clone()
        } else {
            t.to_owned()
        })
    })
    .await?;
    tx.commit().await?;
    Ok(StatusCode::OK)
}

async fn delete_tags(
    State(state): State<AppState>,
    user: ApiUser,
    Path(guid): Path<String>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let book = book(&state, &user, &guid).await?;
    book.require_write()?;
    let names: Vec<String> = parse(&body)?;
    let mut tx = state.db.begin().await?;
    for name in &names {
        sqlx::query("delete from ab_tags where ab_id = ? and name = ?")
            .bind(book.id)
            .bind(name)
            .execute(&mut *tx)
            .await?;
    }
    rewrite_peer_tags(&mut tx, book.id, |t| {
        (!names.iter().any(|n| n == t)).then(|| t.to_owned())
    })
    .await?;
    tx.commit().await?;
    Ok(StatusCode::OK)
}
