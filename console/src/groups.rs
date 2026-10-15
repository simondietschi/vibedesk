//! Gerätegruppen und Zugriffsregeln.
//!
//! Ein Gerät gehört zu höchstens einer Gruppe. Benutzer erhalten Zugriff auf
//! ganze Gruppen. Mit dem Zugriffsmodus «groups» setzt hbbs die Regeln beim
//! Verbindungsaufbau durch (siehe `server/src/vibedesk_access.rs`): Admins
//! dürfen alles, Benutzer ihre eigenen Geräte und die ihrer Gruppen.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    audit,
    auth::Admin,
    error::{ApiError, ApiResult},
    now, AppState,
};

/// Ohne Regeln: jeder darf mit Geräte-ID und Passwort verbinden.
pub const ACCESS_OPEN: &str = "open";
/// Nur angemeldete Benutzer, und nur auf erlaubte Geräte.
pub const ACCESS_GROUPS: &str = "groups";

pub async fn access_mode(state: &AppState) -> ApiResult<String> {
    let mode: Option<String> =
        sqlx::query_scalar("select value from settings where key = 'access_mode'")
            .fetch_optional(&state.db)
            .await?;
    Ok(mode.unwrap_or_else(|| ACCESS_OPEN.to_owned()))
}

#[derive(Serialize)]
pub struct GroupUser {
    user_id: i64,
    username: String,
}

#[derive(Serialize)]
pub struct GroupView {
    id: i64,
    name: String,
    note: String,
    device_count: i64,
    users: Vec<GroupUser>,
}

