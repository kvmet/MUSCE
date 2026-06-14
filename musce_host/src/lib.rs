//! The runtime: a single sim thread that owns the `World` and ticks at a fixed
//! cadence, with persistence on a tokio task. Commands in / events out (no-op
//! until networking lands). The world is loaded before the first tick and saved
//! synchronously on shutdown.

mod dispatch;
mod session;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, Sender};
use musce_core::{EntityId, Snapshot, World};
use musce_persistence::{Loaded, Persistence, SqliteStore};
use musce_proto::{Command, Outgoing};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::dispatch::Dispatch;

/// Base tick period. 100ms = 10 Hz. Change here to retune the heartbeat.
pub const TICK_INTERVAL: Duration = Duration::from_millis(100);
/// Periodic snapshot cadence, in ticks. Tick-count (not wall-clock) so it stays
/// deterministic. ~every 5 s at the default tick rate.
pub const SAVE_EVERY: u32 = 50;
/// Default TCP listen address for the line-mode transport.
pub const LISTEN_ADDR: &str = "127.0.0.1:4000";

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub tick_interval: Duration,
    pub save_every: u32,
    /// Where the TCP transport binds. `None` runs headless (no networking),
    /// which is what the tests use.
    pub listen_addr: Option<SocketAddr>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tick_interval: TICK_INTERVAL,
            save_every: SAVE_EVERY,
            listen_addr: Some(LISTEN_ADDR.parse().expect("valid default listen addr")),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RunReport {
    pub ticks: u64,
    pub saves: u64,
}

/// Per-tick context handed to systems. Carries both clocks: `tick` (deterministic
/// sim time, the default for game logic) and `now` (wall-clock, for real-world
/// scheduling). Captured once per tick so every system sees the same instant.
pub struct TickCtx {
    pub tick: u64,
    pub now: SystemTime,
}

/// Result of a completed save, sent back to the sim thread so it can clear the
/// pending-delete set only once the write is durable.
enum Ack {
    Saved(Vec<EntityId>),
    Failed,
}

/// Boot, run the tick loop until `shutdown` is set, then save and stop. The
/// `World` is built and owned entirely by the sim thread.
pub async fn run(
    store: SqliteStore,
    config: Config,
    shutdown: Arc<AtomicBool>,
) -> Result<RunReport, Box<dyn std::error::Error + Send + Sync>> {
    store.init().await?;
    let loaded = store.load().await?; // boot load, before any tick

    let (snap_tx, snap_rx) = tokio::sync::mpsc::unbounded_channel::<Snapshot>();
    let (ack_tx, ack_rx) = crossbeam_channel::unbounded::<Ack>();
    let (done_tx, done_rx) = oneshot::channel::<RunReport>();

    // The net <-> sim boundary: commands in (crossbeam, drained by the sim
    // thread), events out (tokio mpsc, drained by the net router).
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<Command>();
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<Outgoing>();

    if let Some(addr) = config.listen_addr {
        match musce_net::start(addr, cmd_tx, event_rx).await {
            Ok(bound) => tracing::info!(%bound, "listening"),
            Err(e) => tracing::error!(error = %e, "failed to start networking"),
        }
    }

    let persist = tokio::spawn(persistence_task(store.clone(), snap_rx, ack_tx));

    let sim = std::thread::Builder::new()
        .name("musce-sim".into())
        .spawn(move || sim_loop(loaded, snap_tx, ack_rx, cmd_rx, event_tx, shutdown, config, done_tx))
        .expect("spawn sim thread");

    let report = done_rx.await?; // fires after the final save is acked
    let _ = persist.await; // ends once the sim thread drops snap_tx
    let _ = sim.join();
    Ok(report)
}

