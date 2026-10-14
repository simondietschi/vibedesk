//! Anmeldung des VibeDesk-Clients (`/api/login`, `/api/currentUser`,
//! `/api/logout`) und die Listen für den Reiter «Geräte» im Client
//! (`/api/users`, `/api/peers`, `/api/device-group/accessible`).
//!
//! Die Antworten folgen dem Format, das der Client erwartet. Fehler sind
//! `{"error": "..."}`, der Client zeigt den Text an.

use std::{collections::HashMap, sync::Mutex};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequestParts, Query, State},
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

use crate::{
    auth::{
        burn_password_check, random_token, token_hash, verify_password, ClientIp, LoginLimiter,
    },
    client_api,
    error::{ApiError, ApiResult},
    now, totp, AppState,
};

const MAX_BODY: usize = 256 * 1024;
/// Client-Anmeldungen ohne Nutzung verfallen nach dieser Zeit.
pub const TOKEN_IDLE_SECS: i64 = 90 * 24 * 3600;
const CHALLENGE_SECS: i64 = 5 * 60;
const CHALLENGE_ATTEMPTS: u32 = 5;
const PAGE_SIZE_MAX: i64 = 500;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/login-options", get(login_options))
        .route("/api/login", post(login))
        .route("/api/currentUser", post(current_user))
        .route("/api/logout", post(logout))
        .route("/api/users", get(users))
        .route("/api/peers", get(peers))
        .route("/api/device-group/accessible", get(device_groups))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

/// JSON aus dem Body lesen. Fehler als `{"error"}`, damit der Client sie anzeigt.
pub(crate) fn parse<T: DeserializeOwned>(body: &Bytes) -> ApiResult<T> {
    serde_json::from_slice(body).map_err(|_| ApiError::bad_request("Ungültige Anfrage"))
}

/// Ein Client, der sich mit seinem Token ausweist.
#[derive(Clone, Debug)]
pub struct ApiUser {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
    pub token_hash: String,
}

impl FromRequestParts<AppState> for ApiUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(ApiError::unauthorized)?;
        let hash = token_hash(token);
        let now = now();
        let row: Option<(i64, String, String, String, i64)> = sqlx::query_as(
            "select u.id, u.username, u.display_name, u.role, t.last_used_at
             from api_tokens t join users u on u.id = t.user_id
             where t.token_hash = ? and t.last_used_at > ? and u.disabled = 0",
        )
        .bind(&hash)
        .bind(now - TOKEN_IDLE_SECS)
        .fetch_optional(&state.db)
        .await?;
        let (id, username, display_name, role, last_used) =
            row.ok_or_else(ApiError::unauthorized)?;
        if now - last_used > 3600 {
            sqlx::query("update api_tokens set last_used_at = ? where token_hash = ?")
                .bind(now)
                .bind(&hash)
                .execute(&state.db)
                .await?;
        }
        Ok(ApiUser {
            id,
            username,
            display_name,
            is_admin: role == "admin",
            token_hash: hash,
        })
    }
}

/// Benutzerdaten im Format, das der Client liest.
fn user_payload(username: &str, display_name: &str, is_admin: bool) -> Value {
    json!({
        "name": username,
        "display_name": display_name,
        "email": "",
        "note": "",
        "status": 1,
        "is_admin": is_admin,
    })
}

/// Offene Zwei-Faktor-Anmeldung zwischen Passwort- und Code-Schritt.
pub struct Challenge {
    user_id: i64,
    username: String,
    device_id: String,
    device_uuid: String,
    expires_at: i64,
    attempts: u32,
}

#[derive(Default)]
pub struct Challenges(Mutex<HashMap<String, Challenge>>);

impl Challenges {
    fn insert(&self, secret: String, challenge: Challenge, now: i64) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, c| c.expires_at > now);
        map.insert(secret, challenge);
    }
}

async fn login_options() -> Json<Value> {
    Json(json!([]))
}

#[derive(Deserialize)]
struct LoginRequest {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    uuid: String,
    #[serde(default, rename = "type")]
    typ: String,
    #[serde(default, rename = "tfaCode")]
    tfa_code: String,
    #[serde(default, rename = "verificationCode")]
    verification_code: String,
    #[serde(default)]
    secret: String,
}

#[derive(sqlx::FromRow)]
struct LoginRow {
    id: i64,
    username: String,
    display_name: String,
    role: String,
    password_hash: String,
    disabled: bool,
    totp_secret: Option<String>,
    totp_last_step: i64,
}

