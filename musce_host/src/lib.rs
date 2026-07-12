//! The runtime: a single sim thread that owns the `World` and ticks at a fixed
//! cadence, with persistence on a tokio task. Commands in / events out (no-op
//! until networking lands). The world is loaded before the first tick and saved
//! synchronously on shutdown.

/// The account authority lives in its own leaf crate (`musce_auth`); re-exported
/// under the path it grew up at so a game keeps addressing `musce_host::auth`.
pub use musce_auth as auth;

mod dispatch;
mod session;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, Sender};
use musce_action::{CommandTable, System};
use musce_core::{EntityBlob, EntityId, Snapshot, World};
use musce_persistence::{Loaded, Persistence, SCHEMA_VERSION, SqliteStore};
use musce_proto::{Command, Outgoing};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::auth::{Accounts, CapRegistry, MemoryAccountStore};
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

/// Builds the starting world when the database loads empty; a loaded world is
/// left untouched.
pub type Seed = fn(&mut World);

/// The `@play` policy: which actor a connection comes to drive. Pure selection;
/// the floor records the attachment as session state. Returns `None` if the game
/// has no character to give.
pub type ChooseActor = fn(&World) -> Option<EntityId>;

/// World-type registration the runtime runs against a fresh `World` before it
/// loads or seeds, so a game's own component types are known to the deserializer
/// (registration must precede deserialization) and persist thereafter. Engine
/// components register themselves in `World::new`; this is where a game adds its.
pub type Register = fn(&mut World);

/// The whole of what the runtime needs from a game: its bare and admin verb
/// registries, its world seed, and its `@play` actor-choice policy. A plain struct
/// of values plus fn pointers; the runtime never depends on a particular game,
/// only on this. The account floor (`@quit`/`@who`/`@help`) stays engine; only
/// `@play`'s choice of actor is game policy, which is why `choose_actor` is the
/// one floor concern the game injects. `CommandTable`, `Gate`, and dispatch are
/// engine mechanism; the game owns which verbs each table holds and their prose.
/// See `docs/architecture/engine-and-game.md`.
pub struct Game {
    /// Bare in-game verbs, driven through the embodiment frame.
    pub commands: CommandTable,
    /// `@`-namespace admin/builder verbs, capability-gated, driven through the admin
    /// frame. Empty for a game with no builder surface.
    pub admin: CommandTable,
    pub seed: Seed,
    pub choose_actor: ChooseActor,
    /// Tick-loop systems, run in order every tick through the phase pipeline. A
    /// `Vec` so the runtime runs N by construction; empty for a game with no
    /// simulation.
    pub systems: Vec<System>,
    /// Registers the game's own component/relation types on a fresh world, before
    /// load or seed. The runtime calls this so a wanderer (or any game type)
    /// deserializes and persists. No-op for a game that adds no types.
    pub register: Register,
    /// The game's capability vocabulary, interned to `CapId`s while it wired its
    /// gates. Shared (`Arc`) so the account authority can hold the same registry it
    /// resolves account grant strings against, so a gate's id and a grant's id denote
    /// the same capability. Immutable once the game is built (all registration happens
    /// during construction). Empty for a game with no capability-gated verbs. See
    /// `docs/architecture/authorization.md`.
    pub caps: Arc<CapRegistry>,
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
    game: Game,
) -> Result<RunReport, Box<dyn std::error::Error + Send + Sync>> {
    store.init().await?;
    let loaded = store.load().await?; // boot load, before any tick

    let (snap_tx, snap_rx) = tokio::sync::mpsc::unbounded_channel::<Snapshot>();
    let (ack_tx, ack_rx) = crossbeam_channel::unbounded::<Ack>();
    let (done_tx, done_rx) = oneshot::channel::<Result<RunReport, String>>();

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
        .spawn(move || {
            sim_loop(
                loaded, snap_tx, ack_rx, cmd_rx, event_tx, shutdown, config, game, done_tx,
            )
        })
        .expect("spawn sim thread");

    // Fires after the final save is acked, or with an error if the world failed
    // to load (the sim refuses to boot rather than run empty over real data).
    let report = done_rx
        .await?
        .map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
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
    game: Game,
    done_tx: oneshot::Sender<Result<RunReport, String>>,
) {
    let mut world = World::new();
    // The game's own component types must be registered before the deserializer
    // can read them, so this runs before load (and before seed).
    (game.register)(&mut world);

    // Bring persisted blobs up to the current schema before deserializing them.
    let mut entities = loaded.entities;
    migrate_blobs(loaded.schema_version, &mut entities);

    if let Err(e) = world.load(&entities, loaded.next_id) {
        // Blobs the current schema cannot read (a removed or renamed component
        // with no migration). Refuse to boot rather than run an empty world and
        // reissue ids from 1, which the next save would write over the persisted
        // entities. Failing here leaves the stored world untouched.
        let msg = format!("failed to load persisted world: {e}; refusing to boot");
        tracing::error!(error = %e, "failed to load persisted world; refusing to boot");
        let _ = done_tx.send(Err(msg));
        return;
    }
    // First boot against an empty database: lay down the game's starter world so
    // there is ground truth to play. A loaded world is left untouched.
    if entities.is_empty() {
        (game.seed)(&mut world);
        tracing::info!("seeded starter world");
    }

    // Bring up the account authority, resolving grants against the game's caps
    // registry. An empty store bootstraps one su operator; a populated store with no
    // su, or an unknown grant, refuses to boot rather than run mis-authorized. Slice 1
    // stands up a trivial in-memory backend; a durable one lands with authentication.
    let account_store = MemoryAccountStore::new();
    let accounts = match Accounts::boot(&account_store, game.caps.clone()) {
        Ok(accounts) => accounts,
        Err(e) => {
            let msg = format!("account authority refused to boot: {e}");
            tracing::error!(error = %e, "account authority refused to boot");
            let _ = done_tx.send(Err(msg));
            return;
        }
    };

    let mut dispatch = Dispatch::new(game, accounts);
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

        // One emit sink for the whole tick, shared by command dispatch and the
        // system pipeline: both route semantic output to the same outbox.
        let mut emit = |out| {
            if event_tx.send(out).is_err() {
                tracing::error!("event outbox closed; dropping output");
            }
        };

        // Drain the command inbox: the only entry point for external mutation.
        // The loop holds no command knowledge; it hands each command to the
        // dispatcher, which routes it to the right input-stack frame and emits
        // events (and, for in-game frames, runs `execute` against the world).
        while let Ok(cmd) = cmd_rx.try_recv() {
            dispatch.handle(cmd, &mut world, &mut emit);
        }

        dispatch.run_systems(&mut world, &ctx, &mut emit);

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

    let _ = done_tx.send(Ok(RunReport { ticks: tick, saves }));
}

