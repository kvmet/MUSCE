//! Ground truth for the dumb pipe: a real TCP client in, a rendered line out.

use std::time::Duration;

use musce_net::{Command, Event, EventKind, Input, Outgoing, start};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// A line sent by a client surfaces as `Connected` then `Line`, and an event
/// addressed to that connection is rendered back to the same client.
#[tokio::test]
async fn line_in_event_out() {
    let (inbox_tx, inbox_rx) = crossbeam_channel::unbounded::<Command>();
    let (outbox_tx, outbox_rx) = tokio::sync::mpsc::unbounded_channel::<Outgoing>();

    let addr = start("127.0.0.1:0".parse().unwrap(), inbox_tx, outbox_rx)
        .await
        .unwrap();

    let mut client = TcpStream::connect(addr).await.unwrap();
    client.write_all(b"hello\n").await.unwrap();

    // crossbeam recv is blocking; bounce it off a blocking task so the runtime
    // stays free.
    let recv = |rx: crossbeam_channel::Receiver<Command>| async move {
        tokio::task::spawn_blocking(move || (rx.recv_timeout(Duration::from_secs(2)).unwrap(), rx))
            .await
            .unwrap()
    };

    let (first, inbox_rx) = recv(inbox_rx).await;
    assert!(matches!(first.input, Input::Connected { .. }));
    let id = first.connection;

    let (second, _inbox_rx) = recv(inbox_rx).await;
    assert_eq!(second.connection, id);
    assert!(matches!(second.input, Input::Line(ref s) if s == "hello"));

    // Event out -> rendered line at the client.
    outbox_tx
        .send(Outgoing::Event(Event::to_connection(
            id,
            EventKind::Feedback,
            "hi there",
        )))
        .unwrap();

    let mut reader = BufReader::new(client);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(line, "hi there\r\n");
}

/// `Outgoing::Close` drops the connection: the client sees EOF and the sim hears
/// `Disconnected`.
#[tokio::test]
async fn close_drops_connection() {
    let (inbox_tx, inbox_rx) = crossbeam_channel::unbounded::<Command>();
    let (outbox_tx, outbox_rx) = tokio::sync::mpsc::unbounded_channel::<Outgoing>();

    let addr = start("127.0.0.1:0".parse().unwrap(), inbox_tx, outbox_rx)
        .await
        .unwrap();

    let client = TcpStream::connect(addr).await.unwrap();

    let recv = |rx: crossbeam_channel::Receiver<Command>| async move {
        tokio::task::spawn_blocking(move || (rx.recv_timeout(Duration::from_secs(2)).unwrap(), rx))
            .await
            .unwrap()
    };

    let (connected, inbox_rx) = recv(inbox_rx).await;
    let id = connected.connection;

    outbox_tx.send(Outgoing::Close(id)).unwrap();

    // Client hits EOF.
    let mut reader = BufReader::new(client);
    let mut line = String::new();
    let n = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(n, 0);

    let (disc, _inbox_rx) = recv(inbox_rx).await;
    assert_eq!(disc.connection, id);
    assert!(matches!(disc.input, Input::Disconnected));
}
