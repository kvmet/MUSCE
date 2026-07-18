//! The transport-agnostic core. A transport turns its byte stream into a
//! `Connection` (a pair of line-oriented halves plus capabilities); everything
//! above this line is identical for TCP, WebSocket, or SSH. The sim never sees
//! any of these types: it speaks only `Command`/`Outgoing` from `boundary`.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use tokio::sync::mpsc;

use musce_proto::{Capabilities, Command, ConnectionId, Delivery, Input, Outgoing, ServerMsg};

/// A reader half: yields the next input already in the sim's `Input` vocabulary.
/// `None` means end of stream. Framing (a WebSocket frame boundary, a telnet
/// newline) and, for a structured transport, parsing the wire envelope are the
/// transport's concern; the layers above only ever see typed `Input`. A telnet
/// transport only ever yields `Input::Line`; a WebSocket transport also yields
/// `Input::Query` from its JSON envelope.
pub trait InputReader: Send + 'static {
    fn next_input(&mut self)
    -> impl std::future::Future<Output = io::Result<Option<Input>>> + Send;
}

/// A writer half: renders the sim's output to this transport's wire format. Two
/// kinds cross it: streamed events and structured replies. A telnet writer emits
/// bare text lines for events and drops replies (a line-mode client never asks for
/// one); a WebSocket writer emits a JSON envelope for both. Transport-specific
/// framing lives in the impl.
pub trait EventWriter: Send + 'static {
    fn write_event(
        &mut self,
        ev: &Delivery,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;

    fn write_reply(
        &mut self,
        reply: &ServerMsg,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;
}

/// A transport-agnostic established connection. A transport implements this to
/// expose its stream as independent read/write halves (so one task can read and
/// write without aliasing) plus the capabilities it advertises.
pub trait Connection: Send + 'static {
    type Reader: InputReader;
    type Writer: EventWriter;

    fn capabilities(&self) -> Capabilities;
    fn split(self) -> (Self::Reader, Self::Writer);
}

/// What the router pushes into a single connection's mailbox.
#[derive(Debug, Clone)]
pub enum ConnMsg {
    Event(Delivery),
    /// A structured reply to a read query. Only a structured transport can render
    /// it; a line-mode writer drops it.
    Reply(ServerMsg),
    /// Close after the already-queued messages ahead of this drain.
    Close,
}

/// `ConnectionId -> mailbox` for every live connection. The accept loop inserts,
/// the router looks up and removes, the per-connection task removes itself on
/// exit. Locks are held only for the map op, never across an await.
pub type Registry = Arc<Mutex<HashMap<ConnectionId, mpsc::UnboundedSender<ConnMsg>>>>;

/// Allocate an id for an established connection, register its mailbox, and spawn
/// its `serve_connection` task. Shared by every transport's accept loop: the id
/// comes from one process-wide counter (`ids`), so two transports running at once
/// never mint the same `ConnectionId` (which is the registry key). A transport
/// calls this once its handshake, if any, has produced a live `Connection`.
pub(crate) fn spawn_serve<C: Connection>(
    conn: C,
    peer: Option<SocketAddr>,
    inbox: Sender<Command>,
    registry: Registry,
    ids: &Arc<AtomicU64>,
) {
    let id = ConnectionId::next(ids);
    let (tx, rx) = mpsc::unbounded_channel();
    registry.lock().unwrap().insert(id, tx);
    tracing::info!(?id, ?peer, "connection opened");
    tokio::spawn(serve_connection(id, peer, conn, inbox, rx, registry));
}

/// Own one connection end to end: announce it, pump input up as `Command`s and
/// rendered events down to the wire, and tear it down. One `select!` loop over
/// the two independent halves, so a `Close` (or EOF) ends the task and drops the
/// socket cleanly. The mailbox `rx` is this connection's slot in the `Registry`.
pub async fn serve_connection<C: Connection>(
    id: ConnectionId,
    peer: Option<std::net::SocketAddr>,
    conn: C,
    inbox: Sender<Command>,
    mut rx: mpsc::UnboundedReceiver<ConnMsg>,
    registry: Registry,
) {
    let caps = conn.capabilities();
    let (mut reader, mut writer) = conn.split();

    // The sim allocates a session off this; it is the first thing it hears.
    let _ = inbox.send(Command {
        connection: id,
        input: Input::Connected { caps, peer },
    });

    loop {
        tokio::select! {
            input = reader.next_input() => match input {
                Ok(Some(input)) => {
                    if inbox.send(Command { connection: id, input }).is_err() {
                        break; // sim gone
                    }
                }
                Ok(None) => break,                 // client closed
                Err(e) => {
                    tracing::debug!(?id, error = %e, "read error; closing connection");
                    break;
                }
            },
            msg = rx.recv() => match msg {
                Some(ConnMsg::Event(ev)) => {
                    if let Err(e) = writer.write_event(&ev).await {
                        tracing::debug!(?id, error = %e, "write error; closing connection");
                        break;
                    }
                }
                Some(ConnMsg::Reply(reply)) => {
                    if let Err(e) = writer.write_reply(&reply).await {
                        tracing::debug!(?id, error = %e, "write error; closing connection");
                        break;
                    }
                }
                Some(ConnMsg::Close) | None => break, // sim asked to close, or router dropped us
            },
        }
    }

    registry.lock().unwrap().remove(&id);
    let _ = inbox.send(Command {
        connection: id,
        input: Input::Disconnected,
    });
}

/// Drain the sim's outbox and fan each message into the right connection mailbox.
/// Net is a pure pipe: a `Delivery` is already bound to a connection (the action
/// layer's audience resolver expanded `Entity`/`Locus` sim-side), so there is no
/// audience left to route on and an unresolved one cannot reach here by
/// construction.
pub async fn route_events(mut outbox: mpsc::UnboundedReceiver<Outgoing>, registry: Registry) {
    while let Some(out) = outbox.recv().await {
        match out {
            Outgoing::Event(ev) => send_to(&registry, ev.to, ConnMsg::Event(ev)),
            Outgoing::Reply(to, reply) => send_to(&registry, to, ConnMsg::Reply(reply)),
            Outgoing::Close(id) => {
                send_to(&registry, id, ConnMsg::Close);
                registry.lock().unwrap().remove(&id);
            }
        }
    }
}

/// Look up a mailbox (cloning the sender so the lock drops before sending) and
/// deliver. A missing id just means the connection already went away.
fn send_to(registry: &Registry, id: ConnectionId, msg: ConnMsg) {
    let tx = registry.lock().unwrap().get(&id).cloned();
    if let Some(tx) = tx {
        let _ = tx.send(msg);
    }
}
