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
use crate::caps::{CapId, Verdict};
use crate::ctx::Ctx;

/// A verb's parse-and-act function. Receives the command context and the
/// argument tail (everything after the verb word). A game writes these and
/// registers them; the engine only invokes them.
pub type Handler = fn(&mut Ctx, &str);

/// Permission required to run a verb, checked at dispatch before the handler runs.
/// `Open` is every in-game verb; `Cap` gates a verb on an account capability. The
/// capability is game vocabulary (an interned [`CapId`] the game's caps registry
/// mints); the engine owns only this membership check. su bypasses gates, so a
/// superuser passes any `Cap` gate regardless of its grants. See
/// `docs/architecture/authorization.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Open,
    Cap(CapId),
}

impl Gate {
    fn permits(self, verdict: &Verdict) -> bool {
        match self {
            Gate::Open => true,
            Gate::Cap(cap) => verdict.permits(cap),
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

/// Dispatch one command line against a command table for `actor`: look the verb
/// up, gate-check it on the actor, run its handler to gather semantic output, then
/// resolve those events' audiences to connections through `emit`. `actor` is the
/// entity the connection drives. Frame selection (`@`-floor vs embodiment vs
/// admin) is the host's job; this runs whichever table the host hands it, so it
/// serves both the bare embodiment frame (the game table) and the admin frame
/// (the `@`-verb table), the gate carrying the difference.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_command(
    table: &CommandTable,
    world: &mut World,
    actors: &Actors,
    actor: EntityId,
    conn: ConnectionId,
    verdict: &Verdict,
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
        let mut ctx = Ctx::new(world, actor, conn, verdict, &mut out);
        match table.lookup(&word.to_lowercase()) {
            Some(verb) if verb.gate.permits(verdict) => (verb.handler)(&mut ctx, rest),
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
    use crate::CapSet;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Description, Locus};
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
        let locus = {
            let mut b = EntityBuilder::new();
            b.add(Locus);
            world.spawn(b)
        };
        let actor = {
            let mut b = EntityBuilder::new();
            b.add(Description("an actor".into()));
            world.spawn(b)
        };
        world.move_entity(actor, locus).unwrap();

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
        dispatch_command(
            &table,
            world,
            actors,
            actor,
            conn,
            &Verdict::guest(),
            line,
            &mut |o| out.push(o),
        );
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
        dispatch_command(
            &t,
            &mut world,
            &actors,
            actor,
            conn,
            &Verdict::guest(),
            "yell",
            &mut |o| out.push(o),
        );
        assert!(matches!(
            out.as_slice(),
            [Outgoing::Event(Event {
                kind: EventKind::Narration,
                ..
            })]
        ));
    }

    /// A `Gate::Cap` verb runs under a verdict holding the capability and is refused
    /// (handler never runs) under one without it. The verdict is what carries the
    /// permission, not anything on the actor.
    #[test]
    fn cap_gate_permits_only_with_the_cap() {
        let (mut world, actors, actor, conn) = world_with_player();
        let cap = CapId(0);

        let mut t = CommandTable::new();
        t.register("smite", Gate::Cap(cap), |c, _| c.feedback("zap"));

        // Guest verdict lacks the cap: refused.
        let guest = Verdict::guest();
        let mut out = Vec::new();
        dispatch_command(
            &t,
            &mut world,
            &actors,
            actor,
            conn,
            &guest,
            "smite",
            &mut |o| out.push(o),
        );
        let text = match &out[..] {
            [Outgoing::Event(Event { text, .. })] => text.clone(),
            other => panic!("expected one event, got {other:?}"),
        };
        assert!(text.contains("aren't allowed"), "got: {text:?}");

        // A verdict holding the cap: now it runs.
        let granted = Verdict::new([cap].into_iter().collect(), false);
        let mut out = Vec::new();
        dispatch_command(
            &t,
            &mut world,
            &actors,
            actor,
            conn,
            &granted,
            "smite",
            &mut |o| out.push(o),
        );
        assert!(
            matches!(&out[..], [Outgoing::Event(Event { text, .. })] if text.contains("zap")),
            "the granted verdict should run the verb, got: {out:?}"
        );

        // su bypasses the gate with no grant at all.
        let su = Verdict::new(CapSet::new(), true);
        let mut out = Vec::new();
        dispatch_command(
            &t,
            &mut world,
            &actors,
            actor,
            conn,
            &su,
            "smite",
            &mut |o| out.push(o),
        );
        assert!(
            matches!(&out[..], [Outgoing::Event(Event { text, .. })] if text.contains("zap")),
            "su should bypass the gate, got: {out:?}"
        );
    }
}
