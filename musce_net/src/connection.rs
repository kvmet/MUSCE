//! The transport-agnostic core. A transport turns its byte stream into a
//! `Connection` (a pair of line-oriented halves plus capabilities); everything
//! above this line is identical for TCP, WebSocket, or SSH. The sim never sees
//! any of these types: it speaks only `Command`/`Outgoing` from `boundary`.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use tokio::sync::mpsc;

use musce_proto::{Capabilities, Command, ConnectionId, Event, Input, Outgoing};

/// A reader half: yields input one line at a time. `None` means end of stream.
/// The newline framing is the transport's concern (a WebSocket frame is already
/// a "line"); the layers above only ever see whole lines.
pub trait LineReader: Send + 'static {
    fn next_line(&mut self)
    -> impl std::future::Future<Output = io::Result<Option<String>>> + Send;
}

/// A writer half: renders are done above; this just puts bytes on the wire.
/// Transport-specific framing (WebSocket envelopes, telnet IAC) lives in the
/// impl, below this method.
pub trait LineWriter: Send + 'static {
    fn write_line(
        &mut self,
        line: &str,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;
}

/// A transport-agnostic established connection. A transport implements this to
/// expose its stream as independent read/write halves (so one task can read and
/// write without aliasing) plus the capabilities it advertises.
pub trait Connection: Send + 'static {
    type Reader: LineReader;
    type Writer: LineWriter;

    fn capabilities(&self) -> Capabilities;
    fn split(self) -> (Self::Reader, Self::Writer);
}

/// What the router pushes into a single connection's mailbox.
#[derive(Debug, Clone)]
pub enum ConnMsg {
    Event(Event),
    /// Close after the already-queued messages ahead of this drain.
    Close,
}

/// `ConnectionId -> mailbox` for every live connection. The accept loop inserts,
/// the router looks up and removes, the per-connection task removes itself on
/// exit. Locks are held only for the map op, never across an await.
pub type Registry = Arc<Mutex<HashMap<ConnectionId, mpsc::UnboundedSender<ConnMsg>>>>;

/// Render a semantic event to the wire format for a connection. Plain ANSI-less
/// text for now; `caps` (color, size) will shape this when richer rendering
/// lands. CRLF because line-mode clients expect it.
pub fn render(ev: &Event, _caps: &Capabilities) -> String {
    format!("{}\r\n", ev.text)
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
            line = reader.next_line() => match line {
                Ok(Some(line)) => {
                    if inbox.send(Command { connection: id, input: Input::Line(line) }).is_err() {
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
                    if let Err(e) = writer.write_line(&render(&ev, &caps)).await {
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
/// Net is a pure `Connection` pipe: the action layer's audience resolver expands
/// `Entity`/`Locus` into `Connection` events sim-side before they reach here, so a
/// non-connection audience at this point is a bug upstream, not normal traffic.
pub async fn route_events(mut outbox: mpsc::UnboundedReceiver<Outgoing>, registry: Registry) {
    use musce_proto::Audience;

    while let Some(out) = outbox.recv().await {
        match out {
            Outgoing::Event(ev) => match ev.to {
                Audience::Connection(id) => send_to(&registry, id, ConnMsg::Event(ev)),
                Audience::Entity(_) | Audience::Locus(_) => {
                    tracing::error!(audience = ?ev.to, "unresolved audience reached net; resolver should have expanded it");
                }
            },
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
