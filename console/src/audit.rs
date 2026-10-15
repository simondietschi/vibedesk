//! Protokoll: Verbindungen, Dateiübertragungen und Alarme, die die Clients
//! melden (`/api/audit/...`), dazu die Aktionen in der Konsole.
//!
//! Das gesteuerte Gerät meldet pro Verbindung drei Ereignisse: Aufbau
//! (`action: new`), Anmeldung der Gegenseite (`peer`, `type`) und Ende
//! (`action: close`). Sie werden über Geräte-ID und `conn_id` zu einem
//! Eintrag zusammengeführt.

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::SqlitePool;

use crate::{
    account::ApiUser,
    auth::Admin,
    client_api,
    error::{ApiError, ApiResult},
    now, AppState,
};

const MAX_BODY: usize = 64 * 1024;
/// So lange merkt sich die Konsole eine `nonce`, um wiederholte Meldungen zu
/// erkennen. Der Client wiederholt höchstens 120 Sekunden lang.
const NONCE_SECS: i64 = 10 * 60;
/// Ein Verbindungsaufbau ohne Ende gilt nach dieser Zeit als abgeschlossen,
/// eine spätere Meldung mit derselben `conn_id` beginnt dann einen neuen Eintrag.
const OPEN_CONN_SECS: i64 = 7 * 24 * 3600;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/audit/conn", post(conn))
        .route("/api/audit/file", post(file))
        .route("/api/audit/alarm", post(alarm))
        .route("/api/audit", put(note_by_guid))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

/// Trägt eine Aktion in der Konsole ins Protokoll ein. Fehler werden nur
/// geloggt, damit die eigentliche Aktion nicht daran scheitert.
pub async fn admin_event(db: &SqlitePool, username: &str, action: &str, detail: &str, ip: &str) {
    let result = sqlx::query(
        "insert into admin_log (username, action, detail, ip, created_at) values (?, ?, ?, ?, ?)",
    )
    .bind(username)
    .bind(action)
    .bind(detail)
    .bind(ip)
    .bind(now())
    .execute(db)
    .await;
    if let Err(e) = result {
        tracing::error!("Protokolleintrag fehlgeschlagen: {e}");
    }
    tracing::info!("{username}: {action} {detail}");
}

/// Löscht Protokolleinträge, die älter als `days` Tage sind.
pub async fn prune(db: &SqlitePool, days: i64) -> anyhow::Result<()> {
    let cutoff = now() - days * 24 * 3600;
    for table in ["conn_log", "file_log", "alarm_log", "admin_log"] {
        let column = if table == "conn_log" {
            "started_at"
        } else {
            "created_at"
        };
        sqlx::query(&format!("delete from {table} where {column} < ?"))
            .bind(cutoff)
            .execute(db)
            .await?;
    }
    sqlx::query("delete from audit_nonces where created_at < ?")
        .bind(now() - NONCE_SECS)
        .execute(db)
        .await?;
    Ok(())
}

fn str_field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Prüft Absender und `nonce`. `None` heisst: ignorieren, aber mit 200
/// antworten, damit der Client nicht wiederholt.
async fn accept(state: &AppState, v: &Value) -> ApiResult<Option<String>> {
    let id = str_field(v, "id");
    let uuid = str_field(v, "uuid");
    if id.is_empty() || id.len() > 100 || uuid.is_empty() || uuid.len() > 200 {
        return Ok(None);
    }
    if !client_api::authorized(state, id, uuid).await? {
        return Ok(None);
    }
    let nonce = str_field(v, "nonce");
    if !nonce.is_empty() {
        let inserted = sqlx::query(
            "insert into audit_nonces (nonce, created_at) values (?, ?) on conflict do nothing",
        )
        .bind(clip(nonce, 100))
        .bind(now())
        .execute(&state.db)
        .await?
        .rows_affected();
        if inserted == 0 {
            return Ok(None);
        }
    }
    Ok(Some(id.to_owned()))
}

fn parse(body: &Bytes) -> Option<Value> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .filter(Value::is_object)
}

