//! The runtime: a single sim thread that owns the `World` and ticks at a fixed
//! cadence, with persistence on a tokio task. Commands in / events out (no-op
//! until networking lands). The world is loaded before the first tick and saved
//! synchronously on shutdown.

mod accounts;
mod dispatch;
mod session;

pub use accounts::{AccountView, LoginVeto};

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, Sender};
use musce_action::{CapRegistry, ColdOp, CommandTable, System};
use musce_auth::Account;
use musce_core::{EntityBlob, EntityId, Snapshot, World};
use musce_persistence::{AccountStore, KvStore, Loaded, Persistence, SCHEMA_VERSION, WorldStore};
use musce_proto::{Command, Delivery, EventKind, Outgoing};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::accounts::{AccountOp, AccountOutcome, OPERATOR_USERNAME, account_task};
use crate::dispatch::{Dispatch, HostOp};

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
    /// gates. Shared (`Arc`) so the off-thread account task resolves an account's
    /// grant names against the same registry the gates use, so a gate's id and a
    /// grant's id denote the same capability. Immutable once the game is built (all
    /// registration happens during construction). Empty for a game with no
    /// capability-gated verbs. See `docs/architecture/authorization.md`.
    pub caps: Arc<CapRegistry>,
    /// The app's login veto, run off-thread after a connection's account passes the
    /// engine's hard gate (`Disabled` refused): `Ok(())` admits, `Err(reason)`
    /// refuses with a shown reason. It can only further restrict, never lift a hard
    /// refusal. An app that does not gate logins uses `|_| Ok(())`. See
    /// `docs/architecture/authorization.md`.
    pub login_veto: LoginVeto,
    /// Turns a cold-store value's opaque bytes into the text delivered to a reader.
    /// Injected because decoding is game knowledge (the game encoded the bytes on
    /// write), so the engine's cold task never interprets a cold value: it calls
    /// this. `Ok` is the body to deliver, `Err` a line shown when the bytes will not
    /// decode. The reference game reads UTF-8; a game with a different cold encoding
    /// supplies its own. See `docs/architecture/persistence.md`.
    pub decode_cold: fn(&[u8]) -> Result<String, String>,
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
/// `World` is built and owned entirely by the sim thread. Both stores load here,
/// in the async context, before the sim thread exists; the sim receives plain
/// data and never holds a store.
pub async fn run(
    store: WorldStore,
    config: Config,
    shutdown: Arc<AtomicBool>,
    game: Game,
) -> Result<RunReport, Box<dyn std::error::Error + Send + Sync>> {
    store.init().await?;
    store.kv_init().await?; // the cold content table shares the world's store
    store.accounts_init().await?; // accounts share the world's store, own table
    let loaded = store.load().await?; // boot load, before any tick

    // Bootstrap: a store with no superuser gets one seeded operator, so a fresh
    // world is administrable at all. Passwordless and reachable only via the
    // loopback `@operator` stub. Checked after a successful load, so a store that
    // merely failed to read never mints an operator over accounts it could not see.
    if !store.any_superuser().await? {
        let mut operator = Account::new(OPERATOR_USERNAME);
        operator.set_su(true);
        store.account_upsert(&operator).await?;
        tracing::info!(username = OPERATOR_USERNAME, "seeded bootstrap operator");
    }

    let (snap_tx, snap_rx) = tokio::sync::mpsc::unbounded_channel::<Snapshot>();
    let (ack_tx, ack_rx) = crossbeam_channel::unbounded::<Ack>();
    let (done_tx, done_rx) = oneshot::channel::<Result<RunReport, String>>();

    // The net <-> sim boundary: commands in (crossbeam, drained by the sim
    // thread), events out (tokio mpsc, drained by the net router).
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<Command>();
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<Outgoing>();

    // The cold-store boundary: cold requests out (tokio mpsc, drained by the cold
    // task, which holds the store the sim never does). Its results ride the same
    // event outbox straight to the reader, so the sim never sees the completion.
    let (cold_tx, cold_rx) = tokio::sync::mpsc::unbounded_channel::<ColdOp>();
    let cold_event_tx = event_tx.clone();
    // A fn pointer, copied out before `game` moves into the sim thread.
    let decode_cold = game.decode_cold;

    // The account boundary: ops out to the off-thread account task, outcomes back to
    // the *sim* (unlike cold results, which go straight to the reader) because they
    // mutate sim-owned session state. Copied out before `game` moves into the sim.
    let (account_op_tx, account_op_rx) = tokio::sync::mpsc::unbounded_channel::<AccountOp>();
    let (account_result_tx, account_result_rx) = crossbeam_channel::unbounded::<AccountOutcome>();
    let account_caps = game.caps.clone();
    let login_veto = game.login_veto;

    if let Some(addr) = config.listen_addr {
        match musce_net::start(addr, cmd_tx, event_rx).await {
            Ok(bound) => tracing::info!(%bound, "listening"),
            Err(e) => tracing::error!(error = %e, "failed to start networking"),
        }
    }

    let persist = tokio::spawn(persistence_task(store.clone(), snap_rx, ack_tx));
    let cold = tokio::spawn(cold_task(
        store.clone(),
        cold_rx,
        cold_event_tx,
        decode_cold,
    ));
    let account = tokio::spawn(account_task(
        store.clone(),
        account_caps,
        login_veto,
        account_op_rx,
        account_result_tx,
    ));

    let sim = std::thread::Builder::new()
        .name("musce-sim".into())
        .spawn(move || {
            sim_loop(
                loaded,
                snap_tx,
                ack_rx,
                cmd_rx,
                event_tx,
                cold_tx,
                account_op_tx,
                account_result_rx,
                shutdown,
                config,
                game,
                done_tx,
            )
        })
        .expect("spawn sim thread");

    // Fires after the final save is acked, or with an error if the world failed
    // to load (the sim refuses to boot rather than run empty over real data).
    let report = done_rx
        .await?
        .map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
    let _ = persist.await; // ends once the sim thread drops snap_tx
    // The cold and account tasks end when the sim drops their op senders; awaiting
    // drains any buffered work (a final `inscribe`'s bytes, an in-flight account
    // upsert) to durability, the same reason the snapshot writer is awaited.
    let _ = cold.await;
    let _ = account.await;
    let _ = sim.join();
    Ok(report)
}

