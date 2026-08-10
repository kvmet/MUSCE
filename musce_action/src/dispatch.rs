//! The command table: a registry of in-app verbs the dispatcher looks up by
//! name, plus the single entry point the host calls for a bare (embodied)
//! command. An app registers its verbs here rather than into a growing `match`,
//! and lookup resolves abbreviations (`n` -> `north`, `dr` -> `drop`) so adding a
//! verb is a local change. The table, registration, and lookup are engine
//! mechanism; the verbs themselves are app content. See
//! `docs/architecture/actions.md`.

use musce_core::{EntityId, World};
use musce_proto::Outgoing;

use crate::audience::{self, Outbound};
use crate::bindings::Actors;
use crate::caps::{CapId, Verdict};
use crate::ctx::{Caller, ColdOp, Ctx};

/// A verb's parse-and-act function. Receives the command context and the
/// argument tail (everything after the verb word). An app writes these and
/// registers them; the engine only invokes them.
pub type Handler = fn(&mut Ctx, &str);

/// An app's grounded-act function: perform an affordance whose entities are already
/// bound (a pointing client clicked them), so there is no noun to resolve. The
/// bound values travel as one [`Grounded`] value so additions cannot drift across
/// positional arguments. The app maps `focus`/`with` onto roles, since which role
/// the focus fills is app policy.
pub type PerformHandler = fn(&mut Ctx, Grounded<'_>);

/// A grounded act's bound parts: the affordance name and the entities a pointing
/// client clicked (the `focus`, and any second entity a role sub-pick supplied).
/// What a name resolver would produce for a typed verb, handed in ready-made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grounded<'a> {
    pub affordance: &'a str,
    pub focus: EntityId,
    pub with: Option<EntityId>,
}

/// Permission required to run a verb, checked at dispatch before the handler runs.
/// `Open` is every in-app verb; `Cap` gates a verb on an account capability. The
/// capability is app vocabulary (an interned [`CapId`] the app's caps registry
/// mints); the engine owns only this membership check. su bypasses gates, so a
/// superuser passes any `Cap` gate regardless of its grants. See
/// `docs/architecture/authorization.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Open,
    Cap(CapId),
}

