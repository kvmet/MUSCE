//! End-to-end ground truth for the action slice: a real TCP client drives a real
//! sim through the full path (transport -> dispatcher -> verb handlers -> audience
//! resolver -> rendered output). connect -> @play -> look -> go north -> take, with
//! the reference game (`musce_ref`) driven through the real engine runtime.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use musce_host::{Config, run};
use musce_persistence::WorldStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// A fresh in-memory account store, one per test sim. `run` initializes its
/// schema; an empty store boots into the operator bootstrap.
async fn accounts_db() -> musce_host::auth::AccountBackend {
    musce_host::auth::AccountBackend::connect("sqlite::memory:")
        .await
        .unwrap()
}

/// Grab a currently-free loopback port by binding and immediately dropping a
/// listener. `run` does not surface its bound address, so the test picks the port
/// and hands it in. The brief gap before the sim rebinds is the standard
/// pick-a-free-port race and is harmless here.
async fn free_port() -> std::net::SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap()
}

/// Connect to the sim's listener, retrying briefly to absorb the gap between the
/// free-port probe and the sim rebinding it.
async fn connect(addr: std::net::SocketAddr) -> (OwnedReadHalf, OwnedWriteHalf) {
    let mut attempt = None;
    for _ in 0..50 {
        match TcpStream::connect(addr).await {
            Ok(s) => {
                attempt = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    attempt
        .expect("connect to the sim's TCP listener")
        .into_split()
}

/// Extract the first `#<id>` from server output (the id a creation verb reports).
fn first_id(s: &str) -> u64 {
    let after = s.split('#').nth(1).expect("an #id in the output");
    after
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .expect("digits after #")
}

/// Write one command line (newline-terminated) and flush it.
async fn send(writer: &mut OwnedWriteHalf, line: &str) {
    writer
        .write_all(format!("{line}\n").as_bytes())
        .await
        .unwrap();
    writer.flush().await.unwrap();
}

/// Collect whatever the server sends until a read gap (the response burst ends)
/// or EOF. The server renders one event as one write that may carry embedded
/// newlines (a multi-line room look), so we accumulate raw bytes rather than
/// parse lines.
async fn read_burst(reader: &mut OwnedReadHalf) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        match tokio::time::timeout(Duration::from_millis(300), reader.read(&mut chunk)).await {
            Ok(Ok(0)) => break, // EOF
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
            Ok(Err(_)) => break,
            Err(_) => break, // gap: burst over
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test]
async fn connect_play_look_go_take() {
    let addr = free_port().await;

    // Empty in-memory DB: the sim seeds the starter world on first boot. `run`
    // initializes the schema itself.
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000, // keep saves out of the way for this test
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;

    // Banner on connect.
    let welcome = read_burst(&mut reader).await;
    assert!(
        welcome.contains("Welcome to MUSCE"),
        "welcome banner, got: {welcome:?}"
    );

    send(&mut writer, "@play").await;
    let played = read_burst(&mut reader).await;
    assert!(
        played.contains("You are now"),
        "@play confirmation, got: {played:?}"
    );

    send(&mut writer, "look").await;
    let looked = read_burst(&mut reader).await;
    assert!(
        looked.contains("stone hall"),
        "look shows the start room, got: {looked:?}"
    );
    assert!(
        looked.contains("north"),
        "look lists the north exit, got: {looked:?}"
    );

    send(&mut writer, "go north").await;
    let moved = read_burst(&mut reader).await;
    assert!(
        moved.contains("walled garden"),
        "arrival auto-look shows the garden, got: {moved:?}"
    );

    send(&mut writer, "take key").await;
    let took = read_burst(&mut reader).await;
    assert!(
        took.contains("You take a brass key"),
        "take feedback, got: {took:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// The cold content store, end to end: a seeded journal ships unwritten, so a first
/// `read` reads blank; `inscribe` writes its text cold (`kv_put`), and a second
/// `read` fetches it back (`kv_get`) and the game decodes it for delivery. Exercises
/// the whole async cold path (verb -> cold channel -> cold task -> store -> event
/// outbox) over the wire.
#[tokio::test]
async fn inscribe_then_read_a_book_round_trips_cold_storage() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;

    send(&mut writer, "@play").await;
    let _played = read_burst(&mut reader).await;

    // The seeded journal has no text yet: a read finds the key absent.
    send(&mut writer, "read journal").await;
    let blank = read_burst(&mut reader).await;
    assert!(
        blank.contains("nothing written"),
        "an unwritten book reads blank, got: {blank:?}"
    );

    // Write the text cold, then read it back through the store.
    send(
        &mut writer,
        "inscribe journal The moon was full that night.",
    )
    .await;
    let wrote = read_burst(&mut reader).await;
    assert!(
        wrote.contains("You finish writing"),
        "inscribe acks the write, got: {wrote:?}"
    );

    send(&mut writer, "read journal").await;
    let text = read_burst(&mut reader).await;
    assert!(
        text.contains("The moon was full that night."),
        "read returns the inscribed text, got: {text:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// `pilot` redirects bare commands to the controlled puppet, and `release` brings
/// them back to the character. Observed over the wire: after piloting the seeded
/// drone, `go north` moves the *drone* (the arrival look shows the garden), while
/// the character never left the hall, so a `release` + `look` lands back in the
/// hall.
#[tokio::test]
async fn pilot_redirects_bare_commands_then_release_returns() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;

    send(&mut writer, "@play").await;
    let _played = read_burst(&mut reader).await;

    // Before piloting, bare commands drive the character, in the hall.
    send(&mut writer, "look").await;
    let looked = read_burst(&mut reader).await;
    assert!(
        looked.contains("stone hall"),
        "self look shows the hall, got: {looked:?}"
    );

    send(&mut writer, "pilot drone").await;
    let piloted = read_burst(&mut reader).await;
    assert!(
        piloted.contains("take control"),
        "pilot confirmation, got: {piloted:?}"
    );

    // Now bare commands drive the drone: moving north shows the drone's arrival.
    send(&mut writer, "go north").await;
    let moved = read_burst(&mut reader).await;
    assert!(
        moved.contains("walled garden"),
        "the drone moved north, got: {moved:?}"
    );

    // The character itself never left the hall: release returns control to it.
    send(&mut writer, "release").await;
    let released = read_burst(&mut reader).await;
    assert!(
        released.contains("return to yourself"),
        "release confirmation, got: {released:?}"
    );

    send(&mut writer, "look").await;
    let back = read_burst(&mut reader).await;
    assert!(
        back.contains("stone hall"),
        "back to self in the hall, got: {back:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// `@possess` establishes control over a thing created at runtime, which `pilot`
/// can then aim at. Observed over the wire: `@create` a goblin, `@possess` it by
/// the reported id, `pilot` it, then a bare `go north` moves the *goblin* (the
/// arrival look shows the garden), proving the runtime-possession path.
#[tokio::test]
async fn possess_then_pilot_drives_a_created_thing() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;
    send(&mut writer, "@operator").await;
    let _op = read_burst(&mut reader).await;
    send(&mut writer, "@play").await;
    let _played = read_burst(&mut reader).await;

    // Create a goblin in the hall and capture the id the verb reports.
    send(&mut writer, "@create goblin").await;
    let created = read_burst(&mut reader).await;
    let goblin = first_id(&created);

    // Possess it by that id, then pilot it.
    send(&mut writer, &format!("@possess #{goblin}")).await;
    let possessed = read_burst(&mut reader).await;
    assert!(
        possessed.contains("possess"),
        "@possess confirmation, got: {possessed:?}"
    );

    send(&mut writer, "pilot goblin").await;
    let piloted = read_burst(&mut reader).await;
    assert!(
        piloted.contains("take control"),
        "pilot confirmation, got: {piloted:?}"
    );

    // Bare commands now drive the goblin: moving north shows its arrival.
    send(&mut writer, "go north").await;
    let moved = read_burst(&mut reader).await;
    assert!(
        moved.contains("walled garden"),
        "the possessed goblin moved north, got: {moved:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// A real system on the tick pipeline, end to end with zero player input: a
/// `@create`d rat wanders out of the hall on its own, and the narration reaches a
/// connection that sent no command. The tick interval is set so a wander step
/// (`WANDER_EVERY` ticks) lands well outside `read_burst`'s 300ms gap, so each
/// step is its own burst rather than one unbroken stream; the bounded poll mirrors
/// `connect`'s retry rather than racing a single wall-clock read.
#[tokio::test]
async fn a_wandering_creature_moves_with_no_input() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(100),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;
    send(&mut writer, "@operator").await;
    let _op = read_burst(&mut reader).await;
    send(&mut writer, "@play").await;
    let _played = read_burst(&mut reader).await;

    // Create a rat in the hall (where the avatar stands), then send nothing more.
    send(&mut writer, "@create rat").await;
    let _created = read_burst(&mut reader).await;

    // Bounded-poll for the rat's autonomous step; it wanders on its own.
    let mut saw_wander = false;
    for _ in 0..20 {
        if read_burst(&mut reader).await.contains("rat wanders") {
            saw_wander = true;
            break;
        }
    }
    assert!(
        saw_wander,
        "the rat should wander out of the hall with no command driving it"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// A game system reacting to a structural fact, end to end: `@create` a goblin in
/// the actor's room, `@destroy` it, and the `death_cry` system narrates its demise
/// in the SAME response burst as the destroy feedback. The destroy is command-
/// driven, so the fact is drained and reacted to on the same tick (no wall-clock
/// racing, unlike the wander e2e which deliberately spaces steps across bursts).
#[tokio::test]
async fn destroying_a_thing_cries_out_in_the_room() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;
    send(&mut writer, "@operator").await;
    let _op = read_burst(&mut reader).await;
    send(&mut writer, "@play").await;
    let _played = read_burst(&mut reader).await;

    // Create a goblin in the hall (where the avatar stands), capturing its id.
    send(&mut writer, "@create goblin").await;
    let created = read_burst(&mut reader).await;
    let goblin = first_id(&created);

    // Destroy it; the cry rides the same burst as the destroy feedback.
    send(&mut writer, &format!("@destroy #{goblin}")).await;
    let destroyed = read_burst(&mut reader).await;
    assert!(
        destroyed.contains("Destroyed"),
        "@destroy feedback, got: {destroyed:?}"
    );
    assert!(
        destroyed.contains("goblin crumbles to dust"),
        "death cry in the same burst, got: {destroyed:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// The admin frame end to end: the connection elevates to the operator account via
/// the loopback `@operator` stub, so `@create`/`@set`/`@dig` reach the admin table
/// (su bypasses their capability gates) and mutate the world. Verified by chaining on
/// the id `@create` reports and reading the result back through a bare `look`.
#[tokio::test]
async fn admin_verbs_build_the_world() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;
    send(&mut writer, "@operator").await;
    let _op = read_burst(&mut reader).await;
    send(&mut writer, "@play").await;
    let _played = read_burst(&mut reader).await;

    // Create a thing; the verb reports its new id so we can reference it.
    send(&mut writer, "@create torch").await;
    let created = read_burst(&mut reader).await;
    assert!(
        created.contains("Created") && created.contains('#'),
        "@create reports the new id, got: {created:?}"
    );
    let torch = first_id(&created);

    // Reference it by that id: retune its description (whole-component @set).
    send(
        &mut writer,
        &format!("@set #{torch}.description \"a magic torch\""),
    )
    .await;
    let setted = read_burst(&mut reader).await;
    assert!(
        setted.contains("Set description"),
        "@set confirmation, got: {setted:?}"
    );

    // Dig a new room in a free direction (the hall already has north/down).
    send(&mut writer, "@dig east a hidden vault").await;
    let dug = read_burst(&mut reader).await;
    assert!(dug.contains("Dug east"), "@dig confirmation, got: {dug:?}");

    // A bare look proves all three landed in the world: the new east exit and the
    // created-then-renamed torch are both in the room.
    send(&mut writer, "look").await;
    let looked = read_burst(&mut reader).await;
    assert!(
        looked.contains("east"),
        "look shows the dug exit, got: {looked:?}"
    );
    assert!(
        looked.contains("a magic torch"),
        "look shows the created, renamed torch, got: {looked:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// A seeded sequence drives an entity end to end with zero player input: the
/// clockwork sentry patrols hall <-> garden on its own, and its movement narration
/// reaches a connection that sent only `@play`. The patrol repeats, so a bounded
/// poll catches a pass through the avatar's room whenever the test attaches.
/// Mirrors the wander e2e (a system on the tick pipeline, observed over the wire),
/// but driven by a `Sequences` component rather than a bespoke system.
#[tokio::test]
async fn a_patrolling_sentry_moves_with_no_input() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;
    send(&mut writer, "@play").await;

    // Accumulate everything after @play; the sentry's narration arrives on its own
    // schedule, whenever the patrol next passes through the avatar's room.
    let mut seen = String::new();
    let mut saw = false;
    for _ in 0..30 {
        seen.push_str(&read_burst(&mut reader).await);
        if seen.contains("clockwork sentry") {
            saw = true;
            break;
        }
    }
    assert!(
        saw,
        "the sentry should patrol with no command driving it, got: {seen:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// A finite sequence ending in self-destruction, converging with the gate-2
/// reaction channel: the seeded torch's terminal beat destroys it, and the
/// `death_cry` system narrates the demise one tick later, reaching the avatar in
/// the same room with no narration code in the sequence layer at all. Observed over
/// the wire: after `@play`, the burn-out cry appears with no command driving it.
#[tokio::test]
async fn a_torch_burns_out_and_cries_in_the_room() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;
    send(&mut writer, "@play").await;

    let mut seen = String::new();
    let mut saw = false;
    for _ in 0..40 {
        seen.push_str(&read_burst(&mut reader).await);
        if seen.contains("guttering torch crumbles to dust") {
            saw = true;
            break;
        }
    }
    assert!(
        saw,
        "the torch should burn out and its death cry reach the room, got: {seen:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// Combat end to end: descend to the cellar's giant rat, land a non-lethal blow
/// (its Health drops but it lives), then a second blow that kills it. The killing
/// blow's own feedback and the `death_cry` reaction (`Fact::Destroyed` -> "crumbles
/// to dust") both reach the client in the same tick's burst.
#[tokio::test]
async fn attack_wears_a_foe_down_then_kills_it() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;
    send(&mut writer, "@play").await;
    let _played = read_burst(&mut reader).await;

    // Down to the cellar, where the rat stands.
    send(&mut writer, "go down").await;
    let arrived = read_burst(&mut reader).await;
    assert!(
        arrived.contains("cellar") && arrived.contains("giant rat"),
        "arrival shows the cellar and the rat, got: {arrived:?}"
    );

    // First blow: Strength 5 off 8 HP, so the rat survives.
    send(&mut writer, "attack rat").await;
    let hit = read_burst(&mut reader).await;
    assert!(
        hit.contains("You hit a giant rat for 5 damage"),
        "the first blow lands but does not kill, got: {hit:?}"
    );

    // Second blow empties its Health (via the `kill` alias): the kill line and the
    // death cry arrive together, the reaction converging on the same fact channel
    // gate 2 built.
    send(&mut writer, "kill rat").await;
    let kill = read_burst(&mut reader).await;
    assert!(
        kill.contains("You strike a giant rat down"),
        "the killing blow's feedback, got: {kill:?}"
    );
    assert!(
        kill.contains("giant rat crumbles to dust"),
        "the death cry reacts to the kill in the same burst, got: {kill:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// Containers end to end: take the seeded coin off the hall floor, stash it in the
/// wooden chest with `put`, then `examine` the chest to see the coin listed inside.
/// This exercises the container-contents view that keeps `put` from being a black
/// hole, rendered over the wire.
#[tokio::test]
async fn put_a_coin_in_the_chest_then_see_it_inside() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;
    send(&mut writer, "@play").await;
    let _played = read_burst(&mut reader).await;

    // The hall shows both the chest and the loose coin.
    send(&mut writer, "look").await;
    let looked = read_burst(&mut reader).await;
    assert!(
        looked.contains("wooden chest") && looked.contains("copper coin"),
        "the hall holds the chest and the coin, got: {looked:?}"
    );

    send(&mut writer, "take coin").await;
    let took = read_burst(&mut reader).await;
    assert!(
        took.contains("You take a copper coin"),
        "take feedback, got: {took:?}"
    );

    send(&mut writer, "put coin in chest").await;
    let stashed = read_burst(&mut reader).await;
    assert!(
        stashed.contains("You put a copper coin in a wooden chest"),
        "put feedback, got: {stashed:?}"
    );

    // Examining the chest now lists the coin inside it.
    send(&mut writer, "examine chest").await;
    let inside = read_burst(&mut reader).await;
    assert!(
        inside.contains("It contains: a copper coin"),
        "the chest reveals its contents, got: {inside:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// `give` end to end: hand the seeded coin to the patrol drone (a `Creature`, a
/// valid recipient) standing in the hall, exercising the three-party emit through
/// real audience resolution over the wire.
#[tokio::test]
async fn give_a_coin_to_the_drone() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;
    send(&mut writer, "@play").await;
    let _played = read_burst(&mut reader).await;

    send(&mut writer, "take coin").await;
    let took = read_burst(&mut reader).await;
    assert!(
        took.contains("You take a copper coin"),
        "take feedback, got: {took:?}"
    );

    send(&mut writer, "give coin to drone").await;
    let gave = read_burst(&mut reader).await;
    assert!(
        gave.contains("You give a copper coin to a patrol drone"),
        "give feedback, got: {gave:?}"
    );

    // The coin left the actor's hands, so it is no longer in inventory.
    send(&mut writer, "inventory").await;
    let inv = read_burst(&mut reader).await;
    assert!(
        !inv.contains("copper coin"),
        "the coin is no longer carried, got: {inv:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// Authorization end to end: the operator mints a builder account, grants it the
/// `build` capability, and logs in as it; the granted `@create` works, `@quell`
/// sets aside the quellable build cap so `@create` is refused, and un-quelling
/// restores it. This is the falsifying flow the authorization model could not
/// exercise until a non-su account with a granted cap and a login existed.
#[tokio::test]
async fn a_granted_builder_creates_until_quelled() {
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
        accounts_db().await,
        config,
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;

    // As the operator, mint a builder account and grant it the build cap.
    send(&mut writer, "@operator").await;
    let _ = read_burst(&mut reader).await;
    send(&mut writer, "@account new builder").await;
    let acct = read_burst(&mut reader).await;
    assert!(
        acct.contains("Created account \"builder\""),
        "account creation feedback, got: {acct:?}"
    );
    send(&mut writer, "@grant builder build").await;
    let granted = read_burst(&mut reader).await;
    assert!(
        granted.contains("Granted \"build\" to builder"),
        "grant feedback, got: {granted:?}"
    );

    // Log in as the builder (non-su) and take a body.
    send(&mut writer, "@login builder").await;
    let logged = read_burst(&mut reader).await;
    assert!(
        logged.contains("logged in as builder"),
        "login feedback, got: {logged:?}"
    );
    send(&mut writer, "@play").await;
    let _ = read_burst(&mut reader).await;

    // The build cap admits @create. Create an inert torch, not a `rat`: a rat
    // carries `Wander` and would flood the hall with movement narration every few
    // ticks, so `read_burst` would never see its inter-burst gap.
    send(&mut writer, "@create torch").await;
    let built = read_burst(&mut reader).await;
    assert!(
        built.contains("Created"),
        "the granted builder can create, got: {built:?}"
    );

    // Quell sets aside the quellable build cap: @create is now refused.
    send(&mut writer, "@quell").await;
    let _ = read_burst(&mut reader).await;
    send(&mut writer, "@create torch").await;
    let refused = read_burst(&mut reader).await;
    assert!(
        refused.contains("aren't allowed"),
        "quelled, the build cap is set aside, got: {refused:?}"
    );

    // Un-quell restores it.
    send(&mut writer, "@quell").await;
    let _ = read_burst(&mut reader).await;
    send(&mut writer, "@create torch").await;
    let again = read_burst(&mut reader).await;
    assert!(
        again.contains("Created"),
        "un-quelled, the build cap is back, got: {again:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}

/// Durability end to end: accounts survive a full sim restart. The first sim's
/// operator mints a builder and grants it `build`; the same account store then
/// feeds a second sim, where the builder logs straight in and the granted cap
/// still admits `@create`. That one flow falsifies the whole durable path: the
/// per-mutation writer persisted the bootstrap operator, the account, and the
/// grant; the shutdown drain flushed them; and the second boot loaded them
/// rather than re-bootstrapping (a re-bootstrap would hold only the operator,
/// so `@login builder` would fail).
#[tokio::test]
async fn accounts_survive_a_restart() {
    // One account store across both sims; the shared pool keeps the in-memory
    // database alive between them. Each sim gets its own throwaway world DB.
    let accounts = accounts_db().await;
    let config = |addr| Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };

    // Sim 1: mint the builder and grant it build.
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let handle = tokio::spawn(run(
        store,
        accounts.clone(),
        config(addr),
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;
    send(&mut writer, "@operator").await;
    let _ = read_burst(&mut reader).await;
    send(&mut writer, "@account new builder").await;
    let _ = read_burst(&mut reader).await;
    send(&mut writer, "@grant builder build").await;
    let granted = read_burst(&mut reader).await;
    assert!(
        granted.contains("Granted \"build\" to builder"),
        "grant feedback, got: {granted:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();

    // Sim 2: same account store, fresh world. The builder and its grant must
    // already be there.
    let addr = free_port().await;
    let store = WorldStore::connect("sqlite::memory:").await.unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let handle = tokio::spawn(run(
        store,
        accounts.clone(),
        config(addr),
        shutdown.clone(),
        musce_ref::game(),
    ));

    let (mut reader, mut writer) = connect(addr).await;
    let _welcome = read_burst(&mut reader).await;
    send(&mut writer, "@login builder").await;
    let logged = read_burst(&mut reader).await;
    assert!(
        logged.contains("logged in as builder"),
        "the account survived the restart, got: {logged:?}"
    );
    send(&mut writer, "@play").await;
    let _ = read_burst(&mut reader).await;
    // An inert torch, not a rat: a `Wander` kind would starve read_burst's gap.
    send(&mut writer, "@create torch").await;
    let built = read_burst(&mut reader).await;
    assert!(
        built.contains("Created"),
        "the grant survived the restart, got: {built:?}"
    );

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.await.unwrap();
}
