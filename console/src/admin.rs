//! API der Weboberfläche unter `/admin/api`.

use std::collections::BTreeMap;

use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    admin_books, audit,
    auth::{
        self, burn_password_check, check_password_policy, hash_password, verify_password, Admin,
        ClientIp, CurrentUser, LoginLimiter,
    },
    error::{ApiError, ApiResult},
    now, totp, AppState, ONLINE_WINDOW_SECS,
};

/// Header, den die Weboberfläche bei jeder ändernden Anfrage mitsendet.
/// Fremde Seiten können ihn ohne CORS-Freigabe nicht setzen.
pub const CSRF_HEADER: &str = "x-vibedesk";
const TOTP_ISSUER: &str = "VibeDesk";

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/status", get(status))
        .route("/me", get(me))
        .route("/me/password", post(change_own_password))
        .route("/me/2fa/setup", post(totp_setup))
        .route("/me/2fa/enable", post(totp_enable))
        .route("/me/2fa/disable", post(totp_disable))
        .route("/users", get(list_users).post(create_user))
        .route("/users/{id}", patch(update_user).delete(delete_user))
        .route("/users/{id}/password", post(reset_password))
        .route("/users/{id}/reset-2fa", post(reset_totp))
        .route("/devices", get(list_devices))
        .route("/devices/{id}", patch(update_device).delete(delete_device))
        .route(
            "/address-books",
            get(admin_books::list).post(admin_books::create),
        )
        .route(
            "/address-books/{guid}",
            patch(admin_books::update).delete(admin_books::remove),
        )
        .route("/address-books/{guid}/peers", get(admin_books::peers))
        .route("/logs/{kind}", get(audit::list))
        .route("/logs/{kind}/csv", get(audit::export_csv))
        .route(
            "/address-books/{guid}/members/{user_id}",
            put(admin_books::set_member).delete(admin_books::remove_member),
        )
        .layer(middleware::from_fn_with_state(state, csrf_guard))
}

async fn csrf_guard(State(_): State<AppState>, req: Request, next: Next) -> Response {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD);
    if !safe && req.headers().get(CSRF_HEADER).is_none() {
        return ApiError::forbidden().into_response();
    }
    let mut res = next.run(req).await;
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn ip_text(ip: Option<std::net::IpAddr>) -> String {
    ip.map(|ip| ip.to_string()).unwrap_or_default()
}

fn validate_username(username: &str) -> ApiResult<()> {
    let ok = (1..=64).contains(&username.len())
        && username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._@-".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "Benutzernamen dürfen nur Buchstaben, Ziffern und . _ @ - enthalten (höchstens 64 Zeichen)",
        ))
    }
}

fn validate_role(role: &str) -> ApiResult<()> {
    if role == "admin" || role == "user" {
        Ok(())
    } else {
        Err(ApiError::bad_request("Unbekannte Rolle"))
    }
}

// ---------------------------------------------------------------- Anmeldung

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
    #[serde(default)]
    code: String,
}

#[derive(sqlx::FromRow)]
struct LoginRow {
    id: i64,
    password_hash: String,
    disabled: bool,
    totp_secret: Option<String>,
    totp_last_step: i64,
}

