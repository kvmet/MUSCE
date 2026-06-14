//! Networking for MUSCE: a transport-agnostic pipe between the outside world and
//! the single sim thread. Net is deliberately dumb. It turns a transport into
//! `Command { connection, input }` for the sim inbox and renders `Outgoing`
//! events back to connections. All routing that needs world state happens
//! sim-side; net holds only per-connection presentation state (capabilities,
//! later input mode). See `docs/architecture/networking-and-sessions.md`.

mod boundary;
mod connection;
mod tcp;

use std::net::SocketAddr;

use crossbeam_channel::Sender;
use tokio::sync::mpsc::UnboundedReceiver;

pub use boundary::{
    Audience, Capabilities, Command, ConnectionId, Event, EventKind, Input, Outgoing,
};
pub use connection::{Connection, LineReader, LineWriter, render};

/// Start networking: bind the TCP transport, spawn its accept loop and the event
/// router, and return the bound address. `inbox` is the sim's command channel
/// (net is the producer); `outbox` is the sim's event stream (net is the
/// consumer). Both run as detached tokio tasks for the lifetime of the runtime.
pub async fn start(
    addr: SocketAddr,
    inbox: Sender<Command>,
    outbox: UnboundedReceiver<Outgoing>,
) -> std::io::Result<SocketAddr> {
    let registry = connection::Registry::default();
    tokio::spawn(connection::route_events(outbox, registry.clone()));
    tcp::listen(addr, inbox, registry).await
}