async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    body: Bytes,
) -> ApiResult<Response> {
    let req: LoginRequest = parse(&body)?;
    let now = now();
    let keys = LoginLimiter::keys(&req.username, ip);
    if let Some(secs) = state.limiter.locked(&keys, now) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "Zu viele Fehlversuche. Bitte in {} Minuten erneut versuchen.",
                (secs + 59) / 60
            ),
        ));
    }
    if req.typ == "email_code" {
        return login_second_step(&state, req, &keys, now).await;
    }

    let row: Option<LoginRow> = sqlx::query_as(
        "select id, username, display_name, role, password_hash, disabled, totp_secret, totp_last_step
         from users where username = ?",
    )
    .bind(&req.username)
    .fetch_optional(&state.db)
    .await?;
    let wrong = || {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Benutzername oder Passwort falsch",
        )
    };
    let Some(row) = row else {
        burn_password_check(&req.password);
        state.limiter.fail(&keys, now);
        return Err(wrong());
    };
    if !verify_password(&row.password_hash, &req.password) {
        state.limiter.fail(&keys, now);
        return Err(wrong());
    }
    if row.disabled {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Dieses Konto ist gesperrt",
        ));
    }
    if row.totp_secret.is_some() {
        // Der Client kennt nur «email_check» als Zwischenschritt, «tfa_check»
        // als tfa_type lässt ihn nach dem Code der Authenticator-App fragen.
        let secret = random_token(24);
        state.challenges.insert(
            secret.clone(),
            Challenge {
                user_id: row.id,
                username: row.username.clone(),
                device_id: req.id,
                device_uuid: req.uuid,
                expires_at: now + CHALLENGE_SECS,
                attempts: 0,
            },
            now,
        );
        return Ok(Json(json!({
            "type": "email_check",
            "tfa_type": "tfa_check",
            "secret": secret,
            "user": user_payload(&row.username, &row.display_name, row.role == "admin"),
        }))
        .into_response());
    }
    state.limiter.succeed(&keys);
    issue_token(&state, &row, &req.id, &req.uuid, now).await
}

async fn login_second_step(
    state: &AppState,
    req: LoginRequest,
    keys: &[String],
    now: i64,
) -> ApiResult<Response> {
    let expired = || {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Anmeldung abgelaufen, bitte neu anmelden",
        )
    };
    let (user_id, device_id, device_uuid) = {
        let map = state.challenges.0.lock().unwrap_or_else(|e| e.into_inner());
        let c = map.get(&req.secret).ok_or_else(expired)?;
        if c.expires_at <= now || !c.username.eq_ignore_ascii_case(&req.username) {
            return Err(expired());
        }
        (c.user_id, c.device_id.clone(), c.device_uuid.clone())
    };
    let row: LoginRow = sqlx::query_as(
        "select id, username, display_name, role, password_hash, disabled, totp_secret, totp_last_step
         from users where id = ?",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(expired)?;
    let code = if req.tfa_code.is_empty() {
        &req.verification_code
    } else {
        &req.tfa_code
    };
    let step = row
        .totp_secret
        .as_deref()
        .and_then(|secret| totp::verify(secret, code, now))
        .filter(|&step| step > row.totp_last_step);
    let Some(step) = step else {
        state.limiter.fail(keys, now);
        let mut map = state.challenges.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = map.get_mut(&req.secret) {
            c.attempts += 1;
            if c.attempts >= CHALLENGE_ATTEMPTS {
                map.remove(&req.secret);
            }
        }
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Bestätigungscode ungültig",
        ));
    };
    if row.disabled {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Dieses Konto ist gesperrt",
        ));
    }
    state
        .challenges
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&req.secret);
    sqlx::query("update users set totp_last_step = ? where id = ?")
        .bind(step)
        .bind(row.id)
        .execute(&state.db)
        .await?;
    state.limiter.succeed(keys);
    issue_token(state, &row, &device_id, &device_uuid, now).await
}