async fn conn(State(state): State<AppState>, body: Bytes) -> ApiResult<StatusCode> {
    let Some(v) = parse(&body) else {
        return Ok(StatusCode::BAD_REQUEST);
    };
    // Notiz des steuernden Geräts am Ende einer Sitzung: ohne UUID, darum nur
    // an eine bestehende Sitzung gehängt.
    if let Some(note) = v.get("note").and_then(Value::as_str) {
        let session = v
            .get("session_id")
            .map(Value::to_string)
            .unwrap_or_default();
        sqlx::query(
            "update conn_log set note = ? where id = (
                select id from conn_log where device_id = ? and session_id = ?
                order by started_at desc limit 1)",
        )
        .bind(clip(note, 1000))
        .bind(str_field(&v, "id"))
        .bind(session)
        .execute(&state.db)
        .await?;
        return Ok(StatusCode::OK);
    }
    let Some(device_id) = accept(&state, &v).await? else {
        return Ok(StatusCode::OK);
    };
    let conn_id = v.get("conn_id").and_then(Value::as_i64).unwrap_or_default();
    let session_id = v
        .get("session_id")
        .map(Value::to_string)
        .unwrap_or_default();
    let now = now();
    let open: Option<i64> = sqlx::query_scalar(
        "select id from conn_log where device_id = ? and conn_id = ? and ended_at is null
         and started_at > ? order by started_at desc limit 1",
    )
    .bind(&device_id)
    .bind(conn_id)
    .bind(now - OPEN_CONN_SECS)
    .fetch_optional(&state.db)
    .await?;
    let action = str_field(&v, "action");
    let row_id = match (action, open) {
        ("new", _) | (_, None) => {
            let id = sqlx::query(
                "insert into conn_log (device_id, conn_id, session_id, started_at) values (?, ?, ?, ?)",
            )
            .bind(&device_id)
            .bind(conn_id)
            .bind(&session_id)
            .bind(now)
            .execute(&state.db)
            .await?
            .last_insert_rowid();
            if action == "new" {
                sqlx::query("update conn_log set ip = ? where id = ?")
                    .bind(clip(str_field(&v, "ip"), 100))
                    .bind(id)
                    .execute(&state.db)
                    .await?;
            }
            id
        }
        (_, Some(id)) => id,
    };
    if action == "close" {
        sqlx::query("update conn_log set ended_at = ? where id = ?")
            .bind(now)
            .bind(row_id)
            .execute(&state.db)
            .await?;
    }
    if let Some(peer) = v.get("peer").and_then(Value::as_array) {
        let peer_id = peer.first().and_then(Value::as_str).unwrap_or_default();
        let peer_name = peer.get(1).and_then(Value::as_str).unwrap_or_default();
        sqlx::query(
            "update conn_log set peer_id = ?, peer_name = ?, conn_type = ?, primary_auth = ?,
             two_factor = ?, authed_at = ?, session_id = ? where id = ?",
        )
        .bind(clip(peer_id, 100))
        .bind(clip(peer_name, 200))
        .bind(v.get("type").and_then(Value::as_i64))
        .bind(v.get("primary_auth").and_then(Value::as_i64))
        .bind(v.get("two_factor").and_then(Value::as_i64))
        .bind(now)
        .bind(&session_id)
        .bind(row_id)
        .execute(&state.db)
        .await?;
    }
    Ok(StatusCode::OK)
}

async fn file(State(state): State<AppState>, body: Bytes) -> ApiResult<StatusCode> {
    let Some(v) = parse(&body) else {
        return Ok(StatusCode::BAD_REQUEST);
    };
    let Some(device_id) = accept(&state, &v).await? else {
        return Ok(StatusCode::OK);
    };
    // `info` ist ein JSON-Text mit IP, Name der Gegenseite und den grössten Dateien.
    let info: Value = serde_json::from_str(str_field(&v, "info")).unwrap_or_default();
    sqlx::query(
        "insert into file_log (device_id, peer_id, peer_name, ip, direction, path, is_file, num, files, created_at)
         values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&device_id)
    .bind(clip(str_field(&v, "peer_id"), 100))
    .bind(clip(str_field(&info, "name"), 200))
    .bind(clip(str_field(&info, "ip"), 100))
    .bind(v.get("type").and_then(Value::as_i64).unwrap_or_default())
    .bind(clip(str_field(&v, "path"), 2000))
    .bind(v.get("is_file").and_then(Value::as_bool).unwrap_or_default())
    .bind(info.get("num").and_then(Value::as_i64).unwrap_or_default())
    .bind(clip(&info.get("files").cloned().unwrap_or(json!([])).to_string(), 8000))
    .bind(now())
    .execute(&state.db)
    .await?;
    Ok(StatusCode::OK)
}

