//! The shared protocol vocabulary: the types that cross the net <-> sim thread
//! boundary (commands in, events out) plus the audience/event model the action
//! layer addresses output with. Pure and transport-free (no tokio): `musce_net`,
//! `musce_action`, and `musce_host` all speak it, so the action layer never
//! depends on the transport. See `docs/architecture/actions.md` and
//! `docs/architecture/networking-and-sessions.md`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};

use musce_core::EntityId;
use serde::{Deserialize, Serialize};

/// Net-local identity for one live connection. Monotonic and never reused, so a
/// stale reference can never resolve to a different connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConnectionId(pub u64);

impl ConnectionId {
    /// Allocate the next id from a shared counter.
    pub fn next(counter: &AtomicU64) -> Self {
        ConnectionId(counter.fetch_add(1, Ordering::Relaxed))
    }
}

/// Per-connection presentation state net holds locally because it owns framing.
/// The sim reads it (handed up on connect) and later updates it via outbound
/// directives; it never lives in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Client can render ANSI color.
    pub color: bool,
    /// Client can only do line-mode (no char/raw keystroke mode). A dumb TCP
    /// client is line-only; SSH/WebSocket will report `false` here.
    pub line_mode_only: bool,
    /// Terminal size in (cols, rows), if known.
    pub size: Option<(u16, u16)>,
}

/// A request from a connection to the sim. Lifecycle (`Connected`/`Disconnected`)
/// rides the same channel as input so the sim has a single entry point for
/// allocating, driving, and tearing down a session.
#[derive(Debug, Clone)]
pub struct Command {
    pub connection: ConnectionId,
    pub input: Input,
}

#[derive(Debug, Clone)]
pub enum Input {
    /// Net opened a connection; carries the advertised capabilities.
    Connected {
        caps: Capabilities,
        peer: Option<SocketAddr>,
    },
    /// One line of input (the trailing newline already stripped).
    Line(String),
    /// Net lost the connection (client closed, or net closed it after `Close`).
    Disconnected,
}

/// What the sim sends toward connections: content to render, or a presentation
/// directive net executes locally. (Input-mode switching and modal hints will
/// extend this when those land.)
#[derive(Debug, Clone)]
pub enum Outgoing {
    Event(Event),
    /// Drop a connection (e.g. after `@quit`). Net flushes any already-queued
    /// content for it first, then closes the socket.
    Close(ConnectionId),
}

/// A semantic, addressed piece of output. Kept semantic (not pre-rendered) so a
/// richer client can render it its own way later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub to: Audience,
    pub kind: EventKind,
    pub text: String,
}

impl Event {
    /// Convenience for the common case: text aimed at one connection.
    pub fn to_connection(id: ConnectionId, kind: EventKind, text: impl Into<String>) -> Self {
        Event {
            to: Audience::Connection(id),
            kind,
            text: text.into(),
        }
    }

    /// Text aimed at everyone in a room. The sim-side audience resolver expands
    /// this into per-connection events; net never sees it.
    pub fn to_room(room: EntityId, kind: EventKind, text: impl Into<String>) -> Self {
        Event {
            to: Audience::Room(room),
            kind,
            text: text.into(),
        }
    }
}

/// Who an event is for. `Entity`/`Room` are resolved to `Connection` sim-side by
/// the action layer's audience resolver (it needs world state and the
/// connection-to-entity map); net only ever routes `Connection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Audience {
    Connection(ConnectionId),
    Entity(EntityId),
    Room(EntityId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    /// Server-originated notice (connect banner, shutdown warning).
    System,
    /// Direct response to a command.
    Feedback,
    /// World description (room look, arrivals, things others do).
    Narration,
}