async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Response> {
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
    let row: Option<LoginRow> = sqlx::query_as(
        "select id, password_hash, disabled, totp_secret, totp_last_step from users where username = ?",
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
        audit::admin_event(
            &state.db,
            &req.username,
            "Anmeldung Konsole fehlgeschlagen",
            "unbekanntes Konto",
            &ip_text(ip),
        )
        .await;
        return Err(wrong());
    };
    if !verify_password(&row.password_hash, &req.password) {
        state.limiter.fail(&keys, now);
        audit::admin_event(
            &state.db,
            &req.username,
            "Anmeldung Konsole fehlgeschlagen",
            "falsches Passwort",
            &ip_text(ip),
        )
        .await;
        return Err(wrong());
    }
    if row.disabled {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Dieses Konto ist gesperrt",
        ));
    }
    if let Some(secret) = &row.totp_secret {
        if req.code.trim().is_empty() {
            return Ok((
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Bestätigungscode erforderlich", "needs_code": true })),
            )
                .into_response());
        }
        match totp::verify(secret, &req.code, now) {
            Some(step) if step > row.totp_last_step => {
                sqlx::query("update users set totp_last_step = ? where id = ?")
                    .bind(step)
                    .bind(row.id)
                    .execute(&state.db)
                    .await?;
            }
            _ => {
                state.limiter.fail(&keys, now);
                return Ok((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "Bestätigungscode ungültig", "needs_code": true })),
                )
                    .into_response());
            }
        }
    }
    state.limiter.succeed(&keys);
    sqlx::query("update users set last_login_at = ? where id = ?")
        .bind(now)
        .bind(row.id)
        .execute(&state.db)
        .await?;
    let token = auth::create_session(&state, row.id).await?;
    audit::admin_event(
        &state.db,
        &req.username,
        "Anmeldung Konsole",
        "",
        &ip_text(ip),
    )
    .await;
    Ok((
        [(
            header::SET_COOKIE,
            auth::session_cookie(&state, &token, auth::SESSION_MAX_SECS),
        )],
        Json(json!({ "ok": true })),
    )
        .into_response())
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(token) = auth::session_token(&headers) {
        auth::delete_session(&state, &token).await?;
    }
    Ok((
        [(header::SET_COOKIE, auth::session_cookie(&state, "", 0))],
        Json(json!({ "ok": true })),
    )
        .into_response())
}

async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "hbbs": state.hbbs.is_some(),
    }))
}

async fn me(user: CurrentUser) -> Json<CurrentUser> {
    Json(user)
}

// ------------------------------------------------------------- Eigenes Konto

#[derive(Deserialize)]
struct ChangePassword {
    current_password: String,
    new_password: String,
}

async fn check_own_password(state: &AppState, user_id: i64, password: &str) -> ApiResult<()> {
    let hash: String = sqlx::query_scalar("select password_hash from users where id = ?")
        .bind(user_id)
        .fetch_one(&state.db)
        .await?;
    if verify_password(&hash, password) {
        Ok(())
    } else {
        Err(ApiError::bad_request("Das aktuelle Passwort ist falsch"))
    }
}

async fn change_own_password(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<ChangePassword>,
) -> ApiResult<Response> {
    check_own_password(&state, user.id, &req.current_password).await?;
    check_password_policy(&req.new_password)?;
    sqlx::query("update users set password_hash = ? where id = ?")
        .bind(hash_password(&req.new_password)?)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    // Alle Sitzungen beenden, auch gestohlene, und die aktuelle neu ausstellen.
    revoke_sessions(&state, user.id).await?;
    audit::admin_event(
        &state.db,
        &user.username,
        "Eigenes Passwort geändert",
        "",
        "",
    )
    .await;
    let token = auth::create_session(&state, user.id).await?;
    Ok((
        [(
            header::SET_COOKIE,
            auth::session_cookie(&state, &token, auth::SESSION_MAX_SECS),
        )],
        Json(json!({ "ok": true })),
    )
        .into_response())
}

#[derive(Deserialize)]
struct PasswordConfirm {
    password: String,
}

#[derive(Serialize)]
struct TotpSetup {
    secret: String,
    otpauth_url: String,
    qr_svg: String,
}

async fn totp_setup(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<PasswordConfirm>,
) -> ApiResult<Json<TotpSetup>> {
    check_own_password(&state, user.id, &req.password).await?;
    let secret = totp::generate_secret();
    sqlx::query("update users set totp_pending = ? where id = ?")
        .bind(&secret)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    let otpauth_url = totp::otpauth_url(&secret, &user.username, TOTP_ISSUER);
    let qr_svg = totp::qr_svg(&otpauth_url)?;
    Ok(Json(TotpSetup {
        secret,
        otpauth_url,
        qr_svg,
    }))
}

#[derive(Deserialize)]
struct TotpCode {
    code: String,
}

