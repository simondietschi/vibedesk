//! Passwörter, Sitzungen, Login-Sperre und die Extraktoren für angemeldete Benutzer.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Mutex, OnceLock},
};

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{header, request::Parts, HeaderMap, HeaderValue},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    error::{ApiError, ApiResult},
    now, AppState,
};

pub const SESSION_COOKIE: &str = "vd_session";
/// Höchstdauer einer Sitzung.
pub const SESSION_MAX_SECS: i64 = 12 * 3600;
/// Sitzungen ohne Aktivität laufen nach dieser Zeit ab.
pub const SESSION_IDLE_SECS: i64 = 2 * 3600;
pub const MIN_PASSWORD_LEN: usize = 10;
const MAX_PASSWORD_LEN: usize = 256;

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("Passwort-Hash fehlgeschlagen: {e}"))
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// Prüft ein Passwort gegen einen festen Hash, damit eine Anmeldung mit
/// unbekanntem Benutzernamen gleich lange dauert wie mit einem bekannten.
pub fn burn_password_check(password: &str) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let hash = DUMMY.get_or_init(|| hash_password("vibedesk-dummy").unwrap_or_default());
    verify_password(hash, password);
}

pub fn check_password_policy(password: &str) -> ApiResult<()> {
    let len = password.chars().count();
    if len < MIN_PASSWORD_LEN {
        return Err(ApiError::bad_request(format!(
            "Das Passwort muss mindestens {MIN_PASSWORD_LEN} Zeichen lang sein"
        )));
    }
    if len > MAX_PASSWORD_LEN {
        return Err(ApiError::bad_request("Das Passwort ist zu lang"));
    }
    Ok(())
}

pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Zufälliges Passwort für neue Konten, ohne leicht verwechselbare Zeichen.
pub fn random_password() -> String {
    const CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| CHARS[(rng.next_u32() as usize) % CHARS.len()] as char)
        .collect()
}

pub(crate) fn token_hash(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub async fn create_session(state: &AppState, user_id: i64) -> ApiResult<String> {
    let token = random_token(32);
    let now = now();
    sqlx::query(
        "insert into sessions (token_hash, user_id, created_at, expires_at, last_seen_at)
         values (?, ?, ?, ?, ?)",
    )
    .bind(token_hash(&token))
    .bind(user_id)
    .bind(now)
    .bind(now + SESSION_MAX_SECS)
    .bind(now)
    .execute(&state.db)
    .await?;
    sqlx::query("delete from sessions where expires_at < ? or last_seen_at < ?")
        .bind(now)
        .bind(now - SESSION_IDLE_SECS)
        .execute(&state.db)
        .await?;
    Ok(token)
}

pub async fn delete_session(state: &AppState, token: &str) -> ApiResult<()> {
    sqlx::query("delete from sessions where token_hash = ?")
        .bind(token_hash(token))
        .execute(&state.db)
        .await?;
    Ok(())
}

pub fn session_cookie(state: &AppState, token: &str, max_age: i64) -> HeaderValue {
    let secure = if state.settings.secure_cookies {
        "; Secure"
    } else {
        ""
    };
    let value = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
    );
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static(""))
}

pub fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.to_owned())
}

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct CurrentUser {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub totp_enabled: bool,
}

impl CurrentUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let token = session_token(&parts.headers).ok_or_else(ApiError::unauthorized)?;
        let hash = token_hash(&token);
        let now = now();
        let user: Option<(CurrentUser, i64)> = sqlx::query_as::<_, (i64, String, String, String, bool, i64)>(
            "select u.id, u.username, u.display_name, u.role, u.totp_secret is not null, s.last_seen_at
             from sessions s join users u on u.id = s.user_id
             where s.token_hash = ? and s.expires_at > ? and s.last_seen_at > ? and u.disabled = 0",
        )
        .bind(&hash)
        .bind(now)
        .bind(now - SESSION_IDLE_SECS)
        .fetch_optional(&state.db)
        .await?
        .map(|(id, username, display_name, role, totp_enabled, last_seen)| {
            (
                CurrentUser {
                    id,
                    username,
                    display_name,
                    role,
                    totp_enabled,
                },
                last_seen,
            )
        });
        let (user, last_seen) = user.ok_or_else(ApiError::unauthorized)?;
        if now - last_seen > 60 {
            sqlx::query("update sessions set last_seen_at = ? where token_hash = ?")
                .bind(now)
                .bind(&hash)
                .execute(&state.db)
                .await?;
        }
        Ok(user)
    }
}