impl Gate {
    pub(crate) fn permits(self, verdict: &Verdict) -> bool {
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

/// The registry of in-app verbs. Ordered: lookup prefers an exact name, then the
/// first registered verb the input is a prefix of, so registration order is the
/// abbreviation tie-break (register movement before `say`, so `s` is south and
/// `sa` is say). An app builds one at boot and the runtime shares it read-only
/// across ticks.
pub struct CommandTable {
    verbs: Vec<Verb>,
}

impl CommandTable {
    /// A fresh, empty table. An app fills it by `register`ing its verbs.
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

/// Dispatch one command line against a command table for a [`Caller`]: look the verb
/// up, gate-check it on the caller's verdict, run its handler to gather semantic
/// output, then resolve those events' audiences to connections through `emit`. Frame
/// selection (`@`-floor vs embodiment vs admin) is the host's job; this runs
/// whichever table the host hands it, so it serves both the bare embodiment frame
/// (the app table) and the admin frame (the `@`-verb table), the gate carrying the
/// difference.
pub fn dispatch_command(
    table: &CommandTable,
    world: &mut World,
    actors: &Actors,
    caller: Caller,
    line: &str,
    emit: &mut impl FnMut(Outgoing),
) -> Vec<ColdOp> {
    let line = line.trim();
    let (word, rest) = match line.split_once(char::is_whitespace) {
        Some((w, r)) => (w, r.trim_start()),
        None => (line, ""),
    };

    // A wordless line is a no-op, not a match. The empty string is a prefix of every
    // verb, so lookup would otherwise fire the first-registered one; the `@`-namespace
    // reaches here with an empty tail (a lone `@`), which the host's empty-line guard
    // does not catch. Silent, matching how an empty bare line is dropped upstream.
    if word.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<Outbound> = Vec::new();
    // The block scopes `ctx` so its `&mut out` borrow ends before `out` is resolved;
    // its tail hands back the cold-store requests the handler queued (a plain move,
    // since a cold op needs no world/actor resolution).
    let cold = {
        let mut ctx = Ctx::new(world, caller, &mut out);
        match table.lookup(&word.to_lowercase()) {
            Some(verb) if verb.gate.permits(ctx.verdict()) => (verb.handler)(&mut ctx, rest),
            Some(_) => ctx.feedback("You aren't allowed to do that."),
            None => ctx.feedback(format!("I don't understand \"{word}\".")),
        }
        ctx.take_cold()
    };

    for ob in out {
        audience::resolve(world, actors, ob, emit);
    }

    cold
}

/// Perform a grounded act: build a [`Ctx`], run the app's `perform` handler with
/// the already-bound entities, then resolve the events it emits to connections. The
/// same Ctx-and-audience machinery `dispatch_command` runs, entered with a bound
/// affordance frame instead of a parsed line, so a click narrates through exactly
/// the path a verb does. Table lookup and parsing are absent (the client did that
/// by clicking). The canonical affordance performer and its authority check are
/// still pending; this entry point only routes the grounded callback.
pub fn dispatch_perform(
    world: &mut World,
    actors: &Actors,
    caller: Caller,
    handler: PerformHandler,
    act: Grounded,
    emit: &mut impl FnMut(Outgoing),
) -> Vec<ColdOp> {
    let mut out: Vec<Outbound> = Vec::new();
    let cold = {
        let mut ctx = Ctx::new(world, caller, &mut out);
        handler(&mut ctx, act);
        ctx.take_cold()
    };

    for ob in out {
        audience::resolve(world, actors, ob, emit);
    }

    cold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CapSet;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Description, Locus};
    use musce_proto::{ConnectionId, Delivery, EventKind};

    /// Two test verbs over the public emit API, standing in for app content so
    /// the engine routing is exercised without depending on a real app. `ping`
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
            Caller::new(actor, conn, &Verdict::guest()),
            line,
            &mut |o| out.push(o),
        );
        out.into_iter()
            .map(|o| match o {
                Outgoing::Event(Delivery { text, .. }) => text,
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
    fn empty_line_is_a_noop() {
        // A wordless line must not match a verb: the empty string is a prefix of
        // every registered verb, so without the guard this fires the first one. The
        // `@`-namespace reaches dispatch with an empty tail (a lone `@`), so this is
        // the engine's guard against a bare `@` running the first admin verb.
        let (mut world, actors, actor, conn) = world_with_player();
        assert!(texts(&mut world, &actors, actor, conn, "").is_empty());
        assert!(texts(&mut world, &actors, actor, conn, "   ").is_empty());
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
            Caller::new(actor, conn, &Verdict::guest()),
            "yell",
            &mut |o| out.push(o),
        );
        assert!(matches!(
            out.as_slice(),
            [Outgoing::Event(Delivery {
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
        let mut caps = crate::CapRegistry::new();
        let cap = caps.register("smite");

        let mut t = CommandTable::new();
        t.register("smite", Gate::Cap(cap), |c, _| c.feedback("zap"));

        // Guest verdict lacks the cap: refused.
        let guest = Verdict::guest();
        let mut out = Vec::new();
        dispatch_command(
            &t,
            &mut world,
            &actors,
            Caller::new(actor, conn, &guest),
            "smite",
            &mut |o| out.push(o),
        );
        let text = match &out[..] {
            [Outgoing::Event(Delivery { text, .. })] => text.clone(),
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
            Caller::new(actor, conn, &granted),
            "smite",
            &mut |o| out.push(o),
        );
        assert!(
            matches!(&out[..], [Outgoing::Event(Delivery { text, .. })] if text.contains("zap")),
            "the granted verdict should run the verb, got: {out:?}"
        );

        // su bypasses the gate with no grant at all.
        let su = Verdict::new(CapSet::new(), true);
        let mut out = Vec::new();
        dispatch_command(
            &t,
            &mut world,
            &actors,
            Caller::new(actor, conn, &su),
            "smite",
            &mut |o| out.push(o),
        );
        assert!(
            matches!(&out[..], [Outgoing::Event(Delivery { text, .. })] if text.contains("zap")),
            "su should bypass the gate, got: {out:?}"
        );
    }

    #[test]
    fn grounded_act_reaches_the_handler_as_one_complete_value() {
        let (mut world, actors, actor, conn) = world_with_player();
        let second = world.spawn(EntityBuilder::new());
        let verdict = Verdict::guest();
        let mut out = Vec::new();
        dispatch_perform(
            &mut world,
            &actors,
            Caller::new(actor, conn, &verdict),
            |ctx, grounded| {
                ctx.feedback(format!(
                    "{}:{}:{}",
                    grounded.affordance,
                    grounded.focus.0,
                    grounded.with.unwrap().0
                ));
            },
            Grounded {
                affordance: "put",
                focus: actor,
                with: Some(second),
            },
            &mut |outgoing| out.push(outgoing),
        );
        assert!(matches!(
            &out[..],
            [Outgoing::Event(Delivery { text, .. })]
                if text == &format!("put:{}:{}", actor.0, second.0)
        ));
    }
}