async fn totp_enable(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<TotpCode>,
) -> ApiResult<Json<serde_json::Value>> {
    let pending: Option<String> = sqlx::query_scalar("select totp_pending from users where id = ?")
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;
    let pending =
        pending.ok_or_else(|| ApiError::bad_request("Bitte die Einrichtung neu starten"))?;
    let step = totp::verify(&pending, &req.code, now()).ok_or_else(|| {
        ApiError::bad_request("Der Code stimmt nicht. Uhrzeit des Telefons prüfen.")
    })?;
    sqlx::query(
        "update users set totp_secret = totp_pending, totp_pending = null, totp_last_step = ? where id = ?",
    )
    .bind(step)
    .bind(user.id)
    .execute(&state.db)
    .await?;
    audit::admin_event(
        &state.db,
        &user.username,
        "Zwei-Faktor eingeschaltet",
        "",
        "",
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TotpDisable {
    password: String,
    code: String,
}

async fn totp_disable(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<TotpDisable>,
) -> ApiResult<Json<serde_json::Value>> {
    check_own_password(&state, user.id, &req.password).await?;
    let secret: Option<String> = sqlx::query_scalar("select totp_secret from users where id = ?")
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;
    let secret = secret.ok_or_else(|| ApiError::bad_request("Zwei-Faktor ist nicht aktiv"))?;
    if totp::verify(&secret, &req.code, now()).is_none() {
        return Err(ApiError::bad_request("Der Code stimmt nicht"));
    }
    sqlx::query("update users set totp_secret = null, totp_pending = null where id = ?")
        .bind(user.id)
        .execute(&state.db)
        .await?;
    audit::admin_event(
        &state.db,
        &user.username,
        "Zwei-Faktor ausgeschaltet",
        "",
        "",
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

// ------------------------------------------------------------------ Benutzer

#[derive(Serialize, sqlx::FromRow)]
struct UserRow {
    id: i64,
    username: String,
    display_name: String,
    role: String,
    disabled: bool,
    totp_enabled: bool,
    created_at: i64,
    last_login_at: Option<i64>,
}

const USER_COLUMNS: &str = "id, username, display_name, role, disabled,
    totp_secret is not null as totp_enabled, created_at, last_login_at";

async fn fetch_user(state: &AppState, id: i64) -> ApiResult<UserRow> {
    sqlx::query_as(&format!("select {USER_COLUMNS} from users where id = ?"))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn list_users(State(state): State<AppState>, _: Admin) -> ApiResult<Json<Vec<UserRow>>> {
    let users = sqlx::query_as(&format!(
        "select {USER_COLUMNS} from users order by username collate nocase"
    ))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(users))
}

#[derive(Deserialize)]
struct CreateUser {
    username: String,
    #[serde(default)]
    display_name: String,
    role: String,
    #[serde(default)]
    password: Option<String>,
}

/// Legt einen Benutzer an. Ohne Passwort wird eines erzeugt und einmalig zurückgegeben.
pub async fn insert_user(
    db: &sqlx::SqlitePool,
    username: &str,
    display_name: &str,
    role: &str,
    password: &str,
) -> ApiResult<i64> {
    let result = sqlx::query(
        "insert into users (username, display_name, password_hash, role, created_at)
         values (?, ?, ?, ?, ?)",
    )
    .bind(username)
    .bind(display_name)
    .bind(hash_password(password)?)
    .bind(role)
    .bind(now())
    .execute(db)
    .await;
    match result {
        Ok(r) => Ok(r.last_insert_rowid()),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(ApiError::new(
            StatusCode::CONFLICT,
            "Diesen Benutzernamen gibt es schon",
        )),
        Err(e) => Err(e.into()),
    }
}

async fn create_user(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(req): Json<CreateUser>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let username = req.username.trim();
    validate_username(username)?;
    validate_role(&req.role)?;
    let display_name: String = req.display_name.trim().chars().take(100).collect();
    let (password, generated) = match req.password.filter(|p| !p.is_empty()) {
        Some(p) => {
            check_password_policy(&p)?;
            (p, false)
        }
        None => (auth::random_password(), true),
    };
    let id = insert_user(&state.db, username, &display_name, &req.role, &password).await?;
    audit::admin_event(
        &state.db,
        &admin.username,
        "Benutzer angelegt",
        &format!("{username} ({})", req.role),
        "",
    )
    .await;
    let user = fetch_user(&state, id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "user": user,
            "password": if generated { Some(password) } else { None },
        })),
    ))
}

#[derive(Deserialize)]
struct UpdateUser {
    display_name: Option<String>,
    role: Option<String>,
    disabled: Option<bool>,
}

