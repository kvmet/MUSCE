//! Networking for MUSCE: a transport-agnostic pipe between the outside world and
//! the single sim thread. Net is deliberately dumb. It turns a transport into
//! `Command { connection, input }` for the sim inbox and renders `Outgoing`
//! events back to connections. All routing that needs world state happens
//! sim-side; net holds only per-connection presentation state (capabilities,
//! later input mode). See `docs/architecture/networking-and-sessions.md`.

mod connection;
mod tcp;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use crossbeam_channel::Sender;
use musce_proto::{Command, Outgoing};
use tokio::sync::mpsc::UnboundedReceiver;

pub use connection::{Connection, EventWriter, InputReader};

/// The addresses the transports actually bound to. A caller that binds to port 0
/// reads the real port back here (tests do this); `None` for a transport that was
/// not requested.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bound {
    pub tcp: Option<SocketAddr>,
    pub ws: Option<SocketAddr>,
}

/// Start networking: spawn the event router, then bind each requested transport
/// (raw TCP line-mode and/or WebSocket) behind one shared connection registry and
/// id counter, so ids never collide across transports. `inbox` is the sim's
/// command channel (net is the producer); `outbox` is the sim's event stream (net
/// is the consumer). All tasks run detached for the lifetime of the runtime.
pub async fn start(
    tcp_addr: Option<SocketAddr>,
    ws_addr: Option<SocketAddr>,
    inbox: Sender<Command>,
    outbox: UnboundedReceiver<Outgoing>,
) -> std::io::Result<Bound> {
    let registry = connection::Registry::default();
    let ids = Arc::new(AtomicU64::new(0));
    tokio::spawn(connection::route_events(outbox, registry.clone()));

    let mut bound = Bound::default();
    if let Some(addr) = tcp_addr {
        bound.tcp = Some(tcp::listen(addr, inbox.clone(), registry.clone(), ids.clone()).await?);
    }
    if let Some(addr) = ws_addr {
        bound.ws = Some(ws::listen(addr, inbox, registry, ids).await?);
    }
    Ok(bound)
}
