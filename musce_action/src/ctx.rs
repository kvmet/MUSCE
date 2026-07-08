//! The handler context and its emit API: the engine surface a game's verb
//! handlers program against. `Ctx` carries the world a handler mutates, the actor
//! it acts through, the connection that issued the command, and the output buffer
//! it emits into. The emit methods address output semantically (first-person to
//! the actor, third-person to the room with the actor excluded, or directed to a
//! specific entity); the dispatcher resolves those audiences to connections
//! afterward. See
//! `docs/architecture/actions.md`.

use std::time::SystemTime;

use musce_core::{EntityId, Fact, World};
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

    /// Directed output to a specific entity, resolved to the connection(s) driving
    /// it at output time. If the entity drives no connection it reaches no one, the
    /// same way narration to a room of NPCs does; the in-world act still happened.
    pub fn emit_entity(&mut self, target: EntityId, kind: EventKind, text: impl Into<String>) {
        self.out
            .push(Outbound::new(Event::to_entity(target, kind, text)));
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

/// A tick-loop system: the simulation-side analogue of a verb [`Handler`]. It
/// mutates the world and emits semantic output through a [`SystemCtx`], which the
/// runtime resolves to connections the same way it does a verb's. A game registers
/// these in its `Game.systems`; the engine only invokes them.
///
/// [`Handler`]: crate::Handler
pub type System = fn(&mut SystemCtx);

/// The per-tick context handed to a [`System`]. Mirrors [`Ctx`] for the
/// simulation half: the world a system mutates and the output buffer it emits
/// into, plus both clocks. There is no actor or connection, because a system acts
/// on the world's behalf, not a player's, so its output is third-person only.
///
/// Both clocks are carried even when a system uses only one: `tick` is
/// deterministic sim time (the default for game logic) and `now` is wall-clock
/// (for real-world scheduling). They come straight from the runtime's per-tick
/// context, captured once so every system in a tick sees the same instant.
///
/// `facts` is the tick's structural-fact batch: an observation stream of the
/// world mutations that have committed (destructions, and more as consumers need
/// them). A reaction system iterates it; a non-reactive system ignores it. The
/// slice borrows a buffer the runtime drained once before the system loop, so a
/// system never sees a fact another system in the same pass emitted.
pub struct SystemCtx<'a> {
    pub world: &'a mut World,
    pub tick: u64,
    pub now: SystemTime,
    pub facts: &'a [Fact],
    out: &'a mut Vec<Outbound>,
}

impl<'a> SystemCtx<'a> {
    pub fn new(
        world: &'a mut World,
        tick: u64,
        now: SystemTime,
        facts: &'a [Fact],
        out: &'a mut Vec<Outbound>,
    ) -> Self {
        SystemCtx {
            world,
            tick,
            now,
            facts,
            out,
        }
    }

    /// Third-person output to everyone in `room`. A system has no first person, so
    /// unlike [`Ctx::emit_room_except_self`] there is no actor to exclude.
    pub fn emit_room(&mut self, room: EntityId, kind: EventKind, text: impl Into<String>) {
        self.out
            .push(Outbound::new(Event::to_room(room, kind, text)));
    }
}