async fn view(state: &AppState, id: i64) -> ApiResult<GroupView> {
    let (name, note, device_count): (String, String, i64) = sqlx::query_as(
        "select g.name, g.note, (select count(*) from devices d where d.group_id = g.id)
         from device_groups g where g.id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let users = sqlx::query_as::<_, (i64, String)>(
        "select u.id, u.username from device_group_users gu join users u on u.id = gu.user_id
         where gu.group_id = ? order by u.username collate nocase",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|(user_id, username)| GroupUser { user_id, username })
    .collect();
    Ok(GroupView {
        id,
        name,
        note,
        device_count,
        users,
    })
}

pub async fn list(State(state): State<AppState>, _: Admin) -> ApiResult<Json<Vec<GroupView>>> {
    let ids: Vec<i64> =
        sqlx::query_scalar("select id from device_groups order by name collate nocase")
            .fetch_all(&state.db)
            .await?;
    let mut groups = Vec::with_capacity(ids.len());
    for id in ids {
        groups.push(view(&state, id).await?);
    }
    Ok(Json(groups))
}

fn clean_name(name: &str) -> ApiResult<String> {
    let name: String = name.trim().chars().take(100).collect();
    if name.is_empty() {
        return Err(ApiError::bad_request("Bitte einen Namen angeben"));
    }
    Ok(name)
}

fn name_conflict(e: sqlx::Error) -> ApiError {
    match e {
        sqlx::Error::Database(db) if db.is_unique_violation() => ApiError::new(
            StatusCode::CONFLICT,
            "Eine Gruppe mit diesem Namen gibt es schon",
        ),
        e => e.into(),
    }
}

#[derive(Deserialize)]
pub struct GroupInput {
    name: Option<String>,
    note: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(req): Json<GroupInput>,
) -> ApiResult<(StatusCode, Json<GroupView>)> {
    let name = clean_name(req.name.as_deref().unwrap_or_default())?;
    let note: String = req
        .note
        .unwrap_or_default()
        .trim()
        .chars()
        .take(1000)
        .collect();
    let id = sqlx::query("insert into device_groups (name, note, created_at) values (?, ?, ?)")
        .bind(&name)
        .bind(note)
        .bind(now())
        .execute(&state.db)
        .await
        .map_err(name_conflict)?
        .last_insert_rowid();
    audit::admin_event(
        &state.db,
        &admin.username,
        "Gerätegruppe angelegt",
        &name,
        "",
    )
    .await;
    Ok((StatusCode::CREATED, Json(view(&state, id).await?)))
}

pub async fn update(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<i64>,
    Json(req): Json<GroupInput>,
) -> ApiResult<Json<GroupView>> {
    view(&state, id).await?;
    if let Some(name) = &req.name {
        sqlx::query("update device_groups set name = ? where id = ?")
            .bind(clean_name(name)?)
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(name_conflict)?;
    }
    if let Some(note) = &req.note {
        sqlx::query("update device_groups set note = ? where id = ?")
            .bind(note.trim().chars().take(1000).collect::<String>())
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    let group = view(&state, id).await?;
    audit::admin_event(
        &state.db,
        &admin.username,
        "Gerätegruppe geändert",
        &group.name,
        "",
    )
    .await;
    Ok(Json(group))
}

pub async fn remove(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let group = view(&state, id).await?;
    sqlx::query("delete from device_groups where id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    audit::admin_event(
        &state.db,
        &admin.username,
        "Gerätegruppe gelöscht",
        &group.name,
        "",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_user(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path((id, user_id)): Path<(i64, i64)>,
) -> ApiResult<Json<GroupView>> {
    view(&state, id).await?;
    let username: String = sqlx::query_scalar("select username from users where id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::bad_request("Unbekannter Benutzer"))?;
    sqlx::query(
        "insert into device_group_users (group_id, user_id) values (?, ?) on conflict do nothing",
    )
    .bind(id)
    .bind(user_id)
    .execute(&state.db)
    .await?;
    let group = view(&state, id).await?;
    audit::admin_event(
        &state.db,
        &admin.username,
        "Gruppenzugriff erteilt",
        &format!("{}: {username}", group.name),
        "",
    )
    .await;
    Ok(Json(group))
}

pub async fn remove_user(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path((id, user_id)): Path<(i64, i64)>,
) -> ApiResult<Json<GroupView>> {
    view(&state, id).await?;
    let username: String = sqlx::query_scalar("select username from users where id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?
        .unwrap_or_default();
    sqlx::query("delete from device_group_users where group_id = ? and user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&state.db)
        .await?;
    let group = view(&state, id).await?;
    audit::admin_event(
        &state.db,
        &admin.username,
        "Gruppenzugriff entzogen",
        &format!("{}: {username}", group.name),
        "",
    )
    .await;
    Ok(Json(group))
}

#[derive(Deserialize)]
pub struct DeviceGroupInput {
    group_id: Option<i64>,
}

pub async fn set_device_group(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(device_id): Path<String>,
    Json(req): Json<DeviceGroupInput>,
) -> ApiResult<StatusCode> {
    if device_id.is_empty() || device_id.len() > 100 {
        return Err(ApiError::not_found());
    }
    let group_name = match req.group_id {
        Some(id) => view(&state, id).await?.name,
        None => "keine".to_owned(),
    };
    sqlx::query(
        "insert into devices (id, group_id) values (?, ?)
         on conflict(id) do update set group_id = excluded.group_id",
    )
    .bind(&device_id)
    .bind(req.group_id)
    .execute(&state.db)
    .await?;
    audit::admin_event(
        &state.db,
        &admin.username,
        "Gerät einer Gruppe zugeordnet",
        &format!("{device_id}: {group_name}"),
        "",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_settings(State(state): State<AppState>, _: Admin) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "access_mode": access_mode(&state).await? })))
}

#[derive(Deserialize)]
pub struct SettingsInput {
    access_mode: String,
}

pub async fn put_settings(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(req): Json<SettingsInput>,
) -> ApiResult<Json<Value>> {
    if req.access_mode != ACCESS_OPEN && req.access_mode != ACCESS_GROUPS {
        return Err(ApiError::bad_request("Unbekannter Zugriffsmodus"));
    }
    sqlx::query(
        "insert into settings (key, value) values ('access_mode', ?)
         on conflict(key) do update set value = excluded.value",
    )
    .bind(&req.access_mode)
    .execute(&state.db)
    .await?;
    audit::admin_event(
        &state.db,
        &admin.username,
        "Zugriffsmodus geändert",
        &req.access_mode,
        "",
    )
    .await;
    Ok(Json(json!({ "access_mode": req.access_mode })))
}
