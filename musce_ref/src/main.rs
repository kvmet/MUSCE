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

    // SQLite is the built store. The URL selects its file or in-memory database.
    let db_url = std::env::var("MUSCE_DB").unwrap_or_else(|_| "sqlite://musce.db".into());
    let store = WorldStore::connect(&db_url).await?;
    tracing::info!(%db_url, "connected");

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received");
            shutdown.store(true, Ordering::Relaxed);
        });
    }

    let report = run(store, Config::default(), shutdown, musce_ref::app()).await?;
    tracing::info!(ticks = report.ticks, saves = report.saves, "stopped");
    Ok(())
}
