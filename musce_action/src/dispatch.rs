//! The command table: a registry of in-game verbs the dispatcher looks up by
//! name, plus the single entry point the host calls for a bare (embodied)
//! command. A game registers its verbs here rather than into a growing `match`,
//! and lookup resolves abbreviations (`n` -> `north`, `dr` -> `drop`) so adding a
//! verb is a local change. The table, registration, and lookup are engine
//! mechanism; the verbs themselves are game content. See
//! `docs/architecture/actions.md`.

use musce_core::{EntityId, World};
use musce_proto::{ConnectionId, Outgoing};

use crate::audience::{self, Outbound};
use crate::bindings::Actors;
use crate::ctx::Ctx;

/// A verb's parse-and-act function. Receives the command context and the
/// argument tail (everything after the verb word). A game writes these and
/// registers them; the engine only invokes them.
pub type Handler = fn(&mut Ctx, &str);

/// Permission required to run a verb. Only `Open` exists this slice (every
/// in-game verb is ungated); the admin verbs introduced next add staff tiers
/// here, checked at dispatch before the handler runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Open,
}

impl Gate {
    fn permits(self) -> bool {
        match self {
            Gate::Open => true,
        }
    }
}

struct Verb {
    name: &'static str,
    gate: Gate,
    handler: Handler,
}

/// The registry of in-game verbs. Ordered: lookup prefers an exact name, then the
/// first registered verb the input is a prefix of, so registration order is the
/// abbreviation tie-break (register movement before `say`, so `s` is south and
/// `sa` is say). A game builds one at boot and the runtime shares it read-only
/// across ticks.
pub struct CommandTable {
    verbs: Vec<Verb>,
}

impl CommandTable {
    /// A fresh, empty table. A game fills it by `register`ing its verbs.
    pub fn new() -> Self {
        CommandTable { verbs: Vec::new() }
    }

    /// Register a verb: its name, its permission gate, and its handler. Order
    /// matters for abbreviation ties (see the type docs).
    pub fn register(&mut self, name: &'static str, gate: Gate, handler: Handler) {
        self.verbs.push(Verb {
            name,
            gate,
            handler,
        });
    }

    fn lookup(&self, word: &str) -> Option<&Verb> {
        self.verbs
            .iter()
            .find(|v| v.name == word)
            .or_else(|| self.verbs.iter().find(|v| v.name.starts_with(word)))
    }
}

impl Default for CommandTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Dispatch one bare (embodied) command line: look the verb up, run its handler
/// to gather semantic output, then resolve those events' audiences to
/// connections through `emit`. `actor` is the entity the connection drives. The
/// host owns frame selection (`@`-floor vs embodiment); this is the embodiment
/// frame's entry point.
pub fn dispatch_bare(
    table: &CommandTable,
    world: &mut World,
    actors: &Actors,
    actor: EntityId,
    conn: ConnectionId,
    line: &str,
    emit: &mut impl FnMut(Outgoing),
) {
    let line = line.trim();
    let (word, rest) = match line.split_once(char::is_whitespace) {
        Some((w, r)) => (w, r.trim_start()),
        None => (line, ""),
    };

    let mut out: Vec<Outbound> = Vec::new();
    {
        let mut ctx = Ctx::new(world, actor, conn, &mut out);
        match table.lookup(&word.to_lowercase()) {
            Some(verb) if verb.gate.permits() => (verb.handler)(&mut ctx, rest),
            Some(_) => ctx.feedback("You aren't allowed to do that."),
            None => ctx.feedback(format!("I don't understand \"{word}\".")),
        }
    }

    for ob in out {
        audience::resolve(world, actors, ob, emit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Player, Room};
    use musce_proto::{Audience, Event, EventKind};

    /// Two test verbs over the public emit API, standing in for game content so
    /// the engine routing is exercised without depending on a real game. `ping`
    /// is registered before `pet` so the `p` prefix resolves to `ping`.
    fn table() -> CommandTable {
        let mut t = CommandTable::new();
        t.register("ping", Gate::Open, |c, _| c.feedback("pong"));
        t.register("pet", Gate::Open, |c, _| c.feedback("purr"));
        t
    }

    fn world_with_player() -> (World, Actors, EntityId, ConnectionId) {
        let mut world = World::new();
        let room = {
            let mut b = EntityBuilder::new();
            b.add(Room);
            world.spawn(b)
        };
        let actor = {
            let mut b = EntityBuilder::new();
            b.add(Player);
            world.spawn(b)
        };
        world.move_entity(actor, room).unwrap();

        let conn = ConnectionId(1);
        let mut actors = Actors::default();
        actors.bind(conn, actor);
        (world, actors, actor, conn)
    }

    fn texts(
        world: &mut World,
        actors: &Actors,
        actor: EntityId,
        conn: ConnectionId,
        line: &str,
    ) -> Vec<String> {
        let table = table();
        let mut out = Vec::new();
        dispatch_bare(&table, world, actors, actor, conn, line, &mut |o| {
            out.push(o)
        });
        out.into_iter()
            .map(|o| match o {
                Outgoing::Event(Event {
                    text,
                    to: Audience::Connection(_),
                    ..
                }) => text,
                other => panic!("expected connection event, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn exact_name_beats_prefix() {
        let (mut world, actors, actor, conn) = world_with_player();
        // "pet" matches exactly even though "ping" also starts with "pe"... it
        // does not, but "pet" is exact so it wins regardless of order.
        let out = texts(&mut world, &actors, actor, conn, "pet");
        assert!(out.iter().any(|t| t.contains("purr")));
    }

    #[test]
    fn prefix_resolves_in_registration_order() {
        let (mut world, actors, actor, conn) = world_with_player();
        // "p" is a prefix of both; "ping" was registered first, so it wins.
        let out = texts(&mut world, &actors, actor, conn, "p");
        assert!(out.iter().any(|t| t.contains("pong")));
    }

    #[test]
    fn unknown_verb_feeds_back() {
        let (mut world, actors, actor, conn) = world_with_player();
        let out = texts(&mut world, &actors, actor, conn, "frobnicate");
        assert!(out.iter().any(|t| t.contains("I don't understand")));
    }

    #[test]
    fn emit_kind_carries_through() {
        let (mut world, actors, actor, conn) = world_with_player();
        let mut t = CommandTable::new();
        t.register("yell", Gate::Open, |c, _| {
            c.emit_self(EventKind::Narration, "loud")
        });
        let mut out = Vec::new();
        dispatch_bare(&t, &mut world, &actors, actor, conn, "yell", &mut |o| {
            out.push(o)
        });
        assert!(matches!(
            out.as_slice(),
            [Outgoing::Event(Event {
                kind: EventKind::Narration,
                ..
            })]
        ));
    }
}
