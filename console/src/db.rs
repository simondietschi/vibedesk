use std::{str::FromStr, time::Duration};

use anyhow::Context;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};

/// Schema-Versionen in Reihenfolge. Jede wird genau einmal ausgeführt,
/// der Stand steht in `PRAGMA user_version`.
const MIGRATIONS: &[&str] = &[r#"
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
"#];

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