/// Angemeldeter Benutzer mit Rolle Admin.
pub struct Admin(pub CurrentUser);

impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let user = CurrentUser::from_request_parts(parts, state).await?;
        if !user.is_admin() {
            return Err(ApiError::forbidden());
        }
        Ok(Admin(user))
    }
}

/// Adresse des Clients, bei `trust_proxy` aus `X-Forwarded-For`.
pub struct ClientIp(pub Option<IpAddr>);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        if state.settings.trust_proxy {
            // Der letzte Eintrag stammt vom eigenen Proxy, die davor kann der Client fälschen.
            let forwarded = parts
                .headers
                .get_all("x-forwarded-for")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|v| v.split(','))
                .filter_map(|ip| ip.trim().parse().ok())
                .next_back();
            if forwarded.is_some() {
                return Ok(ClientIp(forwarded));
            }
        }
        Ok(ClientIp(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ci| ci.0.ip()),
        ))
    }
}

const MAX_FAILURES: u32 = 5;
const FAILURE_WINDOW_SECS: i64 = 15 * 60;
const LOCK_SECS: i64 = 15 * 60;

#[derive(Default)]
struct Failures {
    count: u32,
    first_at: i64,
    locked_until: i64,
}

/// Sperrt Benutzername und IP-Adresse nach zu vielen Fehlversuchen.
#[derive(Default)]
pub struct LoginLimiter {
    entries: Mutex<HashMap<String, Failures>>,
}

impl LoginLimiter {
    pub fn keys(username: &str, ip: Option<IpAddr>) -> Vec<String> {
        let mut keys = vec![format!("user:{}", username.to_lowercase())];
        if let Some(ip) = ip {
            keys.push(format!("ip:{ip}"));
        }
        keys
    }

    /// Gibt die verbleibenden Sekunden der Sperre zurück, falls gesperrt.
    pub fn locked(&self, keys: &[String], now: i64) -> Option<i64> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        keys.iter()
            .filter_map(|k| entries.get(k))
            .map(|f| f.locked_until - now)
            .filter(|&secs| secs > 0)
            .max()
    }

    pub fn fail(&self, keys: &[String], now: i64) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() > 10_000 {
            entries.retain(|_, f| f.locked_until > now || now - f.first_at < FAILURE_WINDOW_SECS);
        }
        for key in keys {
            let f = entries.entry(key.clone()).or_default();
            if now - f.first_at > FAILURE_WINDOW_SECS {
                *f = Failures {
                    count: 0,
                    first_at: now,
                    locked_until: 0,
                };
            }
            f.count += 1;
            if f.count >= MAX_FAILURES {
                f.locked_until = now + LOCK_SECS;
            }
        }
    }

    pub fn succeed(&self, keys: &[String]) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        for key in keys {
            entries.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let hash = hash_password("richtig-lang-genug").unwrap();
        assert!(verify_password(&hash, "richtig-lang-genug"));
        assert!(!verify_password(&hash, "falsch"));
        assert!(!verify_password("kein-hash", "falsch"));
    }

    #[test]
    fn limiter_locks_after_five_failures() {
        let limiter = LoginLimiter::default();
        let keys = LoginLimiter::keys("Anna", None);
        for _ in 0..4 {
            limiter.fail(&keys, 1000);
        }
        assert_eq!(limiter.locked(&keys, 1000), None);
        limiter.fail(&keys, 1000);
        assert_eq!(limiter.locked(&keys, 1000), Some(LOCK_SECS));
        assert_eq!(
            limiter.locked(&LoginLimiter::keys("anna", None), 1000 + LOCK_SECS),
            None
        );
    }

    #[test]
    fn reads_session_cookie_among_others() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; vd_session=abc; b=2"),
        );
        assert_eq!(session_token(&headers).as_deref(), Some("abc"));
    }
}
