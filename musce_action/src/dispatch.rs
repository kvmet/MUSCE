//! The command table: a registry of in-game verbs the dispatcher looks up by
//! name, plus the single entry point the host calls for a bare (embodied)
//! command. Verbs register here rather than into a growing `match`, and lookup
//! resolves abbreviations (`n` -> `north`, `dr` -> `drop`) so adding a verb is a
//! local change. See `docs/architecture/actions.md`.

use musce_core::{EntityId, World};
use musce_proto::{ConnectionId, Outgoing};

use crate::audience::{self, Outbound};
use crate::bindings::Actors;
use crate::verbs::{self, Ctx};

/// A verb's parse-and-act function. Receives the command context and the
/// argument tail (everything after the verb word).
type Handler = fn(&mut Ctx, &str);

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
/// abbreviation tie-break (movement before `say`, so `s` is south and `sa` is
/// say). Built once and shared read-only across ticks.
pub struct CommandTable {
    verbs: Vec<Verb>,
}

impl CommandTable {
    fn empty() -> Self {
        CommandTable { verbs: Vec::new() }
    }

    fn register(&mut self, name: &'static str, gate: Gate, handler: Handler) {
        self.verbs.push(Verb { name, gate, handler });
    }

    fn lookup(&self, word: &str) -> Option<&Verb> {
        self.verbs
            .iter()
            .find(|v| v.name == word)
            .or_else(|| self.verbs.iter().find(|v| v.name.starts_with(word)))
    }
}

impl Default for CommandTable {
    /// The MVP verb set. Movement is registered first so single-letter direction
    /// abbreviations win their prefix ties.
    fn default() -> Self {
        let mut t = CommandTable::empty();
        t.register("north", Gate::Open, |c, _| verbs::go(c, "north"));
        t.register("south", Gate::Open, |c, _| verbs::go(c, "south"));
        t.register("east", Gate::Open, |c, _| verbs::go(c, "east"));
        t.register("west", Gate::Open, |c, _| verbs::go(c, "west"));
        t.register("up", Gate::Open, |c, _| verbs::go(c, "up"));
        t.register("down", Gate::Open, |c, _| verbs::go(c, "down"));
        t.register("look", Gate::Open, verbs::look);
        t.register("go", Gate::Open, verbs::go);
        t.register("take", Gate::Open, verbs::take);
        t.register("drop", Gate::Open, verbs::drop);
        t.register("say", Gate::Open, verbs::say);
        t
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
    use musce_core::{Description, Exit, Exits, Player, Room};
    use musce_proto::{Audience, Event};

    fn world_with_player() -> (World, Actors, EntityId, ConnectionId) {
        let mut world = World::new();

        let hall = {
            let mut b = EntityBuilder::new();
            b.add(Room);
            b.add(Description("a stone hall".into()));
            world.spawn(b)
        };
        let garden = {
            let mut b = EntityBuilder::new();
            b.add(Room);
            b.add(Description("a quiet garden".into()));
            world.spawn(b)
        };
        // hall --north--> garden
        let he = world.index().get(hall).unwrap();
        world
            .ecs
            .insert_one(he, Exits(vec![Exit { direction: "north".into(), to: garden }]))
            .unwrap();

        let actor = {
            let mut b = EntityBuilder::new();
            b.add(Player);
            b.add(Description("an adventurer".into()));
            world.spawn(b)
        };
        world.move_entity(actor, hall).unwrap();

        let conn = ConnectionId(1);
        let mut actors = Actors::default();
        actors.bind(conn, actor);

        (world, actors, actor, conn)
    }

    fn texts(world: &mut World, actors: &Actors, actor: EntityId, conn: ConnectionId, line: &str) -> Vec<String> {
        let table = CommandTable::default();
        let mut out = Vec::new();
        dispatch_bare(&table, world, actors, actor, conn, line, &mut |o| out.push(o));
        out.into_iter()
            .map(|o| match o {
                Outgoing::Event(Event { text, to: Audience::Connection(_), .. }) => text,
                other => panic!("expected connection event, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn bare_direction_abbreviation_moves() {
        let (mut world, actors, actor, conn) = world_with_player();
        // "n" resolves to north and traverses the exit.
        let out = texts(&mut world, &actors, actor, conn, "n");
        assert!(out.iter().any(|t| t.contains("a quiet garden")));
    }

    #[test]
    fn unknown_verb_feeds_back() {
        let (mut world, actors, actor, conn) = world_with_player();
        let out = texts(&mut world, &actors, actor, conn, "frobnicate");
        assert!(out.iter().any(|t| t.contains("I don't understand")));
    }
}