/// Receives snapshots, writes them, and acks. A failed save sends `Failed` (not
/// nothing) so the sim thread never blocks forever waiting on the final save.
async fn persistence_task(
    store: SqliteStore,
    mut snap_rx: UnboundedReceiver<Snapshot>,
    ack_tx: Sender<Ack>,
) {
    while let Some(snap) = snap_rx.recv().await {
        match store.save(&snap).await {
            Ok(()) => {
                let _ = ack_tx.send(Ack::Saved(snap.deletes));
            }
            Err(e) => {
                tracing::error!(error = %e, "snapshot save failed; deletes retained");
                let _ = ack_tx.send(Ack::Failed);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sim_loop(
    loaded: Loaded,
    snap_tx: UnboundedSender<Snapshot>,
    ack_rx: Receiver<Ack>,
    cmd_rx: Receiver<Command>,
    event_tx: UnboundedSender<Outgoing>,
    shutdown: Arc<AtomicBool>,
    config: Config,
    done_tx: oneshot::Sender<RunReport>,
) {
    let mut world = World::new();
    if let Err(e) = world.load(&loaded.entities, loaded.next_id) {
        tracing::error!(error = %e, "failed to load world; starting empty");
    }
    // First boot against an empty database: lay down the starter map so there is
    // ground truth to play. A loaded world is left untouched.
    if loaded.entities.is_empty() {
        let seeded = musce_action::seed(&mut world);
        tracing::info!(start = ?seeded.start, avatar = ?seeded.avatar, "seeded starter world");
    }

    let mut dispatch = Dispatch::new();
    let mut tick: u64 = 0;
    let mut since_save: u32 = 0;
    let mut pending_saves: u32 = 0;
    let mut saves: u64 = 0;

    loop {
        // Apply any saves that completed since last tick.
        while let Ok(ack) = ack_rx.try_recv() {
            apply_ack(&mut world, ack, &mut pending_saves);
        }

        if shutdown.load(Ordering::Relaxed) {
            send_snapshot(&mut world, &snap_tx, &mut pending_saves, &mut saves);
            // Block until every outstanding save (including the final one) is
            // durable. This is the one place the sim waits on the DB.
            while pending_saves > 0 {
                match ack_rx.recv() {
                    Ok(ack) => apply_ack(&mut world, ack, &mut pending_saves),
                    Err(_) => break, // persistence gone; nothing more we can do
                }
            }
            break;
        }

        let start = Instant::now();
        let ctx = TickCtx {
            tick,
            now: SystemTime::now(),
        };

        // Drain the command inbox: the only entry point for external mutation.
        // The loop holds no command knowledge; it hands each command to the
        // dispatcher, which routes it to the right input-stack frame and emits
        // events (and, for in-game frames, runs `execute` against the world).
        while let Ok(cmd) = cmd_rx.try_recv() {
            dispatch.handle(cmd, &mut world, &mut |out| {
                if event_tx.send(out).is_err() {
                    tracing::error!("event outbox closed; dropping output");
                }
            });
        }

        run_phases(&mut world, &ctx);

        since_save += 1;
        if since_save >= config.save_every {
            send_snapshot(&mut world, &snap_tx, &mut pending_saves, &mut saves);
            since_save = 0;
        }

        tick += 1;

        if let Some(remaining) = config.tick_interval.checked_sub(start.elapsed()) {
            std::thread::sleep(remaining);
        }
    }

    let _ = done_tx.send(RunReport { ticks: tick, saves });
}

/// The phase pipeline. Empty for now; systems become ordered phases here, each
/// able to schedule by `ctx.tick` or `ctx.now`.
fn run_phases(_world: &mut World, _ctx: &TickCtx) {}

fn apply_ack(world: &mut World, ack: Ack, pending: &mut u32) {
    *pending = pending.saturating_sub(1);
    match ack {
        Ack::Saved(deletes) => world.confirm_saved(&deletes),
        Ack::Failed => tracing::warn!("save failed; deletes retained for retry"),
    }
}

fn send_snapshot(
    world: &mut World,
    snap_tx: &UnboundedSender<Snapshot>,
    pending: &mut u32,
    saves: &mut u64,
) {
    let snap = world.snapshot();
    if snap_tx.send(snap).is_ok() {
        *pending += 1;
        *saves += 1;
    } else {
        tracing::error!("persistence channel closed; snapshot dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Item, Room, World};

    #[tokio::test]
    async fn boot_tick_save_shutdown_lifecycle() {
        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();

        // Seed a world (hall containing an item) into the store.
        {
            let mut w = World::new();
            let mut b = EntityBuilder::new();
            b.add(Room);
            let hall = w.spawn(b);
            let mut b = EntityBuilder::new();
            b.add(Item);
            let thing = w.spawn(b);
            w.move_entity(thing, hall).unwrap();
            store.save(&w.snapshot()).await.unwrap();
        }

        let shutdown = Arc::new(AtomicBool::new(false));
        {
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(80)).await;
                shutdown.store(true, Ordering::Relaxed);
            });
        }

        let config = Config {
            tick_interval: Duration::from_millis(10),
            save_every: 2,
            listen_addr: None, // headless: no real socket in the lifecycle test
        };
        let report = run(store.clone(), config, shutdown).await.unwrap();

        // Shutdown always saves at least once, regardless of timing.
        assert!(report.saves >= 1);

        // The world survived boot-load + shutdown-save through the DB.
        let loaded = store.load().await.unwrap();
        assert_eq!(loaded.entities.len(), 2);
    }
}