/// Bring loaded blobs from their stored schema version up to the current one, in
/// place, before they are deserialized. This is the seam where schema evolution
/// lives: renaming or reshaping a persisted component means bumping
/// `SCHEMA_VERSION` and adding a transform here, keyed by the version it migrates
/// from. No transforms exist yet (the schema has only ever been at version 1), so
/// a current-version world is a no-op; an older version with no transform falls
/// through unchanged and the subsequent load fails loudly rather than silently
/// dropping data. See `docs/architecture/persistence.md`.
fn migrate_blobs(from: u32, _entities: &mut [EntityBlob]) {
    if from == SCHEMA_VERSION {
        return;
    }
    tracing::warn!(
        from,
        to = SCHEMA_VERSION,
        "no schema migration defined for this version; loading blobs unchanged"
    );
}

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
    use musce_core::{Description, Locus, World};

    /// An engine-only `Game`: no verbs, a no-op seed, a `choose_actor` that picks
    /// nothing, no systems, and a no-op `register`. The runtime, not game content,
    /// is what this crate tests, so the lifecycle test needs only a `Game`-shaped
    /// value to hand `run`.
    fn test_game() -> Game {
        Game {
            commands: CommandTable::new(),
            admin: CommandTable::new(),
            seed: |_| {},
            choose_actor: |_| None,
            systems: vec![],
            register: |_| {},
            caps: Arc::new(CapRegistry::new()),
        }
    }

    #[tokio::test]
    async fn boot_tick_save_shutdown_lifecycle() {
        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();

        // Seed a world (hall containing a thing) into the store.
        {
            let mut w = World::new();
            let mut b = EntityBuilder::new();
            b.add(Locus);
            let hall = w.spawn(b);
            let mut b = EntityBuilder::new();
            b.add(Description("a thing".into()));
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
        let report = run(store.clone(), config, shutdown, test_game())
            .await
            .unwrap();

        // Shutdown always saves at least once, regardless of timing.
        assert!(report.saves >= 1);

        // The world survived boot-load + shutdown-save through the DB.
        let loaded = store.load().await.unwrap();
        assert_eq!(loaded.entities.len(), 2);
    }

    /// A persisted world the current schema cannot read makes the runtime refuse
    /// to boot (returns an error) rather than run empty and reissue ids that the
    /// next save would write over the stored entities. The stored world is left
    /// intact.
    #[tokio::test]
    async fn refuses_to_boot_on_an_unreadable_world() {
        use musce_core::{EntityBlob, EntityId, Map, Value};

        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();

        // Persist an entity carrying a component tag no game registers, so load
        // fails. A hand-built snapshot writes it (the normal save path only emits
        // known tags); save does not validate, it just stores the blob.
        let mut data = Map::new();
        data.insert("nonexistent_component".into(), Value::Null);
        let snap = Snapshot {
            entities: vec![EntityBlob {
                id: EntityId(1),
                zone: None,
                data: Value::Object(data),
            }],
            deletes: vec![],
            next_id: 2,
        };
        store.save(&snap).await.unwrap();

        let shutdown = Arc::new(AtomicBool::new(false));
        let config = Config {
            tick_interval: Duration::from_millis(10),
            save_every: 1000,
            listen_addr: None,
        };
        let result = run(store.clone(), config, shutdown, test_game()).await;
        assert!(
            result.is_err(),
            "boot should fail on an unreadable world, got: {result:?}"
        );

        // Refused before any save, so the stored entity is untouched.
        assert_eq!(store.load().await.unwrap().entities.len(), 1);
    }
}