async fn issue_token(
    state: &AppState,
    row: &LoginRow,
    device_id: &str,
    device_uuid: &str,
    now: i64,
) -> ApiResult<Response> {
    let token = random_token(32);
    let device_id: String = device_id.chars().take(100).collect();
    let mut tx = state.db.begin().await?;
    sqlx::query("delete from api_tokens where last_used_at <= ?")
        .bind(now - TOKEN_IDLE_SECS)
        .execute(&mut *tx)
        .await?;
    // Eine Anmeldung pro Gerät und Benutzer: eine neue ersetzt die alte.
    if !device_id.is_empty() {
        sqlx::query("delete from api_tokens where user_id = ? and device_id = ?")
            .bind(row.id)
            .bind(&device_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "insert into api_tokens (token_hash, user_id, device_id, created_at, last_used_at)
         values (?, ?, ?, ?, ?)",
    )
    .bind(token_hash(&token))
    .bind(row.id)
    .bind(&device_id)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    sqlx::query("update users set last_login_at = ? where id = ?")
        .bind(now)
        .bind(row.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    bind_device(state, &device_id, device_uuid, Some(row.id)).await?;
    tracing::info!(
        "Client-Anmeldung von {} auf Gerät {}",
        row.username,
        device_id
    );
    Ok(Json(json!({
        "type": "access_token",
        "access_token": token,
        "user": user_payload(&row.username, &row.display_name, row.role == "admin"),
    }))
    .into_response())
}

/// Ordnet ein Gerät dem angemeldeten Konto zu, aber nur wenn ID und UUID
/// zusammenpassen, damit niemand fremde Geräte auf sich buchen kann.
async fn bind_device(
    state: &AppState,
    device_id: &str,
    device_uuid: &str,
    user_id: Option<i64>,
) -> ApiResult<()> {
    if device_id.is_empty() || device_uuid.is_empty() {
        return Ok(());
    }
    if !client_api::authorized(state, device_id, device_uuid).await? {
        return Ok(());
    }
    sqlx::query(
        "insert into devices (id, uuid, user_id) values (?, ?, ?)
         on conflict(id) do update set user_id = excluded.user_id",
    )
    .bind(device_id)
    .bind(device_uuid)
    .bind(user_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn current_user(user: ApiUser) -> Json<Value> {
    Json(user_payload(
        &user.username,
        &user.display_name,
        user.is_admin,
    ))
}

#[derive(Deserialize, Default)]
struct DeviceRef {
    #[serde(default)]
    id: String,
}

async fn logout(
    State(state): State<AppState>,
    user: Result<ApiUser, ApiError>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let Ok(user) = user else {
        return Ok(StatusCode::OK);
    };
    sqlx::query("delete from api_tokens where token_hash = ?")
        .bind(&user.token_hash)
        .execute(&state.db)
        .await?;
    let device: DeviceRef = serde_json::from_slice(&body).unwrap_or_default();
    if !device.id.is_empty() {
        sqlx::query("update devices set user_id = null where id = ? and user_id = ?")
            .bind(&device.id)
            .bind(user.id)
            .execute(&state.db)
            .await?;
    }
    Ok(StatusCode::OK)
}

/// Seite und Seitengrösse aus `current` und `pageSize`.
pub(crate) fn page(query: &HashMap<String, String>) -> (i64, i64) {
    let current = query
        .get("current")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    let size = query
        .get("pageSize")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(100)
        .clamp(1, PAGE_SIZE_MAX);
    (size, (current - 1) * size)
}

pub(crate) fn paged(total: i64, data: Vec<Value>) -> Json<Value> {
    Json(json!({ "total": total, "data": data }))
}

async fn users(
    State(state): State<AppState>,
    user: ApiUser,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let (limit, offset) = page(&query);
    // Admins sehen alle aktiven Konten, alle anderen nur sich selbst.
    let only = (!user.is_admin).then_some(user.id);
    let filter = "disabled = 0 and (?1 is null or id = ?1)";
    let total: i64 = sqlx::query_scalar(&format!("select count(*) from users where {filter}"))
        .bind(only)
        .fetch_one(&state.db)
        .await?;
    let rows: Vec<(String, String, String)> = sqlx::query_as(&format!(
        "select username, display_name, role from users where {filter}
         order by username collate nocase limit ?2 offset ?3"
    ))
    .bind(only)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    let data = rows
        .iter()
        .map(|(name, display, role)| user_payload(name, display, role == "admin"))
        .collect();
    Ok(paged(total, data))
}

async fn peers(
    State(state): State<AppState>,
    user: ApiUser,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let (limit, offset) = page(&query);
    // Admins sehen alle bekannten Geräte, alle anderen nur die eigenen.
    let only = (!user.is_admin).then_some(user.id);
    let filter = "(?1 is null and (d.first_seen_at is not null or d.user_id is not null))
                  or d.user_id = ?1";
    let total: i64 = sqlx::query_scalar(&format!("select count(*) from devices d where {filter}"))
        .bind(only)
        .fetch_one(&state.db)
        .await?;
    let rows: Vec<(String, String, String, String, String, Option<String>)> =
        sqlx::query_as(&format!(
            "select d.id, d.hostname, d.username, d.os, d.note, u.username
             from devices d left join users u on u.id = d.user_id
             where {filter} order by d.id limit ?2 offset ?3"
        ))
        .bind(only)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
    let data = rows
        .into_iter()
        .map(|(id, hostname, os_user, os, note, account)| {
            let account = account.unwrap_or_default();
            json!({
                "id": id,
                "info": { "username": os_user, "os": os, "device_name": hostname },
                "status": 1,
                "user": account,
                "user_name": account,
                "note": note,
                "device_group_name": "",
            })
        })
        .collect();
    Ok(paged(total, data))
}

async fn device_groups(_: ApiUser) -> Json<Value> {
    paged(0, vec![])
}
