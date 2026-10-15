use std::net::SocketAddr;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use vibedesk_console::{admin, audit, auth, db, hbbs::HbbsDb, router, AppState, Settings};

#[derive(Parser)]
#[command(
    version,
    about = "VibeDesk-Konsole: Benutzer und Geräte eines VibeDesk-Servers verwalten"
)]
struct Cli {
    /// Datenbank der Konsole
    #[arg(
        long,
        env = "VIBEDESK_CONSOLE_DB",
        default_value = "console.sqlite3",
        global = true
    )]
    db: String,

    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Konsole starten (Standard)
    Serve(ServeArgs),
    /// Admin-Konto anlegen und dessen Passwort ausgeben
    CreateAdmin { username: String },
    /// Neues Passwort für ein Konto setzen und ausgeben, Zwei-Faktor bleibt
    ResetPassword { username: String },
}

#[derive(Args)]
struct ServeArgs {
    /// Adresse und Port. Die Clients erwarten die Konsole auf Port 21114.
    #[arg(long, env = "VIBEDESK_CONSOLE_LISTEN", default_value = "0.0.0.0:21114")]
    listen: SocketAddr,

    /// Gerätedatenbank von hbbs (db_v2.sqlite3), wird nur gelesen
    #[arg(long, env = "VIBEDESK_HBBS_DB")]
    hbbs_db: Option<String>,

    /// Cookies nur über HTTPS senden (hinter einem HTTPS-Proxy einschalten)
    #[arg(long, env = "VIBEDESK_CONSOLE_SECURE_COOKIES")]
    secure_cookies: bool,

    /// Client-Adresse aus X-Forwarded-For übernehmen (nur hinter eigenem Proxy)
    #[arg(long, env = "VIBEDESK_CONSOLE_TRUST_PROXY")]
    trust_proxy: bool,

    /// Protokolleinträge nach so vielen Tagen löschen
    #[arg(long, env = "VIBEDESK_CONSOLE_LOG_DAYS", default_value_t = 365)]
    log_days: i64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    let pool = db::open(&cli.db).await?;
    match cli.command {
        Some(Command::CreateAdmin { username }) => {
            let password = auth::random_password();
            admin::insert_user(&pool, &username, "", "admin", &password)
                .await
                .map_err(|e| anyhow::anyhow!(e.message))?;
            println!("Admin «{username}» angelegt. Passwort: {password}");
            println!("Bitte nach der ersten Anmeldung ändern und Zwei-Faktor einrichten.");
        }
        Some(Command::ResetPassword { username }) => {
            let password = auth::random_password();
            let updated = sqlx::query("update users set password_hash = ? where username = ?")
                .bind(auth::hash_password(&password)?)
                .bind(&username)
                .execute(&pool)
                .await?
                .rows_affected();
            anyhow::ensure!(updated == 1, "Kein Konto «{username}» gefunden");
            sqlx::query(
                "delete from sessions where user_id = (select id from users where username = ?)",
            )
            .bind(&username)
            .execute(&pool)
            .await?;
            println!("Neues Passwort für «{username}»: {password}");
        }
        Some(Command::Serve(args)) => serve(pool, args).await?,
        None => serve(pool, cli.serve).await?,
    }
    Ok(())
}

async fn serve(pool: sqlx::SqlitePool, args: ServeArgs) -> anyhow::Result<()> {
    let hbbs = match &args.hbbs_db {
        Some(path) => Some(HbbsDb::open(path).await?),
        None => {
            tracing::warn!(
                "Keine hbbs-Datenbank angegeben (VIBEDESK_HBBS_DB). Heartbeats werden nicht gegen hbbs geprüft."
            );
            None
        }
    };
    let users: i64 = sqlx::query_scalar("select count(*) from users")
        .fetch_one(&pool)
        .await?;
    if users == 0 {
        tracing::warn!(
            "Noch kein Konto vorhanden. Anlegen mit: vibedesk-console create-admin <name>"
        );
    }
    let settings = Settings {
        secure_cookies: args.secure_cookies,
        trust_proxy: args.trust_proxy,
    };
    let log_days = args.log_days.max(1);
    let prune_pool = pool.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
        loop {
            interval.tick().await;
            if let Err(e) = audit::prune(&prune_pool, log_days).await {
                tracing::warn!("Alte Protokolleinträge konnten nicht gelöscht werden: {e:#}");
            }
        }
    });
    let app = router(AppState::new(pool, hbbs, settings));
    let listen = args.listen;
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("Port {listen} ist nicht verfügbar"))?;
    tracing::info!("VibeDesk-Konsole läuft auf http://{listen}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}
