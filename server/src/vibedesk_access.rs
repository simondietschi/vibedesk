//! VibeDesk: Zugriffsregeln der Konsole beim Verbindungsaufbau durchsetzen.
//!
//! Ist `VIBEDESK_CONSOLE_DB` gesetzt und in der Konsole der Zugriffsmodus
//! «groups» eingeschaltet, vermittelt hbbs eine Verbindung nur, wenn das
//! Anmelde-Token des steuernden Clients einem aktiven Konto gehört und dieses
//! das Zielgerät erreichen darf: Admins alle Geräte, Benutzer ihre eigenen und
//! die ihrer Gerätegruppen. Die Datenbank der Konsole wird nur gelesen.

use std::{
    str::FromStr,
    sync::Mutex as StdMutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hbb_common::{log, tokio::sync::Mutex};
use once_cell::sync::Lazy;
use sodiumoxide::crypto::hash::sha256;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};

/// Wie in der Konsole: Client-Anmeldungen verfallen nach 90 Tagen ohne Nutzung.
const TOKEN_IDLE_SECS: i64 = 90 * 24 * 3600;
const MODE_CACHE: Duration = Duration::from_secs(10);
const RETRY_OPEN: Duration = Duration::from_secs(30);

const MSG_LOGIN: &str = "Bitte zuerst mit einem Konto anmelden. Dieser Server erlaubt Verbindungen nur angemeldeten Benutzern.";
const MSG_EXPIRED: &str =
    "Die Anmeldung ist abgelaufen oder das Konto gesperrt. Bitte neu anmelden.";
const MSG_DENIED: &str = "Kein Zugriff auf dieses Gerät. Ein Admin kann ihn in der Konsole über eine Gerätegruppe erteilen.";
const MSG_ERROR: &str = "Die Zugriffsregeln konnten nicht geprüft werden.";

struct Db {
    pool: Option<SqlitePool>,
    last_attempt: Option<Instant>,
}

static DB: Lazy<Mutex<Db>> = Lazy::new(|| {
    Mutex::new(Db {
        pool: None,
        last_attempt: None,
    })
});
static MODE: Lazy<StdMutex<Option<(Instant, bool)>>> = Lazy::new(|| StdMutex::new(None));

/// Öffnet die Datenbank der Konsole beim ersten Bedarf. Fehlt sie noch, weil
/// die Konsole später startet, wird es nach einer Pause erneut versucht.
async fn pool() -> Option<SqlitePool> {
    let path = std::env::var("VIBEDESK_CONSOLE_DB")
        .ok()
        .filter(|p| !p.is_empty())?;
    let mut db = DB.lock().await;
    if let Some(pool) = &db.pool {
        return Some(pool.clone());
    }
    if db.last_attempt.is_some_and(|t| t.elapsed() < RETRY_OPEN) {
        return None;
    }
    db.last_attempt = Some(Instant::now());
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{path}"))
        .ok()?
        .read_only(true)
        .busy_timeout(Duration::from_secs(2));
    match SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
    {
        Ok(pool) => {
            log::info!("VibeDesk-Konsole {path} für Zugriffsregeln geöffnet");
            db.pool = Some(pool.clone());
            Some(pool)
        }
        Err(e) => {
            log::warn!("VibeDesk-Konsole {path} nicht lesbar, Zugriffsregeln inaktiv: {e}");
            None
        }
    }
}

async fn groups_mode(pool: &SqlitePool) -> bool {
    if let Ok(cached) = MODE.lock() {
        if let Some((at, enabled)) = *cached {
            if at.elapsed() < MODE_CACHE {
                return enabled;
            }
        }
    }
    let enabled = match sqlx::query_scalar::<_, String>(
        "select value from settings where key = 'access_mode'",
    )
    .fetch_optional(pool)
    .await
    {
        Ok(mode) => mode.as_deref() == Some("groups"),
        Err(e) => {
            log::warn!("Zugriffsmodus der Konsole nicht lesbar: {e}");
            false
        }
    };
    if let Ok(mut cached) = MODE.lock() {
        *cached = Some((Instant::now(), enabled));
    }
    enabled
}

