//! The reference game's tick-loop systems: simulation that runs on its own,
//! without a command driving it. Where a verb handler is "a player did X", a
//! system is "the world does X every tick". Each is `fn(&mut SystemCtx)`, mutating
//! through `execute` and emitting third-person narration, which the runtime
//! resolves to connections the same way it does a verb's output. See
//! `docs/architecture/concurrency.md` and `docs/architecture/engine-and-game.md`.

use musce_action::{Action, SystemCtx, execute};
use musce_core::{Controls, Description, EntityId, Id, NamedComponent, World};
use musce_proto::EventKind;
use serde::{Deserialize, Serialize};

/// Marks a creature that drifts between rooms on its own. A game-defined component
/// (the engine has no notion of wandering): registered and persisted via
/// [`register`], so a wanderer survives a reboot still wandering. Opt-in, so a
/// plain creature stays put.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Wander;

impl NamedComponent for Wander {
    const TAG: &'static str = "wander";
}

/// Register this game's own world types on a fresh world, before it loads or
/// seeds. The runtime calls this through `Game.register`.
pub fn register(world: &mut World) {
    world.register_component::<Wander>();
}

/// How often, in ticks, a wanderer takes a step. Small, so the runtime demo and
/// the e2e see movement quickly; a per-creature cadence can come later.
pub const WANDER_EVERY: u64 = 5;

/// Move every uncontrolled [`Wander`] creature one room along an exit, on ticks
/// that are a non-zero multiple of [`WANDER_EVERY`]. A creature someone is
/// controlling (a `Controls` edge onto it) is left alone, so possessing a
/// wanderer halts it. The exit is chosen deterministically (lowest-id usable
/// exit) so the simulation is reproducible across runs.
pub fn wander(ctx: &mut SystemCtx) {
    // Tick 0 is boot; only act on later scheduled ticks.
    if ctx.tick == 0 || !ctx.tick.is_multiple_of(WANDER_EVERY) {
        return;
    }

    // Collect the wanderers first: the moves below mutate the same world we would
    // otherwise be iterating.
    let wanderers: Vec<EntityId> = ctx
        .world
        .ecs
        .query::<(&Id, &Wander)>()
        .iter()
        .map(|(id, _)| id.0)
        .collect();

    for creature in wanderers {
        // A controller halts it: piloting a wanderer should stop it in its tracks.
        if ctx.world.target_of::<Controls>(creature).is_some() {
            continue;
        }
        let Some(room) = ctx.world.enclosing_room(creature) else {
            continue;
        };

        // `exits_of` is a reverse index rebuilt on load with no guaranteed order,
        // so sort by id and take the lowest usable exit for a deterministic step.
        // Skip half-wired exits (no destination), the same way `go` does.
        let mut exits = ctx.world.exits_of(room);
        exits.sort();
        let Some((exit, dest)) = exits
            .into_iter()
            .find_map(|e| ctx.world.exit_destination(e).map(|d| (e, d)))
        else {
            continue; // no exit out of here, or every exit is half-wired
        };

        let who = display_name(ctx.world, creature);
        let dir = ctx.world.label_of(exit).unwrap_or_else(|| "away".into());

        // Departure narration to the room being left, resolved after the move
        // commits (so the creature, now elsewhere, is not among its hearers).
        ctx.emit_room(room, EventKind::Narration, format!("{who} wanders {dir}."));

        // A creature moving into a room cannot close a containment cycle, so this
        // is infallible in practice; the guard is a structural backstop.
        if execute(
            ctx.world,
            Action::Move {
                entity: creature,
                into: dest,
            },
        )
        .is_err()
        {
            continue;
        }

        ctx.emit_room(dest, EventKind::Narration, format!("{who} wanders in."));
    }
}

