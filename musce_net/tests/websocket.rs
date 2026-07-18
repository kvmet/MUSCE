//! The WebSocket transport speaks the JSON envelope both ways: a client text
//! command (`{"t":"line",...}`) and a read query (`{"t":"query",...}`) arrive as
//! typed `Input` on the sim inbox, and a sim `Delivery`/`Reply` arrives as one
//! `{"t":"event"|"snapshot",...}` frame. Connect and disconnect ride the same
//! channel as `Input::Connected`/`Disconnected`. This exercises the real
//! `start` -> accept -> handshake -> serve -> codec path against a tungstenite
//! client, so it is the falsifying test for the wire contract end to end.

use std::time::Duration;

use crossbeam_channel::Receiver;
use futures_util::{SinkExt, StreamExt};
use musce_proto::{
    Command, Delivery, EventKind, Input, Offer, OfferStatus, Outgoing, Query, ServerMsg,
};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

/// Pull the next command off the sim inbox without blocking the async runtime:
/// the crossbeam recv is synchronous, so it runs on the blocking pool while the
/// runtime keeps driving the accept/serve tasks that produce it.
async fn next_cmd(rx: &Receiver<Command>) -> Command {
    let rx = rx.clone();
    tokio::task::spawn_blocking(move || {
        rx.recv_timeout(Duration::from_secs(2))
            .expect("a command within 2s")
    })
    .await
    .unwrap()
}

/// The next inbound text frame, parsed as JSON.
async fn next_json(
    client: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
) -> Value {
    let msg = client.next().await.expect("a frame").expect("no ws error");
    serde_json::from_str(msg.into_text().unwrap().as_str()).expect("a JSON frame")
}

#[tokio::test]
async fn websocket_speaks_the_json_envelope_both_ways() {
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<Command>();
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<Outgoing>();

    let bound = musce_net::start(None, Some("127.0.0.1:0".parse().unwrap()), cmd_tx, event_rx)
        .await
        .expect("start networking");
    assert!(bound.tcp.is_none(), "no TCP transport was requested");
    let ws_addr = bound.ws.expect("a bound ws address");

    // Connect over an explicit TCP stream: `client_async` needs only the
    // handshake feature the server already uses, avoiding the `connect` feature.
    let tcp = tokio::net::TcpStream::connect(ws_addr).await.unwrap();
    let url = format!("ws://{ws_addr}/");
    let (mut client, _resp) = tokio_tungstenite::client_async(url.as_str(), tcp)
        .await
        .expect("ws handshake");

    // The sim hears the connection first, and learns the id it must reply to.
    let connected = next_cmd(&cmd_rx).await;
    assert!(matches!(connected.input, Input::Connected { .. }));
    let conn = connected.connection;

    // A text command envelope arrives as an `Input::Line`.
    client
        .send(Message::text(r#"{"t":"line","line":"look"}"#))
        .await
        .unwrap();
    let line = next_cmd(&cmd_rx).await;
    assert_eq!(line.connection, conn);
    assert!(matches!(line.input, Input::Line(l) if l == "look"));

    // A query envelope arrives as a typed `Input::Query`.
    client
        .send(Message::text(r#"{"t":"query","q":"offers","clicked":42}"#))
        .await
        .unwrap();
    let query = next_cmd(&cmd_rx).await;
    assert!(matches!(
        query.input,
        Input::Query(Query::Offers { clicked: 42 })
    ));

    // A sim event arrives as one `{"t":"event",...}` frame.
    event_tx
        .send(Outgoing::Event(Delivery::new(
            conn,
            EventKind::Narration,
            "You see a room.",
        )))
        .unwrap();
    let ev = next_json(&mut client).await;
    assert_eq!(ev["t"], "event");
    assert_eq!(ev["kind"], "narration");
    assert_eq!(ev["text"], "You see a room.");

    // A sim reply arrives as one `{"t":"offers",...}` frame with the status shape
    // the client renders on.
    event_tx
        .send(Outgoing::Reply(
            conn,
            ServerMsg::Offers {
                clicked: 42,
                offers: vec![Offer {
                    name: "take".into(),
                    status: OfferStatus::Available,
                }],
            },
        ))
        .unwrap();
    let reply = next_json(&mut client).await;
    assert_eq!(reply["t"], "offers");
    assert_eq!(reply["clicked"], 42);
    assert_eq!(reply["offers"][0]["name"], "take");
    assert_eq!(reply["offers"][0]["status"]["kind"], "available");

    // Closing the client surfaces as a disconnect on the same connection.
    client.close(None).await.unwrap();
    let disconnected = next_cmd(&cmd_rx).await;
    assert_eq!(disconnected.connection, conn);
    assert!(matches!(disconnected.input, Input::Disconnected));
}
