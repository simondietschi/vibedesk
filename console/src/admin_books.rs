//! Verwaltung der geteilten Adressbücher in der Konsole. Persönliche
//! Adressbücher gehören ihrem Benutzer und erscheinen hier nicht.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    addressbook::{new_guid, RULE_FULL, RULE_READ},
    auth::Admin,
    error::{ApiError, ApiResult},
    now, AppState,
};

#[derive(Serialize)]
pub struct Member {
    user_id: i64,
    username: String,
    rule: i64,
}

#[derive(Serialize)]
pub struct BookView {
    guid: String,
    name: String,
    note: String,
    owner_id: i64,
    owner: String,
    peer_count: i64,
    members: Vec<Member>,
}

async fn book_id(state: &AppState, guid: &str) -> ApiResult<(i64, i64)> {
    sqlx::query_as("select id, owner_id from address_books where guid = ? and personal = 0")
        .bind(guid)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn view(state: &AppState, guid: &str) -> ApiResult<BookView> {
    let (id, name, note, owner_id, owner, peer_count): (i64, String, String, i64, String, i64) =
        sqlx::query_as(
            "select b.id, b.name, b.note, b.owner_id, u.username,
                    (select count(*) from ab_peers p where p.ab_id = b.id)
             from address_books b join users u on u.id = b.owner_id
             where b.guid = ? and b.personal = 0",
        )
        .bind(guid)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let members = sqlx::query_as::<_, (i64, String, i64)>(
        "select m.user_id, u.username, m.rule from ab_members m join users u on u.id = m.user_id
         where m.ab_id = ? order by u.username collate nocase",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|(user_id, username, rule)| Member {
        user_id,
        username,
        rule,
    })
    .collect();
    Ok(BookView {
        guid: guid.to_owned(),
        name,
        note,
        owner_id,
        owner,
        peer_count,
        members,
    })
}

pub async fn list(State(state): State<AppState>, _: Admin) -> ApiResult<Json<Vec<BookView>>> {
    let guids: Vec<String> = sqlx::query_scalar(
        "select guid from address_books where personal = 0 order by name collate nocase",
    )
    .fetch_all(&state.db)
    .await?;
    let mut books = Vec::with_capacity(guids.len());
    for guid in guids {
        books.push(view(&state, &guid).await?);
    }
    Ok(Json(books))
}

fn clean_name(name: &str) -> ApiResult<String> {
    let name: String = name.trim().chars().take(100).collect();
    if name.is_empty() {
        return Err(ApiError::bad_request("Bitte einen Namen angeben"));
    }
    Ok(name)
}

