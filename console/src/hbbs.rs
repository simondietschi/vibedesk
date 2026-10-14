//! Lesezugriff auf die Gerätedatenbank von hbbs (`db_v2.sqlite3`).
//!
//! hbbs speichert dort jedes Gerät, das sich je registriert hat, mit der
//! UUID des Geräts. Die Konsole schreibt nie hinein.

use std::{str::FromStr, time::Duration};

use anyhow::Context;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};

#[derive(Clone)]
pub struct HbbsDb {
    pool: SqlitePool,
}

#[derive(Debug, Clone)]
pub struct HbbsPeer {
    pub id: String,
    pub registered_at: Option<i64>,
    pub ip: String,
}

#[derive(Deserialize, Default)]
struct PeerInfo {
    #[serde(default)]
    ip: String,
}

impl HbbsDb {
    pub async fn open(path: &str) -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{path}"))?
            .read_only(true)
            .busy_timeout(Duration::from_secs(2));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .with_context(|| format!("hbbs-Datenbank {path} lässt sich nicht öffnen"))?;
        Ok(Self { pool })
    }

    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn peers(&self) -> anyhow::Result<Vec<HbbsPeer>> {
        let rows: Vec<(String, Option<i64>, String)> = sqlx::query_as(
            "select id, cast(strftime('%s', created_at) as integer), info from peer",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, registered_at, info)| HbbsPeer {
                id,
                registered_at,
                ip: serde_json::from_str::<PeerInfo>(&info)
                    .unwrap_or_default()
                    .ip,
            })
            .collect())
    }

    /// Prüft, ob die Kombination aus ID und UUID bei hbbs registriert ist.
    /// Die Clients senden die UUID Base64-codiert.
    pub async fn is_registered(&self, id: &str, uuid_b64: &str) -> anyhow::Result<bool> {
        let Ok(uuid) = STANDARD.decode(uuid_b64) else {
            return Ok(false);
        };
        let found: Option<i64> = sqlx::query_scalar("select 1 from peer where id = ? and uuid = ?")
            .bind(id)
            .bind(uuid)
            .fetch_optional(&self.pool)
            .await?;
        Ok(found.is_some())
    }
}
