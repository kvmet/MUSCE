//! The reference game's first autonomous agent: a magpie that hoards. This is the
//! content slice that exercises the agency stack live, the one thing that turns the
//! off-thread arbiter and driver into a creature acting on the sim tick. See
//! `docs/architecture/agency/drives.md`.
//!
//! The loop, once per scheduled tick per uncontrolled magpie: a **metabolism**
//! nudges the bird's `Hoarder` urge upward; a **drive** reads that urge and, past a
//! threshold, emits a `Goal` to get some shiny thing into its nest; the **arbiter**
//! commits to the goal and the **driver** plans and runs it through the same
//! `perform` a player's verb hits. Stowing something (the goal becomes true) relieves
//! the urge. Nothing here is engine machinery: the need, its curve, and the consuming
//! action are all game content over `musce_agency`'s generic mechanism.

use std::cell::RefCell;

use musce::action::{SystemCtx, Verdict};
use musce::agency::{
    Arbiter, Beat, Clause, Driver, Goal, Planner, Predicate, Progress, Term, UnitCost,
};
use musce::wire::EventKind;
use musce::world::{Cascade, Controls, EntityId, Id, NamedComponent, Relation, World};
use serde::{Deserialize, Serialize};

use crate::agency::{RefWorldModel, known_here, perform, put, take};
use crate::names::display_name;
use crate::verbs::Outcome;

/// A creature that hoards, carrying its current urge to do so. Its presence opts a
/// creature into the [`hoard`] drive, exactly as [`Wander`](crate::systems::Wander)
/// opts one into wandering; the `urge` is the need-state the drive reads. Persisted,
/// so a restless magpie stays restless across a reboot.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Hoarder {
    pub urge: u32,
}

impl NamedComponent for Hoarder {
    const TAG: &'static str = "hoarder";
}

/// Where a hoarder stows its finds: source = the bird, target = the container it
/// treats as its nest. One nest per bird. If the nest is destroyed the edge simply
/// detaches (the bird survives, nestless, and the drive stops finding a goal) rather
/// than taking the bird down with it, so this is `Detach`, not `DespawnSources`.
pub struct Nest;

impl Relation for Nest {
    const ACYCLIC: bool = false;
    const ON_TARGET_DESPAWN: Cascade = Cascade::Detach;
    const TARGET_TAG: &'static str = "nest";
}

/// Register the hoarding component and the nest relation so both persist, reload,
/// and (for the nest) cascade. Called from the game's `register` hook before load.
pub fn register(world: &mut World) {
    world.register_component::<Hoarder>();
    world.register_relation::<Nest>();
}

/// How often, in ticks, a magpie's urge advances and it gets a chance to act. Small,
/// like [`WANDER_EVERY`](crate::systems::WANDER_EVERY), so the demo and the e2e see
/// behavior quickly.
pub const HOARD_EVERY: u64 = 5;

/// The urge at which the drive starts emitting a goal. Below it the bird is content
/// and the drive stays quiet; the metabolism climbs it there over a few scheduled
/// ticks so an idle magpie visibly grows restless before it acts.
const URGE_THRESHOLD: u32 = 3;

/// The ceiling the urge saturates at, so a magpie with nothing to steal does not
/// count up forever.
const URGE_MAX: u32 = 10;

/// The arbiter's hysteresis band. With today's single drive there is never a
/// challenger, so it is dormant; it is passed honestly so wiring a second drive
/// later needs no change here. Cross-tick commitment (a persisted incumbent) is the
/// surfaced next step, not built (see `docs/architecture/agency/drives.md`).
const HYSTERESIS: u32 = 2;

/// The hoarding drive: read the bird's own urge and, past the threshold, turn it into
/// a goal to get *some* shiny thing into its nest. The goal's fungible slot (`x`, the
/// thing to stow) is left for the planner to bind against what the bird knows; the
/// nest is a constant the bird reads from its own [`Nest`] edge. Returns `None` when
/// the bird is content or has no nest, which is how a met need loses the ranking
/// without the arbiter ever reading the world.
///
/// The drive reads only the bird's own state (its urge, its nest), never the world's
/// contents. Whether a shiny is actually within reach is the planner's feasibility
/// question, not the drive's: an unreachable goal simply abandons.
pub fn hoard_drive(world: &World, bird: EntityId) -> Option<Goal> {
    let urge = world.get::<Hoarder>(bird)?.urge;
    if urge < URGE_THRESHOLD {
        return None;
    }
    let nest = world.target_of::<Nest>(bird)?;
    let predicate = Clause(vec![
        Predicate::Related {
            a: Term::var("x"),
            b: Term::Const(nest),
            kind: "contained_by".into(),
        }
        .into(),
        Predicate::Tag {
            e: Term::var("x"),
            comp: "shiny".into(),
        }
        .into(),
    ]);
    Some(Goal {
        predicate,
        urgency: urge,
    })
}

