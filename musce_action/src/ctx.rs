//! The handler context and its emit API: the engine surface a game's verb
//! handlers program against. `Ctx` carries the world a handler mutates, the actor
//! it acts through, the connection that issued the command, and the output buffer
//! it emits into. The emit methods address output semantically (first-person to
//! the actor, third-person to the room with the actor excluded); the dispatcher
//! resolves those audiences to connections afterward. See
//! `docs/architecture/actions.md`.

use musce_core::{EntityId, World};
use musce_proto::{ConnectionId, Event, EventKind};

use crate::audience::Outbound;

/// The per-command context handed to a handler: the world it mutates, the actor
/// it acts through, the connection that issued it, and the output buffer it emits
/// into. The actor is explicit so handlers are callable directly in tests and,
/// later, by AI and sequences.
pub struct Ctx<'a> {
    pub world: &'a mut World,
    pub actor: EntityId,
    pub conn: ConnectionId,
    out: &'a mut Vec<Outbound>,
}

impl<'a> Ctx<'a> {
    pub fn new(
        world: &'a mut World,
        actor: EntityId,
        conn: ConnectionId,
        out: &'a mut Vec<Outbound>,
    ) -> Self {
        Ctx {
            world,
            actor,
            conn,
            out,
        }
    }

    /// First-person output, straight to the acting connection.
    pub fn emit_self(&mut self, kind: EventKind, text: impl Into<String>) {
        self.out
            .push(Outbound::new(Event::to_connection(self.conn, kind, text)));
    }

    /// Plain feedback to the acting connection. The dispatcher uses this for
    /// parse-level replies (unknown verb, gated) before any handler runs.
    pub fn feedback(&mut self, text: impl Into<String>) {
        self.emit_self(EventKind::Feedback, text);
    }

    /// Third-person output to everyone in `room` except the actor, so the actor
    /// does not see both their own first-person line and the room's view of it.
    pub fn emit_room_except_self(
        &mut self,
        room: EntityId,
        kind: EventKind,
        text: impl Into<String>,
    ) {
        self.out.push(Outbound::excluding(
            Event::to_room(room, kind, text),
            self.conn,
        ));
    }
}
