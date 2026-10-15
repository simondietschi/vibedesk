//! VibeDesk-Konsole: Verwaltung von Benutzern und Geräten für einen VibeDesk-Server.
//!
//! Die Konsole beantwortet auf Port 21114 die Anfragen der Clients unter
//! `/api` (Heartbeats, Anmeldung, Adressbücher) und liefert unter `/` die
//! Weboberfläche mit ihrer API unter `/admin/api`.

pub mod account;
pub mod addressbook;
pub mod admin;
pub mod admin_books;
pub mod audit;
pub mod auth;
pub mod client_api;
pub mod db;
pub mod error;
pub mod groups;
pub mod hbbs;
pub mod totp;
pub mod web;

use std::{sync::Arc, time::SystemTime};

use axum::Router;
use sqlx::SqlitePool;

/// Ein Gerät gilt als online, wenn sein letzter Heartbeat höchstens so alt ist.
/// Clients senden alle 15 Sekunden einen, bei aktiver Verbindung alle 3.
pub const ONLINE_WINDOW_SECS: i64 = 45;

#[derive(Clone, Debug, Default)]
pub struct Settings {
    /// Cookies nur über HTTPS senden. Nötig, sobald die Konsole hinter einem
    /// HTTPS-Proxy läuft.
    pub secure_cookies: bool,
    /// `X-Forwarded-For` eines vorgeschalteten Proxys als Client-Adresse verwenden.
    pub trust_proxy: bool,
}

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub hbbs: Option<hbbs::HbbsDb>,
    pub settings: Settings,
    pub limiter: Arc<auth::LoginLimiter>,
    pub challenges: Arc<account::Challenges>,
}

impl AppState {
    pub fn new(db: SqlitePool, hbbs: Option<hbbs::HbbsDb>, settings: Settings) -> Self {
        Self {
            db,
            hbbs,
            settings,
            limiter: Arc::new(auth::LoginLimiter::default()),
            challenges: Arc::new(account::Challenges::default()),
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(client_api::router())
        .merge(account::router())
        .merge(addressbook::router())
        .merge(audit::router())
        .nest("/admin/api", admin::router(state.clone()))
        .merge(web::router())
        .with_state(state)
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}