fn token_hash(token: &str) -> String {
    base64::encode_config(sha256::hash(token.as_bytes()).0, base64::URL_SAFE_NO_PAD)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// Gibt den Grund zurück, falls die Verbindung zu `target_id` abgelehnt wird.
pub(crate) async fn check(token: &str, target_id: &str) -> Option<String> {
    let pool = pool().await?;
    if !groups_mode(&pool).await {
        return None;
    }
    check_user(&pool, token, target_id).await
}

async fn check_user(pool: &SqlitePool, token: &str, target_id: &str) -> Option<String> {
    if token.is_empty() {
        return Some(MSG_LOGIN.to_owned());
    }
    let user = sqlx::query_as::<_, (i64, String)>(
        "select u.id, u.role from api_tokens t join users u on u.id = t.user_id
         where t.token_hash = ? and t.last_used_at > ? and u.disabled = 0",
    )
    .bind(token_hash(token))
    .bind(now() - TOKEN_IDLE_SECS)
    .fetch_optional(pool)
    .await;
    let (user_id, role) = match user {
        Ok(Some(user)) => user,
        Ok(None) => return Some(MSG_EXPIRED.to_owned()),
        Err(e) => {
            log::error!("Zugriffsregeln nicht lesbar: {e}");
            return Some(MSG_ERROR.to_owned());
        }
    };
    if role == "admin" {
        return None;
    }
    let allowed = sqlx::query_scalar::<_, i64>(
        "select 1 from devices where id = ?1 and (user_id = ?2
         or group_id in (select group_id from device_group_users where user_id = ?2))",
    )
    .bind(target_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await;
    match allowed {
        Ok(Some(_)) => None,
        Ok(None) => Some(MSG_DENIED.to_owned()),
        Err(e) => {
            log::error!("Zugriffsregeln nicht lesbar: {e}");
            Some(MSG_ERROR.to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prüft die Regeln gegen eine Datenbank mit dem Schema der Konsole.
    #[test]
    fn rules_follow_console_data() {
        hbb_common::tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async {
                let pool = SqlitePoolOptions::new()
                    .max_connections(1)
                    .connect("sqlite::memory:")
                    .await
                    .unwrap();
                sqlx::query(
                    "create table users (id integer primary key, role text, disabled integer);
                     create table api_tokens (token_hash text, user_id integer, last_used_at integer);
                     create table devices (id text, user_id integer, group_id integer);
                     create table device_group_users (group_id integer, user_id integer);
                     insert into users values (1, 'admin', 0), (2, 'user', 0), (3, 'user', 1);
                     insert into devices values ('own', 2, null), ('grouped', null, 7), ('other', null, 8);
                     insert into device_group_users values (7, 2);",
                )
                .execute(&pool)
                .await
                .unwrap();
                for (token, user) in [("admin-token", 1), ("user-token", 2), ("locked-token", 3)] {
                    sqlx::query("insert into api_tokens values (?, ?, ?)")
                        .bind(token_hash(token))
                        .bind(user)
                        .bind(now())
                        .execute(&pool)
                        .await
                        .unwrap();
                }
                assert_eq!(check_user(&pool, "", "own").await.as_deref(), Some(MSG_LOGIN));
                assert_eq!(check_user(&pool, "falsch", "own").await.as_deref(), Some(MSG_EXPIRED));
                assert_eq!(check_user(&pool, "locked-token", "own").await.as_deref(), Some(MSG_EXPIRED));
                assert_eq!(check_user(&pool, "admin-token", "other").await, None);
                assert_eq!(check_user(&pool, "user-token", "own").await, None);
                assert_eq!(check_user(&pool, "user-token", "grouped").await, None);
                assert_eq!(check_user(&pool, "user-token", "other").await.as_deref(), Some(MSG_DENIED));
                assert_eq!(check_user(&pool, "user-token", "unbekannt").await.as_deref(), Some(MSG_DENIED));
            });
    }

    // Muss zum Hash der Konsole passen (SHA-256, Base64 URL-safe ohne Padding).
    #[test]
    fn token_hash_matches_console() {
        assert_eq!(
            token_hash("abc"),
            "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
        );
    }
}
