//! End-to-end ground truth for the action slice: a real TCP client drives a real
//! sim through the full path (transport -> dispatcher -> verb handlers -> audience
//! resolver -> rendered output). connect -> @play -> look -> go north -> take, with
//! the reference game (`musce_ref`) driven through the real engine runtime.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use musce_host::{Config, run};
use musce_persistence::SqliteStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

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
    let store = SqliteStore::connect("sqlite::memory:").await.unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000, // keep saves out of the way for this test
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
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

/// `pilot` redirects bare commands to the controlled puppet, and `release` brings
/// them back to the character. Observed over the wire: after piloting the seeded
/// drone, `go north` moves the *drone* (the arrival look shows the garden), while
/// the character never left the hall, so a `release` + `look` lands back in the
/// hall.
#[tokio::test]
async fn pilot_redirects_bare_commands_then_release_returns() {
    let addr = free_port().await;
    let store = SqliteStore::connect("sqlite::memory:").await.unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let config = Config {
        tick_interval: Duration::from_millis(10),
        save_every: 10_000,
        listen_addr: Some(addr),
    };
    let handle = tokio::spawn(run(
        store.clone(),
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
