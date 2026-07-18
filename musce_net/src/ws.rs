//! WebSocket transport: the first-class transport for the web client. Each
//! WebSocket text frame is one "line", so this maps cleanly onto the
//! transport-agnostic `Connection` line pipe; the client chooses framing. The
//! accept loop runs the HTTP upgrade handshake per connection, then hands the
//! upgraded socket to the same `spawn_serve`/`serve_connection` path every
//! transport shares.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use crossbeam_channel::Sender;
use futures_util::sink::SinkExt;
use futures_util::stream::{SplitSink, SplitStream, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use musce_proto::{Capabilities, ClientMsg, Command, Delivery, Input, ServerMsg};

use crate::connection::{Connection, EventWriter, InputReader, Registry, spawn_serve};

type Ws = WebSocketStream<TcpStream>;

/// The read half: parses each inbound text frame as the client JSON envelope and
/// hands up the typed [`Input`]. A frame that will not parse is skipped with a
/// warning rather than dropping the connection (one malformed message is a client
/// bug, not a reason to end the session). Non-text frames are ignored; a close
/// frame or a dropped stream ends input. tokio-tungstenite answers pings itself as
/// the stream is polled, so keep-alive needs nothing here.
pub struct WsReader(SplitStream<Ws>);

impl InputReader for WsReader {
    async fn next_input(&mut self) -> io::Result<Option<Input>> {
        loop {
            match self.0.next().await {
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<ClientMsg>(t.as_str()) {
                    Ok(msg) => return Ok(Some(msg.into_input())),
                    Err(e) => {
                        tracing::warn!(error = %e, "unparseable client frame; skipping");
                        continue;
                    }
                },
                Some(Ok(Message::Close(_))) | None => return Ok(None),
                Some(Ok(_)) => continue, // binary / ping / pong / other: ignore
                Some(Err(e)) => return Err(io::Error::other(e)),
            }
        }
    }
}

/// The write half: each outgoing message goes out as one WebSocket text frame
/// carrying the JSON server envelope. An event is wrapped as `ServerMsg::Event`; a
/// reply is already a `ServerMsg`.
pub struct WsWriter(SplitSink<Ws, Message>);

impl WsWriter {
    async fn send_json(&mut self, msg: &ServerMsg) -> io::Result<()> {
        let text = serde_json::to_string(msg).map_err(io::Error::other)?;
        self.0
            .send(Message::text(text))
            .await
            .map_err(io::Error::other)
    }
}

impl EventWriter for WsWriter {
    async fn write_event(&mut self, ev: &Delivery) -> io::Result<()> {
        self.send_json(&ServerMsg::Event {
            kind: ev.kind,
            text: ev.text.clone(),
        })
        .await
    }

    async fn write_reply(&mut self, reply: &ServerMsg) -> io::Result<()> {
        self.send_json(reply).await
    }
}

/// One upgraded WebSocket connection.
pub struct WsConnection(Ws);

impl Connection for WsConnection {
    type Reader = WsReader;
    type Writer = WsWriter;

    fn capabilities(&self) -> Capabilities {
        // A WebSocket client can do char-mode (it chooses framing); color and
        // size are unknown until it advertises them.
        Capabilities {
            color: false,
            line_mode_only: false,
            size: None,
        }
    }

    fn split(self) -> (Self::Reader, Self::Writer) {
        let (sink, stream) = self.0.split();
        (WsReader(stream), WsWriter(sink))
    }
}

/// Bind and run the accept loop. The upgrade handshake runs inside each
/// connection's own task so a slow client cannot stall the accept loop; only a
/// successful upgrade mints an id (from the shared counter) and joins the
/// registry. Returns the bound address. The loop runs until the task is dropped.
pub async fn listen(
    addr: SocketAddr,
    inbox: Sender<Command>,
    registry: Registry,
    ids: Arc<AtomicU64>,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;

    tokio::spawn(async move {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(error = %e, "ws accept failed");
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let inbox = inbox.clone();
            let registry = registry.clone();
            let ids = ids.clone();
            tokio::spawn(async move {
                match tokio_tungstenite::accept_async(stream).await {
                    Ok(ws) => spawn_serve(WsConnection(ws), Some(peer), inbox, registry, &ids),
                    Err(e) => tracing::debug!(%peer, error = %e, "ws handshake failed"),
                }
            });
        }
    });

    Ok(bound)
}