/// A name for narration, the creature's `Description`, with a neutral fallback.
/// Mirrors the verb layer's `display_name`.
fn display_name(world: &World, entity: EntityId) -> String {
    world
        .entity(entity)
        .and_then(|er| er.get::<&Description>().map(|d| d.0.clone()))
        .unwrap_or_else(|| "something".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_action::Outbound;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Creature, Description, Exit, Label, LeadsFrom, LeadsTo, Player, Room};
    use musce_proto::Audience;
    use std::time::SystemTime;

    struct Fixture {
        world: World,
        rat: EntityId,
        a: EntityId,
        b: EntityId,
    }

    /// Room A with a single north exit to room B, a `Wander` rat standing in A.
    fn fixture() -> Fixture {
        let mut world = World::new();
        register(&mut world);

        let a = spawn(&mut world, |b| {
            b.add(Room);
            b.add(Description("room A".into()));
        });
        let b = spawn(&mut world, |b| {
            b.add(Room);
            b.add(Description("room B".into()));
        });
        link(&mut world, a, b, "north");

        let rat = spawn(&mut world, |b| {
            b.add(Creature);
            b.add(Wander);
            b.add(Description("a sewer rat".into()));
        });
        world.move_entity(rat, a).unwrap();

        Fixture { world, rat, a, b }
    }

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    fn link(w: &mut World, from: EntityId, to: EntityId, dir: &str) {
        let exit = spawn(w, |b| {
            b.add(Exit);
            b.add(Label(dir.into()));
        });
        w.relate::<LeadsFrom>(exit, from).unwrap();
        w.relate::<LeadsTo>(exit, to).unwrap();
    }

    /// Run `wander` at an explicit tick, returning its emitted outbound buffer.
    fn tick(world: &mut World, tick: u64) -> Vec<Outbound> {
        let mut out = Vec::new();
        let mut ctx = SystemCtx::new(world, tick, SystemTime::UNIX_EPOCH, &mut out);
        wander(&mut ctx);
        out
    }

    fn room_narration(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Room(_)))
            .map(|o| o.event.text.clone())
            .collect()
    }

    #[test]
    fn moves_on_a_scheduled_tick_and_narrates() {
        let mut f = fixture();
        let out = tick(&mut f.world, WANDER_EVERY);

        // It stepped from A into B.
        assert_eq!(f.world.enclosing_room(f.rat), Some(f.b));

        let lines = room_narration(&out);
        assert!(
            lines
                .iter()
                .any(|t| t.contains("a sewer rat wanders north")),
            "departure narration, got: {lines:?}"
        );
        assert!(
            lines.iter().any(|t| t.contains("a sewer rat wanders in")),
            "arrival narration, got: {lines:?}"
        );
    }

    #[test]
    fn stays_on_a_non_scheduled_tick() {
        let mut f = fixture();
        // A genuine non-multiple of WANDER_EVERY: nothing happens.
        let out = tick(&mut f.world, WANDER_EVERY + 1);

        assert_eq!(f.world.enclosing_room(f.rat), Some(f.a));
        assert!(room_narration(&out).is_empty());
    }

    #[test]
    fn stays_on_tick_zero() {
        let mut f = fixture();
        // Tick 0 is a multiple of WANDER_EVERY but is boot, explicitly excluded.
        let out = tick(&mut f.world, 0);

        assert_eq!(f.world.enclosing_room(f.rat), Some(f.a));
        assert!(room_narration(&out).is_empty());
    }

    #[test]
    fn a_controller_halts_it() {
        let mut f = fixture();
        let keeper = spawn(&mut f.world, |b| {
            b.add(Player);
            b.add(Description("a rat keeper".into()));
        });
        f.world.relate::<Controls>(f.rat, keeper).unwrap();

        let out = tick(&mut f.world, WANDER_EVERY);

        // Controlled: it stays put and says nothing.
        assert_eq!(f.world.enclosing_room(f.rat), Some(f.a));
        assert!(room_narration(&out).is_empty());
    }

    #[test]
    fn a_room_with_no_exit_keeps_it_put() {
        let mut f = fixture();
        // B has no outgoing exit; a rat there has nowhere to go.
        f.world.move_entity(f.rat, f.b).unwrap();

        let out = tick(&mut f.world, WANDER_EVERY);

        assert_eq!(f.world.enclosing_room(f.rat), Some(f.b));
        assert!(room_narration(&out).is_empty());
    }

    /// A wanderer survives a reboot still wandering: because `Wander` is a
    /// registered component it serializes, so after a snapshot/load round-trip the
    /// reloaded rat still carries it and still steps on a scheduled tick. The fresh
    /// world registers the game's types before load, mirroring the runtime's
    /// register-before-load contract; without that the `wander` tag would fail to
    /// deserialize.
    #[test]
    fn wander_survives_a_reload() {
        let mut f = fixture();
        let snap = f.world.snapshot();

        let mut reloaded = World::new();
        register(&mut reloaded);
        reloaded.load(&snap.entities, snap.next_id).unwrap();

        // Ids round-trip, so the original rat/room handles still address the
        // reloaded world.
        let out = tick(&mut reloaded, WANDER_EVERY);
        assert_eq!(reloaded.enclosing_room(f.rat), Some(f.b));
        assert!(
            room_narration(&out)
                .iter()
                .any(|t| t.contains("a sewer rat wanders north")),
            "reloaded rat should still wander, got: {:?}",
            room_narration(&out)
        );
    }
}