/// Receives snapshots, writes them, and acks. A failed save sends `Failed` (not
/// nothing) so the sim thread never blocks forever waiting on the final save.
async fn persistence_task(
    store: WorldStore,
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

/// Serves cold-store requests off the sim thread: this task is the only holder of
/// the store on the async side. A `Read` fetches the key, hands the bytes to the
/// game's `decode` (the engine interprets nothing), and delivers the result to the
/// reader's connection through the shared event outbox; a `Write` overwrites the
/// key and acks. Both send is best-effort: a closed connection just drops the line.
/// The task ends when the sim drops the request sender.
///
/// Ordering invariant: cold ops for the same key are applied in issue order, so a
/// read observes a preceding write (read-your-writes) and two writes cannot reorder
/// into a lost update. Today this is free: one task draining one channel is a total
/// order. If the cold path is ever parallelized for throughput (several workers, or a
/// store read-pool once `SqliteStore` leaves `max_connections(1)`), it must preserve
/// *per-key* order, e.g. route by `hash(key)` so one key stays on one worker while
/// distinct keys run concurrently. This is independent of world sharding: a parallel
/// cold path needs it even unsharded, and a zone-sharded world without a parallel
/// cold path does not.
async fn cold_task(
    store: WorldStore,
    mut cold_rx: UnboundedReceiver<ColdOp>,
    event_tx: UnboundedSender<Outgoing>,
    decode: fn(&[u8]) -> Result<String, String>,
) {
    while let Some(op) = cold_rx.recv().await {
        let delivery = match op {
            ColdOp::Read { key, conn, kind } => {
                let (kind, body) = match store.kv_get(&key).await {
                    // The value is opaque bytes; only the game knows how to read
                    // them, so decoding is the injected game fn, never done here.
                    Ok(Some(bytes)) => match decode(&bytes) {
                        Ok(text) => (kind, text),
                        Err(line) => (EventKind::Feedback, line),
                    },
                    Ok(None) => (
                        EventKind::Feedback,
                        "There is nothing written here.".to_string(),
                    ),
                    Err(e) => {
                        tracing::error!(%key, error = %e, "cold read failed");
                        (
                            EventKind::Feedback,
                            "The words swim and refuse to resolve.".to_string(),
                        )
                    }
                };
                Delivery::new(conn, kind, body)
            }
            ColdOp::Write { key, bytes, conn } => {
                let line = match store.kv_put(&key, &bytes).await {
                    Ok(()) => "You finish writing.",
                    Err(e) => {
                        tracing::error!(%key, error = %e, "cold write failed");
                        "The ink will not take."
                    }
                };
                Delivery::new(conn, EventKind::Feedback, line)
            }
        };
        let _ = event_tx.send(Outgoing::Event(delivery));
    }
}

#[allow(clippy::too_many_arguments)]
fn sim_loop(
    loaded: Loaded,
    snap_tx: UnboundedSender<Snapshot>,
    ack_rx: Receiver<Ack>,
    cmd_rx: Receiver<Command>,
    event_tx: UnboundedSender<Outgoing>,
    cold_tx: UnboundedSender<ColdOp>,
    account_op_tx: UnboundedSender<AccountOp>,
    account_result_rx: Receiver<AccountOutcome>,
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

    let mut dispatch = Dispatch::new(game);
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

        // Apply account outcomes from the off-thread task: each feeds a line back
        // and may bind or refresh a session. Applied before draining commands so a
        // login that completed last tick is in force for this tick's commands.
        while let Ok(outcome) = account_result_rx.try_recv() {
            dispatch.apply_account_outcome(outcome, &mut emit);
        }

        // Drain the command inbox: the only entry point for external mutation. The
        // loop holds no command knowledge; it hands each command to the dispatcher
        // and routes the host ops that come back: a cold read/write to the cold
        // task, an account op to the account task, exactly as snapshots flow to the
        // persistence task.
        while let Ok(cmd) = cmd_rx.try_recv() {
            for op in dispatch.handle(cmd, &mut world, &mut emit) {
                match op {
                    HostOp::Cold(c) => {
                        if cold_tx.send(c).is_err() {
                            tracing::error!("cold request channel closed; dropping request");
                        }
                    }
                    HostOp::Account(a) => {
                        if account_op_tx.send(a).is_err() {
                            tracing::error!("account op channel closed; dropping op");
                        }
                    }
                }
            }
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
fn migrate_blobs(from: u32, _entities: &mut Vec<EntityBlob>) {
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
            login_veto: |_| Ok(()),
            decode_cold: |_| Ok(String::new()),
        }
    }

    #[tokio::test]
    async fn boot_tick_save_shutdown_lifecycle() {
        let store = WorldStore::connect("sqlite::memory:").await.unwrap();
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

        let store = WorldStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();

        // Persist an otherwise-valid entity carrying a component tag no game
        // registers, so load fails on the unknown tag. A hand-built snapshot writes
        // it (the normal save path only emits known tags); save does not validate,
        // it just stores the rows. The `id` component keeps it structurally valid so
        // it is the *unknown tag*, not a missing Id, that refuses the boot.
        let mut data = Map::new();
        data.insert("id".into(), Value::from(1u64));
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