async fn alarm(State(state): State<AppState>, body: Bytes) -> ApiResult<StatusCode> {
    let Some(v) = parse(&body) else {
        return Ok(StatusCode::BAD_REQUEST);
    };
    let Some(device_id) = accept(&state, &v).await? else {
        return Ok(StatusCode::OK);
    };
    let info_text = str_field(&v, "info");
    let info: Value = serde_json::from_str(info_text).unwrap_or_default();
    sqlx::query(
        "insert into alarm_log (device_id, typ, peer_id, peer_name, ip, info, created_at)
         values (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&device_id)
    .bind(v.get("typ").and_then(Value::as_i64).unwrap_or(-1))
    .bind(clip(str_field(&info, "id"), 100))
    .bind(clip(str_field(&info, "name"), 200))
    .bind(clip(str_field(&info, "ip"), 100))
    .bind(clip(info_text, 4000))
    .bind(now())
    .execute(&state.db)
    .await?;
    Ok(StatusCode::OK)
}

/// Der Client kann eine Notiz über eine GUID nachreichen. Diese GUID vergibt
/// nur ein Server, der sie dem Client zuvor mitgeteilt hat, was VibeDesk
/// nicht tut. Die Anfrage wird darum angenommen und ignoriert.
async fn note_by_guid(_: ApiUser) -> StatusCode {
    StatusCode::OK
}

// ------------------------------------------------------------------ Konsole

#[derive(Deserialize)]
pub struct LogQuery {
    #[serde(default)]
    q: String,
    before: Option<i64>,
    limit: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ConnRow {
    id: i64,
    device_id: String,
    hostname: Option<String>,
    ip: String,
    peer_id: String,
    peer_name: String,
    conn_type: Option<i64>,
    primary_auth: Option<i64>,
    two_factor: Option<i64>,
    note: String,
    started_at: i64,
    authed_at: Option<i64>,
    ended_at: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct FileRow {
    id: i64,
    device_id: String,
    hostname: Option<String>,
    peer_id: String,
    peer_name: String,
    ip: String,
    direction: i64,
    path: String,
    is_file: bool,
    num: i64,
    files: String,
    created_at: i64,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct AlarmRow {
    id: i64,
    device_id: String,
    hostname: Option<String>,
    typ: i64,
    peer_id: String,
    peer_name: String,
    ip: String,
    info: String,
    created_at: i64,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct AdminRow {
    id: i64,
    username: String,
    action: String,
    detail: String,
    ip: String,
    created_at: i64,
}

fn like(q: &str) -> String {
    let escaped = q
        .trim()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

async fn rows<T>(state: &AppState, kind: &str, query: &LogQuery, max: i64) -> ApiResult<Vec<T>>
where
    T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin,
{
    let sql = match kind {
        "verbindungen" => {
            "select l.id, l.device_id, d.hostname, l.ip, l.peer_id, l.peer_name, l.conn_type,
                    l.primary_auth, l.two_factor, l.note, l.started_at, l.authed_at, l.ended_at
             from conn_log l left join devices d on d.id = l.device_id
             where l.id < ?1 and (?2 = '%%' or l.device_id like ?2 escape '\\' or d.hostname like ?2 escape '\\'
                   or l.peer_id like ?2 escape '\\' or l.peer_name like ?2 escape '\\'
                   or l.ip like ?2 escape '\\' or l.note like ?2 escape '\\')
             order by l.id desc limit ?3"
        }
        "dateien" => {
            "select l.id, l.device_id, d.hostname, l.peer_id, l.peer_name, l.ip, l.direction, l.path,
                    l.is_file, l.num, l.files, l.created_at
             from file_log l left join devices d on d.id = l.device_id
             where l.id < ?1 and (?2 = '%%' or l.device_id like ?2 escape '\\' or d.hostname like ?2 escape '\\'
                   or l.peer_id like ?2 escape '\\' or l.peer_name like ?2 escape '\\'
                   or l.path like ?2 escape '\\' or l.files like ?2 escape '\\')
             order by l.id desc limit ?3"
        }
        "alarme" => {
            "select l.id, l.device_id, d.hostname, l.typ, l.peer_id, l.peer_name, l.ip, l.info, l.created_at
             from alarm_log l left join devices d on d.id = l.device_id
             where l.id < ?1 and (?2 = '%%' or l.device_id like ?2 escape '\\' or d.hostname like ?2 escape '\\'
                   or l.peer_id like ?2 escape '\\' or l.ip like ?2 escape '\\')
             order by l.id desc limit ?3"
        }
        "konsole" => {
            "select id, username, action, detail, ip, created_at from admin_log
             where id < ?1 and (?2 = '%%' or username like ?2 escape '\\' or action like ?2 escape '\\'
                   or detail like ?2 escape '\\' or ip like ?2 escape '\\')
             order by id desc limit ?3"
        }
        _ => return Err(ApiError::not_found()),
    };
    Ok(sqlx::query_as::<_, T>(sql)
        .bind(query.before.unwrap_or(i64::MAX))
        .bind(like(&query.q))
        .bind(query.limit.unwrap_or(100).clamp(1, max))
        .fetch_all(&state.db)
        .await?)
}

pub async fn list(
    State(state): State<AppState>,
    _: Admin,
    Path(kind): Path<String>,
    Query(query): Query<LogQuery>,
) -> ApiResult<Json<Value>> {
    let value = match kind.as_str() {
        "verbindungen" => json!(rows::<ConnRow>(&state, &kind, &query, 500).await?),
        "dateien" => json!(rows::<FileRow>(&state, &kind, &query, 500).await?),
        "alarme" => json!(rows::<AlarmRow>(&state, &kind, &query, 500).await?),
        "konsole" => json!(rows::<AdminRow>(&state, &kind, &query, 500).await?),
        _ => return Err(ApiError::not_found()),
    };
    Ok(Json(value))
}

/// CSV-Feld nach RFC 4180. Werte, die eine Tabellenkalkulation als Formel
/// lesen würde, bekommen ein vorangestelltes Apostroph.
fn csv_field(v: &Value) -> String {
    let s = match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) && !matches!(v, Value::Number(_)) {
        format!("'{s}")
    } else {
        s
    };
    if s.contains([',', '"', '\n', '\r', ';']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

pub async fn export_csv(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(kind): Path<String>,
    Query(mut query): Query<LogQuery>,
) -> ApiResult<Response> {
    query.limit = Some(100_000);
    query.before = None;
    let value = match kind.as_str() {
        "verbindungen" => json!(rows::<ConnRow>(&state, &kind, &query, 100_000).await?),
        "dateien" => json!(rows::<FileRow>(&state, &kind, &query, 100_000).await?),
        "alarme" => json!(rows::<AlarmRow>(&state, &kind, &query, 100_000).await?),
        "konsole" => json!(rows::<AdminRow>(&state, &kind, &query, 100_000).await?),
        _ => return Err(ApiError::not_found()),
    };
    let items = value.as_array().cloned().unwrap_or_default();
    let mut out = String::new();
    if let Some(Value::Object(first)) = items.first() {
        let keys: Vec<&String> = first.keys().collect();
        out.push_str(
            &keys
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push_str("\r\n");
        for item in &items {
            let line: Vec<String> = keys
                .iter()
                .map(|k| csv_field(item.get(k.as_str()).unwrap_or(&Value::Null)))
                .collect();
            out.push_str(&line.join(","));
            out.push_str("\r\n");
        }
    }
    admin_event(
        &state.db,
        &admin.username,
        "Protokoll exportiert",
        &kind,
        "",
    )
    .await;
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"vibedesk-{kind}.csv\""),
            ),
        ],
        out,
    )
        .into_response())
}
