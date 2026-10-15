use std::{str::FromStr, time::Duration};

use anyhow::Context;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};

/// Schema-Versionen in Reihenfolge. Jede wird genau einmal ausgeführt,
/// der Stand steht in `PRAGMA user_version`.
const MIGRATIONS: &[&str] = &[
    r#"
create table users (
    id integer primary key,
    username text not null unique collate nocase,
    display_name text not null default '',
    password_hash text not null,
    role text not null check (role in ('admin', 'user')),
    disabled integer not null default 0,
    totp_secret text,
    totp_pending text,
    totp_last_step integer not null default 0,
    created_at integer not null,
    last_login_at integer
);

create table sessions (
    token_hash text primary key,
    user_id integer not null references users(id) on delete cascade,
    created_at integer not null,
    expires_at integer not null,
    last_seen_at integer not null
);
create index sessions_user on sessions(user_id);

create table devices (
    id text primary key,
    uuid text not null default '',
    hostname text not null default '',
    username text not null default '',
    os text not null default '',
    version text not null default '',
    cpu text not null default '',
    memory text not null default '',
    note text not null default '',
    first_seen_at integer,
    last_seen_at integer
);
"#,
    r#"
create table api_tokens (
    token_hash text primary key,
    user_id integer not null references users(id) on delete cascade,
    device_id text not null default '',
    created_at integer not null,
    last_used_at integer not null
);
create index api_tokens_user on api_tokens(user_id);

create table address_books (
    id integer primary key,
    guid text not null unique,
    name text not null,
    note text not null default '',
    owner_id integer not null references users(id) on delete cascade,
    personal integer not null default 0,
    created_at integer not null
);
create unique index address_books_personal on address_books(owner_id) where personal = 1;
create unique index address_books_shared_name on address_books(name collate nocase) where personal = 0;

create table ab_members (
    ab_id integer not null references address_books(id) on delete cascade,
    user_id integer not null references users(id) on delete cascade,
    rule integer not null check (rule in (1, 2, 3)),
    primary key (ab_id, user_id)
);

create table ab_peers (
    ab_id integer not null references address_books(id) on delete cascade,
    peer_id text not null,
    data text not null,
    primary key (ab_id, peer_id)
);

create table ab_tags (
    ab_id integer not null references address_books(id) on delete cascade,
    name text not null,
    color integer not null,
    primary key (ab_id, name)
);

alter table devices add column user_id integer references users(id) on delete set null;
"#,
    r#"
create table conn_log (
    id integer primary key,
    device_id text not null,
    conn_id integer not null,
    session_id text not null default '',
    ip text not null default '',
    peer_id text not null default '',
    peer_name text not null default '',
    conn_type integer,
    primary_auth integer,
    two_factor integer,
    note text not null default '',
    started_at integer not null,
    authed_at integer,
    ended_at integer
);
create index conn_log_device on conn_log(device_id, conn_id);
create index conn_log_session on conn_log(device_id, session_id);
create index conn_log_started on conn_log(started_at);

create table file_log (
    id integer primary key,
    device_id text not null,
    peer_id text not null default '',
    peer_name text not null default '',
    ip text not null default '',
    direction integer not null,
    path text not null default '',
    is_file integer not null default 0,
    num integer not null default 0,
    files text not null default '[]',
    created_at integer not null
);
create index file_log_created on file_log(created_at);

create table alarm_log (
    id integer primary key,
    device_id text not null,
    typ integer not null,
    peer_id text not null default '',
    peer_name text not null default '',
    ip text not null default '',
    info text not null default '{}',
    created_at integer not null
);
create index alarm_log_created on alarm_log(created_at);

create table admin_log (
    id integer primary key,
    username text not null default '',
    action text not null,
    detail text not null default '',
    ip text not null default '',
    created_at integer not null
);
create index admin_log_created on admin_log(created_at);

create table audit_nonces (
    nonce text primary key,
    created_at integer not null
);
"#,
    r#"
create table settings (
    key text primary key,
    value text not null
);

create table device_groups (
    id integer primary key,
    name text not null unique collate nocase,
    note text not null default '',
    created_at integer not null
);

create table device_group_users (
    group_id integer not null references device_groups(id) on delete cascade,
    user_id integer not null references users(id) on delete cascade,
    primary key (group_id, user_id)
);

alter table devices add column group_id integer references device_groups(id) on delete set null;
"#,
];

pub async fn open(path: &str) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{path}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .with_context(|| format!("Datenbank {path} lässt sich nicht öffnen"))?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Datenbank im Speicher, für Tests.
pub async fn open_memory() -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
    // Eine einzige Verbindung, sonst hätte jede ihre eigene leere Datenbank.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    migrate(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    let version: i64 = sqlx::query_scalar("pragma user_version")
        .fetch_one(pool)
        .await?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::raw_sql(&format!("pragma user_version = {}", i + 1))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!("Datenbank auf Version {} gebracht", i + 1);
    }
    Ok(())
}
