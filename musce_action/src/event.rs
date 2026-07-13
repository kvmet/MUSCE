//! The semantic, world-addressed output vocabulary handlers author against. An
//! `Event` names *what* to say and *who* should perceive it (an `Audience`); the
//! resolver turns that into per-connection `musce_proto::Delivery`s. This lives in
//! the action layer, not `musce_proto`, because `Audience::Entity`/`Locus` address
//! world entities and never cross to net: they are resolved to connections here,
//! before output leaves the sim.
//!
//! `text` is a plain string today. The intended direction is genuinely *semantic*
//! output (structured content a client renders its own way, for accessibility and
//! for rich vs plain clients), which is why the type is kept separate from the
//! wire form rather than being pre-rendered ANSI. Introducing that structure is
//! deferred until a second renderer exists to define it: content flows through
//! resolution unchanged and every construction site is funneled through these
//! constructors and `Ctx`'s emit API, so `text: String` -> a structured body is a
//! cheap, funneled change when a concrete consumer arrives, not a migration.

use musce_core::EntityId;
use musce_proto::EventKind;

/// A semantic, addressed piece of output. Kept semantic (not pre-rendered) so a
/// richer client can render it its own way later.
#[derive(Debug, Clone)]
pub struct Event {
    pub to: Audience,
    pub kind: EventKind,
    pub text: String,
}

impl Event {
    /// Convenience for the common case: text aimed at one connection.
    pub fn to_connection(
        id: musce_proto::ConnectionId,
        kind: EventKind,
        text: impl Into<String>,
    ) -> Self {
        Event {
            to: Audience::Connection(id),
            kind,
            text: text.into(),
        }
    }

    /// Text aimed at everyone directly within a locus (a scope boundary; the
    /// reference game's rooms are loci). The audience resolver expands this into
    /// per-connection deliveries; net never sees it.
    pub fn to_locus(locus: EntityId, kind: EventKind, text: impl Into<String>) -> Self {
        Event {
            to: Audience::Locus(locus),
            kind,
            text: text.into(),
        }
    }

    /// Text aimed at one entity, resolved to the connection(s) driving it. The
    /// resolver expands this like `to_locus`; if the entity drives no connection it
    /// reaches no one. Net never sees it.
    pub fn to_entity(entity: EntityId, kind: EventKind, text: impl Into<String>) -> Self {
        Event {
            to: Audience::Entity(entity),
            kind,
            text: text.into(),
        }
    }
}

/// Who an event is for. `Entity`/`Locus` are resolved to `Connection`s by the
/// audience resolver (it needs world state and the connection-to-entity map)
/// before output reaches net, which only ever routes an already-resolved
/// `Delivery`. A `Locus` is a scope boundary: the event reaches every connection
/// whose actor stands directly within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    Connection(musce_proto::ConnectionId),
    Entity(EntityId),
    Locus(EntityId),
}