/// Run every uncontrolled [`Hoarder`] one turn of the agency loop, on ticks that are
/// a non-zero multiple of [`HOARD_EVERY`]. A controlled bird (someone piloting it)
/// is left alone, mirroring [`wander`](crate::systems::wander). The whole committed
/// plan runs in this one call; interleaving it a beat per tick is the deferred sim
/// refinement (see `docs/architecture/agency/execution.md`).
pub fn hoard(ctx: &mut SystemCtx) {
    // Tick 0 is boot; only act on later scheduled ticks.
    if ctx.tick == 0 || !ctx.tick.is_multiple_of(HOARD_EVERY) {
        return;
    }

    // Collect first: pursuing a plan below mutates the same world we would otherwise
    // be iterating.
    let birds: Vec<EntityId> = ctx
        .world
        .query::<(&Id, &Hoarder)>()
        .iter()
        .map(|(id, _)| id.0)
        .collect();

    for bird in birds {
        // A controller halts it, exactly as it halts a wanderer.
        if ctx.world.target_of::<Controls>(bird).is_some() {
            continue;
        }

        // Metabolism: the urge to hoard climbs each scheduled tick, saturating.
        let urge = ctx.world.get::<Hoarder>(bird).map(|h| h.urge).unwrap_or(0);
        ctx.world.insert(
            bird,
            Hoarder {
                urge: (urge + 1).min(URGE_MAX),
            },
        );

        // Drive -> arbiter -> driver -> perform. A fresh arbiter each tick (2a): with
        // one drive there is nothing to commit across ticks, so hysteresis is dormant
        // and re-picking is correct.
        let goals: Vec<Goal> = hoard_drive(ctx.world, bird).into_iter().collect();
        let Some(goal) = Arbiter::new(HYSTERESIS).select(&goals) else {
            continue;
        };

        let table = [take(), put()];
        let known = known_here(ctx.world, bird);
        let planner = Planner::new(&table, &RefWorldModel, &UnitCost);
        let driver = Driver::new(&planner);

        // What the bird stowed this turn, if anything, captured off the committing
        // `put` beat so it can be narrated after the pursuit frees the world borrow.
        let stowed: RefCell<Option<EntityId>> = RefCell::new(None);
        let progress = driver.pursue(bird, &goal.predicate, &known, ctx.world, |world, step| {
            let out = perform(world, &step.affordance, &step.frame, &Verdict::guest());
            if step.affordance.name == "put"
                && matches!(out, Outcome::Committed)
                && let Some(item) = step.frame.object
            {
                *stowed.borrow_mut() = Some(item);
            }
            match out {
                Outcome::Committed => Beat::Committed,
                Outcome::Refused(_) => Beat::Refused,
            }
        });

        // Consummation: a completed hoard (or an already-satisfied one, an empty plan)
        // relieves the urge. An abandoned pursuit (nothing within reach to stow)
        // leaves the bird restless, to try again as the world changes.
        if progress == Progress::Achieved {
            ctx.world.insert(bird, Hoarder { urge: 0 });
        }

        if let Some(item) = *stowed.borrow()
            && let Some(room) = ctx.world.enclosing_locus(bird)
        {
            let who = display_name(ctx.world, bird);
            let what = display_name(ctx.world, item);
            ctx.emit_locus(
                room,
                EventKind::Narration,
                format!("{who} tucks {what} into its nest."),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::{Container, Creature, Item, Shiny};
    use musce::action::{Audience, Outbound};
    use musce::world::hecs::EntityBuilder;
    use musce::world::{Description, Locus, Name};
    use std::time::SystemTime;

    struct Fixture {
        world: World,
        magpie: EntityId,
        nest: EntityId,
        bead: EntityId,
        room: EntityId,
    }

    /// A single room holding a magpie, its (empty) nest chest, and a loose shiny
    /// bead, all co-located so the bird knows all three.
    fn fixture() -> Fixture {
        let mut world = World::new();
        register(&mut world);
        crate::kinds::register(&mut world);
        crate::names::register(&mut world);

        let room = spawn(&mut world, |b| {
            b.add(Locus);
            b.add(Description("a cluttered loft".into()));
        });
        let nest = spawn(&mut world, |b| {
            b.add(Container);
            b.add(Name("a twiggy nest".into()));
            b.add(Description("a nest of twigs and wire".into()));
        });
        world.move_entity(nest, room).unwrap();
        let bead = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Shiny);
            b.add(Name("a glass bead".into()));
            b.add(Description("a bead of bright blue glass".into()));
        });
        world.move_entity(bead, room).unwrap();
        let magpie = spawn(&mut world, |b| {
            b.add(Creature);
            b.add(Hoarder { urge: 0 });
            b.add(Name("a magpie".into()));
            b.add(Description(
                "a glossy magpie, head cocked at anything that glitters".into(),
            ));
        });
        world.move_entity(magpie, room).unwrap();
        world.relate::<Nest>(magpie, nest).unwrap();

        Fixture {
            world,
            magpie,
            nest,
            bead,
            room,
        }
    }

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    /// Run `hoard` at an explicit tick, returning its emitted outbound buffer.
    fn tick(world: &mut World, tick: u64) -> Vec<Outbound> {
        let mut out = Vec::new();
        let mut ctx = SystemCtx::new(world, tick, SystemTime::UNIX_EPOCH, &[], &mut out);
        hoard(&mut ctx);
        out
    }

    fn urge_of(w: &World, bird: EntityId) -> u32 {
        w.get::<Hoarder>(bird).map(|h| h.urge).unwrap_or(0)
    }

    fn room_narration(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Locus(_)))
            .map(|o| o.event.text.clone())
            .collect()
    }

    /// The whole drive, live: an idle magpie grows restless over scheduled ticks,
    /// then, once the urge crosses the threshold, plans and runs take-then-put through
    /// the real `perform` to stow the bead in its nest, and the urge falls back. This
    /// is the executable oracle for the sim wiring and the drive at once: nothing
    /// moves the bead but the arbiter/driver loop, and nothing moves the urge but the
    /// metabolism and the consummation.
    #[test]
    fn an_idle_magpie_grows_restless_then_stows_a_shiny() {
        let mut f = fixture();

        // One sub-threshold tick: the urge rose but the bird has not acted yet.
        tick(&mut f.world, HOARD_EVERY);
        assert_eq!(urge_of(&f.world, f.magpie), 1);
        assert_eq!(f.world.container_of(f.bead), Some(f.room)); // still loose

        // Advance to the threshold tick. The bead ends up in the nest and the urge is
        // relieved.
        let mut out = Vec::new();
        for n in 2..=URGE_THRESHOLD as u64 {
            out = tick(&mut f.world, HOARD_EVERY * n);
        }
        assert_eq!(
            f.world.container_of(f.bead),
            Some(f.nest),
            "stowed in the nest"
        );
        assert_eq!(urge_of(&f.world, f.magpie), 0, "stowing relieved the urge");
        assert!(
            room_narration(&out)
                .iter()
                .any(|t| t.contains("a magpie tucks a glass bead into its nest")),
            "stow narration, got: {:?}",
            room_narration(&out)
        );
    }

    /// A controller halts the drive: piloting the magpie stops it acquiring, its urge
    /// frozen, exactly as controlling a wanderer stops it moving.
    #[test]
    fn a_controller_halts_it() {
        let mut f = fixture();
        let keeper = spawn(&mut f.world, |b| {
            b.add(Creature);
            b.add(Description("a falconer".into()));
        });
        f.world.relate::<Controls>(f.magpie, keeper).unwrap();

        for n in 1..=URGE_THRESHOLD as u64 + 1 {
            tick(&mut f.world, HOARD_EVERY * n);
        }

        assert_eq!(
            urge_of(&f.world, f.magpie),
            0,
            "a piloted bird's urge is frozen"
        );
        assert_eq!(f.world.container_of(f.bead), Some(f.room)); // untouched
    }

    /// With nothing to steal, a restless magpie stays restless: the drive still emits
    /// a goal past the threshold, but the planner finds no shiny to bind, so the
    /// pursuit abandons and the urge is *not* relieved (the bird keeps looking). This
    /// covers the Abandoned branch of the consummation.
    #[test]
    fn nothing_to_steal_leaves_it_restless() {
        let mut f = fixture();
        f.world.remove::<Shiny>(f.bead); // the only glittery thing loses its shine

        for n in 1..=URGE_THRESHOLD as u64 + 1 {
            tick(&mut f.world, HOARD_EVERY * n);
        }

        assert!(
            urge_of(&f.world, f.magpie) >= URGE_THRESHOLD,
            "no shiny within reach, so the urge is never relieved"
        );
        assert_eq!(f.world.container_of(f.bead), Some(f.room)); // nothing stowed
    }
}
