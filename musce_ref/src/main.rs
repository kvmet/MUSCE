use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use musce::store::WorldStore;
use musce::{Config, run};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // The URL scheme picks the backend: `sqlite://…` (the default) or
    // `postgres://…`. The game code is identical either way.
    let db_url = std::env::var("MUSCE_DB").unwrap_or_else(|_| "sqlite://musce.db".into());
    let store = WorldStore::connect(&db_url).await?;
    tracing::info!(%db_url, "connected");

    // Accounts live in their own database so a dev world reseed (deleting the
    // world DB) does not wipe logins.
    let accounts_url =
        std::env::var("MUSCE_ACCOUNTS_DB").unwrap_or_else(|_| "sqlite://accounts.sqlite".into());
    let account_store = musce::auth::AccountBackend::connect(&accounts_url).await?;
    tracing::info!(%accounts_url, "accounts connected");

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received");
            shutdown.store(true, Ordering::Relaxed);
        });
    }

    let report = run(
        store,
        account_store,
        Config::default(),
        shutdown,
        musce_ref::game(),
    )
    .await?;
    tracing::info!(ticks = report.ticks, saves = report.saves, "stopped");
    Ok(())
}