async fn check_user(state: &AppState, user_id: i64) -> ApiResult<()> {
    let found: Option<i64> = sqlx::query_scalar("select 1 from users where id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    found
        .map(|_| ())
        .ok_or_else(|| ApiError::bad_request("Unbekannter Benutzer"))
}

fn name_conflict(e: sqlx::Error) -> ApiError {
    match e {
        sqlx::Error::Database(db) if db.is_unique_violation() => ApiError::new(
            StatusCode::CONFLICT,
            "Ein Adressbuch mit diesem Namen gibt es schon",
        ),
        e => e.into(),
    }
}

#[derive(Deserialize)]
pub struct CreateBook {
    name: String,
    #[serde(default)]
    note: String,
    owner_id: Option<i64>,
}

pub async fn create(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(req): Json<CreateBook>,
) -> ApiResult<(StatusCode, Json<BookView>)> {
    let name = clean_name(&req.name)?;
    let owner_id = req.owner_id.unwrap_or(admin.id);
    check_user(&state, owner_id).await?;
    let guid = new_guid();
    sqlx::query(
        "insert into address_books (guid, name, note, owner_id, personal, created_at)
         values (?, ?, ?, ?, 0, ?)",
    )
    .bind(&guid)
    .bind(&name)
    .bind(req.note.trim().chars().take(1000).collect::<String>())
    .bind(owner_id)
    .bind(now())
    .execute(&state.db)
    .await
    .map_err(name_conflict)?;
    tracing::info!("{} hat das Adressbuch «{}» angelegt", admin.username, name);
    Ok((StatusCode::CREATED, Json(view(&state, &guid).await?)))
}

#[derive(Deserialize)]
pub struct UpdateBook {
    name: Option<String>,
    note: Option<String>,
    owner_id: Option<i64>,
}

pub async fn update(
    State(state): State<AppState>,
    _: Admin,
    Path(guid): Path<String>,
    Json(req): Json<UpdateBook>,
) -> ApiResult<Json<BookView>> {
    let (id, _) = book_id(&state, &guid).await?;
    let mut tx = state.db.begin().await?;
    if let Some(name) = &req.name {
        sqlx::query("update address_books set name = ? where id = ?")
            .bind(clean_name(name)?)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(name_conflict)?;
    }
    if let Some(note) = &req.note {
        sqlx::query("update address_books set note = ? where id = ?")
            .bind(note.trim().chars().take(1000).collect::<String>())
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(owner_id) = req.owner_id {
        check_user(&state, owner_id).await?;
        sqlx::query("update address_books set owner_id = ? where id = ?")
            .bind(owner_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        // Der Besitzer hat ohnehin volle Kontrolle.
        sqlx::query("delete from ab_members where ab_id = ? and user_id = ?")
            .bind(id)
            .bind(owner_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(view(&state, &guid).await?))
}

pub async fn remove(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(guid): Path<String>,
) -> ApiResult<StatusCode> {
    let (id, _) = book_id(&state, &guid).await?;
    sqlx::query("delete from address_books where id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    tracing::info!("{} hat ein Adressbuch gelöscht ({guid})", admin.username);
    Ok(StatusCode::NO_CONTENT)
}

/// Einträge ohne Passwörter und Hashes.
pub async fn peers(
    State(state): State<AppState>,
    _: Admin,
    Path(guid): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    let (id, _) = book_id(&state, &guid).await?;
    let rows: Vec<String> =
        sqlx::query_scalar("select data from ab_peers where ab_id = ? order by peer_id")
            .bind(id)
            .fetch_all(&state.db)
            .await?;
    Ok(Json(
        rows.iter()
            .filter_map(|d| serde_json::from_str::<Map<String, Value>>(d).ok())
            .map(|mut p| {
                p.remove("hash");
                p.remove("password");
                Value::Object(p)
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct SetMember {
    rule: i64,
}

pub async fn set_member(
    State(state): State<AppState>,
    _: Admin,
    Path((guid, user_id)): Path<(String, i64)>,
    Json(req): Json<SetMember>,
) -> ApiResult<Json<BookView>> {
    if !(RULE_READ..=RULE_FULL).contains(&req.rule) {
        return Err(ApiError::bad_request("Unbekannte Berechtigung"));
    }
    let (id, owner_id) = book_id(&state, &guid).await?;
    check_user(&state, user_id).await?;
    if user_id == owner_id {
        return Err(ApiError::bad_request(
            "Der Besitzer hat schon volle Kontrolle",
        ));
    }
    sqlx::query(
        "insert into ab_members (ab_id, user_id, rule) values (?, ?, ?)
         on conflict(ab_id, user_id) do update set rule = excluded.rule",
    )
    .bind(id)
    .bind(user_id)
    .bind(req.rule)
    .execute(&state.db)
    .await?;
    Ok(Json(view(&state, &guid).await?))
}

pub async fn remove_member(
    State(state): State<AppState>,
    _: Admin,
    Path((guid, user_id)): Path<(String, i64)>,
) -> ApiResult<Json<BookView>> {
    let (id, _) = book_id(&state, &guid).await?;
    sqlx::query("delete from ab_members where ab_id = ? and user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&state.db)
        .await?;
    Ok(Json(view(&state, &guid).await?))
}
