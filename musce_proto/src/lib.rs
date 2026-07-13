//! The wire vocabulary: the types that actually cross the net <-> sim thread
//! boundary. Commands in (`Command`/`Input`), events out (`Outgoing`, whose
//! `Delivery` payload is already resolved to a single `ConnectionId`), plus the
//! per-connection presentation state net owns (`Capabilities`). Transport-free (no
//! tokio) and dependency-free: `musce_net` and `musce_host` speak it, and it
//! references no world identity, because by the time output reaches this layer the
//! audience has been resolved to a connection. The semantic, world-addressed
//! authoring vocabulary (`Event`/`Audience`) lives in `musce_action`, which owns
//! resolution; net never sees it.
//!
//! These types are **ephemeral**: they ride an in-process channel and are never
//! persisted, so nothing here derives `serde`. A connection is a live socket, not a
//! saved record; if that ever changes it is a deliberate decision, not a reflex.
//!
//! See `docs/architecture/networking-and-sessions.md` and
//! `docs/architecture/actions.md`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};

/// Net-local identity for one live connection. Monotonic and never reused, so a
/// stale reference can never resolve to a different connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(pub u64);

impl ConnectionId {
    /// Allocate the next id from a shared counter.
    pub fn next(counter: &AtomicU64) -> Self {
        ConnectionId(counter.fetch_add(1, Ordering::Relaxed))
    }
}

/// A terminal's size in character cells, as advertised by the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

/// Per-connection presentation state net holds locally because it owns framing.
/// The sim reads it (handed up on connect) and later updates it via outbound
/// directives; it never lives in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Client can render ANSI color.
    pub color: bool,
    /// Client can only do line-mode (no char/raw keystroke mode). A dumb TCP
    /// client is line-only; SSH/WebSocket will report `false` here.
    pub line_mode_only: bool,
    /// Terminal size, if known.
    pub size: Option<TerminalSize>,
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
    Event(Delivery),
    /// Drop a connection (e.g. after `@quit`). Net flushes any already-queued
    /// content for it first, then closes the socket.
    Close(ConnectionId),
}

/// A fully-resolved event bound for one connection: what actually crosses to net.
/// Audience resolution (and the session floor's direct-to-connection emits)
/// produce these, so an unresolved `Entity`/`Locus` audience can never reach net,
/// it is unrepresentable here. Kept semantic (not pre-rendered) so a richer client
/// can render `text` its own way later; net turns it into wire bytes.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub to: ConnectionId,
    pub kind: EventKind,
    pub text: String,
}

impl Delivery {
    pub fn new(to: ConnectionId, kind: EventKind, text: impl Into<String>) -> Self {
        Delivery {
            to,
            kind,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// Server-originated notice (connect banner, shutdown warning).
    System,
    /// Direct response to a command.
    Feedback,
    /// World description (room look, arrivals, things others do).
    Narration,
}