/// Gibt es ausser `except` noch einen aktiven Admin?
async fn other_admin_exists(state: &AppState, except: i64) -> ApiResult<bool> {
    let count: i64 = sqlx::query_scalar(
        "select count(*) from users where role = 'admin' and disabled = 0 and id != ?",
    )
    .bind(except)
    .fetch_one(&state.db)
    .await?;
    Ok(count > 0)
}

async fn update_user(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<i64>,
    Json(req): Json<UpdateUser>,
) -> ApiResult<Json<UserRow>> {
    let user = fetch_user(&state, id).await?;
    let demote = req.role.as_deref().is_some_and(|r| r != "admin");
    let disable = req.disabled == Some(true);
    if id == admin.id && (demote || disable) {
        return Err(ApiError::bad_request(
            "Das eigene Konto kann man nicht sperren oder herabstufen",
        ));
    }
    if user.role == "admin" && (demote || disable) && !other_admin_exists(&state, id).await? {
        return Err(ApiError::bad_request(
            "Es muss mindestens ein aktiver Admin bleiben",
        ));
    }
    if let Some(role) = &req.role {
        validate_role(role)?;
    }
    let mut tx = state.db.begin().await?;
    if let Some(name) = &req.display_name {
        let name: String = name.trim().chars().take(100).collect();
        sqlx::query("update users set display_name = ? where id = ?")
            .bind(name)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(role) = &req.role {
        sqlx::query("update users set role = ? where id = ?")
            .bind(role)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(disabled) = req.disabled {
        sqlx::query("update users set disabled = ? where id = ?")
            .bind(disabled)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if disabled {
            sqlx::query("delete from sessions where user_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("delete from api_tokens where user_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    let mut changes = Vec::new();
    if let Some(role) = &req.role {
        changes.push(format!("Rolle {role}"));
    }
    if let Some(disabled) = req.disabled {
        changes.push(if disabled { "gesperrt" } else { "entsperrt" }.to_owned());
    }
    if req.display_name.is_some() {
        changes.push("Name".to_owned());
    }
    audit::admin_event(
        &state.db,
        &admin.username,
        "Benutzer geändert",
        &format!("{}: {}", user.username, changes.join(", ")),
        "",
    )
    .await;
    Ok(Json(fetch_user(&state, id).await?))
}

async fn delete_user(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let user = fetch_user(&state, id).await?;
    if id == admin.id {
        return Err(ApiError::bad_request(
            "Das eigene Konto kann man nicht löschen",
        ));
    }
    if user.role == "admin" && !other_admin_exists(&state, id).await? {
        return Err(ApiError::bad_request(
            "Es muss mindestens ein aktiver Admin bleiben",
        ));
    }
    let mut tx = state.db.begin().await?;
    // Geteilte Adressbücher gehen an den löschenden Admin, statt mit dem
    // Konto zu verschwinden.
    sqlx::query("update address_books set owner_id = ? where owner_id = ? and personal = 0")
        .bind(admin.id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from ab_members where user_id = ? and ab_id in (select id from address_books where owner_id = ?)")
        .bind(admin.id)
        .bind(admin.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from users where id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    audit::admin_event(
        &state.db,
        &admin.username,
        "Benutzer gelöscht",
        &user.username,
        "",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Beendet alle Sitzungen der Konsole und alle Anmeldungen im Client.
async fn revoke_sessions(state: &AppState, user_id: i64) -> ApiResult<()> {
    sqlx::query("delete from sessions where user_id = ?")
        .bind(user_id)
        .execute(&state.db)
        .await?;
    sqlx::query("delete from api_tokens where user_id = ?")
        .bind(user_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

#[derive(Deserialize)]
struct ResetPassword {
    #[serde(default)]
    password: Option<String>,
}

async fn reset_password(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<i64>,
    Json(req): Json<ResetPassword>,
) -> ApiResult<Json<serde_json::Value>> {
    let user = fetch_user(&state, id).await?;
    if id == admin.id {
        return Err(ApiError::bad_request(
            "Das eigene Passwort unter «Mein Konto» ändern",
        ));
    }
    let (password, generated) = match req.password.filter(|p| !p.is_empty()) {
        Some(p) => {
            check_password_policy(&p)?;
            (p, false)
        }
        None => (auth::random_password(), true),
    };
    sqlx::query("update users set password_hash = ? where id = ?")
        .bind(hash_password(&password)?)
        .bind(id)
        .execute(&state.db)
        .await?;
    revoke_sessions(&state, id).await?;
    audit::admin_event(
        &state.db,
        &admin.username,
        "Passwort zurückgesetzt",
        &user.username,
        "",
    )
    .await;
    Ok(Json(json!({
        "password": if generated { Some(password) } else { None },
    })))
}

async fn reset_totp(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<i64>,
) -> ApiResult<Json<UserRow>> {
    let user = fetch_user(&state, id).await?;
    sqlx::query("update users set totp_secret = null, totp_pending = null where id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    revoke_sessions(&state, id).await?;
    audit::admin_event(
        &state.db,
        &admin.username,
        "Zwei-Faktor zurückgesetzt",
        &user.username,
        "",
    )
    .await;
    Ok(Json(fetch_user(&state, id).await?))
}

// ------------------------------------------------------------------- Geräte

#[derive(Serialize, Default)]
struct DeviceView {
    id: String,
    hostname: String,
    username: String,
    os: String,
    version: String,
    cpu: String,
    memory: String,
    note: String,
    ip: String,
    /// Konto, mit dem der Client auf diesem Gerät angemeldet ist.
    account: String,
    online: bool,
    first_seen_at: Option<i64>,
    last_seen_at: Option<i64>,
    registered_at: Option<i64>,
    /// Bei hbbs registriert. Ohne hbbs-Datenbank immer `null`.
    registered: Option<bool>,
}

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: String,
    hostname: String,
    username: String,
    os: String,
    version: String,
    cpu: String,
    memory: String,
    note: String,
    first_seen_at: Option<i64>,
    last_seen_at: Option<i64>,
    account: Option<String>,
}

async fn list_devices(State(state): State<AppState>, _: Admin) -> ApiResult<Json<Vec<DeviceView>>> {
    let rows: Vec<DeviceRow> = sqlx::query_as(
        "select d.id, d.hostname, d.username, d.os, d.version, d.cpu, d.memory, d.note,
                d.first_seen_at, d.last_seen_at, u.username as account
         from devices d left join users u on u.id = d.user_id",
    )
    .fetch_all(&state.db)
    .await?;
    let now = now();
    let mut devices: BTreeMap<String, DeviceView> = rows
        .into_iter()
        .map(|r| {
            let view = DeviceView {
                online: r
                    .last_seen_at
                    .is_some_and(|t| now - t <= ONLINE_WINDOW_SECS),
                id: r.id.clone(),
                hostname: r.hostname,
                username: r.username,
                os: r.os,
                version: r.version,
                cpu: r.cpu,
                memory: r.memory,
                note: r.note,
                first_seen_at: r.first_seen_at,
                last_seen_at: r.last_seen_at,
                account: r.account.unwrap_or_default(),
                ..Default::default()
            };
            (r.id, view)
        })
        .collect();
    if let Some(hbbs) = &state.hbbs {
        match hbbs.peers().await {
            Ok(peers) => {
                for d in devices.values_mut() {
                    d.registered = Some(false);
                }
                for p in peers {
                    let d = devices.entry(p.id.clone()).or_insert_with(|| DeviceView {
                        id: p.id.clone(),
                        ..Default::default()
                    });
                    d.registered = Some(true);
                    d.registered_at = p.registered_at;
                    d.ip = p.ip;
                }
            }
            Err(e) => tracing::warn!("hbbs-Datenbank nicht lesbar: {e:#}"),
        }
    }
    Ok(Json(devices.into_values().collect()))
}

#[derive(Deserialize)]
struct UpdateDevice {
    note: String,
}

async fn update_device(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    Json(req): Json<UpdateDevice>,
) -> ApiResult<StatusCode> {
    if id.is_empty() || id.len() > 100 {
        return Err(ApiError::not_found());
    }
    let note: String = req.note.trim().chars().take(1000).collect();
    sqlx::query(
        "insert into devices (id, note) values (?, ?)
         on conflict(id) do update set note = excluded.note",
    )
    .bind(&id)
    .bind(note)
    .execute(&state.db)
    .await?;
    audit::admin_event(&state.db, &admin.username, "Gerätenotiz geändert", &id, "").await;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_device(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    sqlx::query("delete from devices where id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    audit::admin_event(
        &state.db,
        &admin.username,
        "Gerät aus Liste entfernt",
        &id,
        "",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
