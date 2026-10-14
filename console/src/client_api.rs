//! Endpunkte, die der VibeDesk-Client von sich aus aufruft. Der Client leitet
//! die Adresse aus seinem ID-Server ab (`http://<server>:21114`).

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::{error::ApiResult, now, AppState};

const MAX_BODY: usize = 64 * 1024;
/// Heartbeats aktualisieren `last_seen_at` höchstens so oft, damit bei
/// aktiven Verbindungen (Heartbeat alle 3 Sekunden) nicht jede Anfrage schreibt.
const LAST_SEEN_RESOLUTION_SECS: i64 = 10;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/heartbeat", post(heartbeat))
        .route("/api/sysinfo", post(sysinfo))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

#[derive(Deserialize)]
struct Heartbeat {
    id: String,
    uuid: String,
}

#[derive(Deserialize)]
struct SysInfo {
    id: String,
    uuid: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    os: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    cpu: String,
    #[serde(default)]
    memory: String,
}

fn valid_ids(id: &str, uuid: &str) -> bool {
    !id.is_empty() && id.len() <= 100 && !uuid.is_empty() && uuid.len() <= 200
}

fn clip(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        s
    } else {
        s.chars().take(max).collect()
    }
}

/// Darf dieses Gerät Daten melden? Mit hbbs-Datenbank muss die UUID dort
/// registriert sein. Ohne gilt die UUID, mit der sich das Gerät zuerst gemeldet
/// hat, damit kein anderer Client dessen Eintrag übernehmen kann.
pub(crate) async fn authorized(state: &AppState, id: &str, uuid: &str) -> ApiResult<bool> {
    if let Some(hbbs) = &state.hbbs {
        return Ok(hbbs.is_registered(id, uuid).await?);
    }
    let known: Option<String> = sqlx::query_scalar("select uuid from devices where id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    Ok(match known {
        Some(known) => known.is_empty() || known == uuid,
        None => true,
    })
}

async fn heartbeat(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    let Ok(hb) = serde_json::from_slice::<Heartbeat>(&body) else {
        return Ok(Json(json!({})).into_response());
    };
    if !valid_ids(&hb.id, &hb.uuid) || !authorized(&state, &hb.id, &hb.uuid).await? {
        return Ok(Json(json!({})).into_response());
    }
    let now = now();
    let updated = sqlx::query(
        "update devices set last_seen_at = ?, uuid = ?
         where id = ? and first_seen_at is not null
           and (last_seen_at is null or last_seen_at <= ? or uuid != ?)",
    )
    .bind(now)
    .bind(&hb.uuid)
    .bind(&hb.id)
    .bind(now - LAST_SEEN_RESOLUTION_SECS)
    .bind(&hb.uuid)
    .execute(&state.db)
    .await?
    .rows_affected();
    if updated == 0 {
        let known: Option<i64> =
            sqlx::query_scalar("select 1 from devices where id = ? and first_seen_at is not null")
                .bind(&hb.id)
                .fetch_optional(&state.db)
                .await?;
        if known.is_none() {
            // Unbekanntes Gerät: Der Client soll seine Systeminfos schicken.
            return Ok(Json(json!({ "sysinfo": true })).into_response());
        }
    }
    Ok(Json(json!({})).into_response())
}

async fn sysinfo(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    let Ok(info) = serde_json::from_slice::<SysInfo>(&body) else {
        return Ok("INVALID".into_response());
    };
    if !valid_ids(&info.id, &info.uuid) || !authorized(&state, &info.id, &info.uuid).await? {
        return Ok("ID_NOT_FOUND".into_response());
    }
    let now = now();
    sqlx::query(
        "insert into devices (id, uuid, hostname, username, os, version, cpu, memory, first_seen_at, last_seen_at)
         values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         on conflict(id) do update set
           uuid = excluded.uuid, hostname = excluded.hostname, username = excluded.username,
           os = excluded.os, version = excluded.version, cpu = excluded.cpu, memory = excluded.memory,
           first_seen_at = coalesce(devices.first_seen_at, excluded.first_seen_at),
           last_seen_at = excluded.last_seen_at",
    )
    .bind(&info.id)
    .bind(&info.uuid)
    .bind(clip(info.hostname, 200))
    .bind(clip(info.username, 200))
    .bind(clip(info.os, 300))
    .bind(clip(info.version, 50))
    .bind(clip(info.cpu, 300))
    .bind(clip(info.memory, 50))
    .bind(now)
    .bind(now)
    .execute(&state.db)
    .await?;
    Ok("SYSINFO_UPDATED".into_response())
}
