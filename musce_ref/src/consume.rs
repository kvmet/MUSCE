//! The reference game's second autonomous agent: a hungry creature that eats. Its
//! one drive is **hunger**, relieved by consuming something edible, and it is the
//! content slice that exercises the planner's *mid-search binding*: unlike the
//! magpie's place-drives (`take`/`put`, whose goal names the thing to move), a
//! consume goal is `fed(actor)`, about the eater, so the food it must find never
//! appears in the goal at all. The planner surfaces the food only while regressing
//! `eat`'s guard and binds it there against what the creature knows. See
//! `docs/architecture/agency/drives.md` and `planner.md`.
//!
//! The loop, once per scheduled tick per uncontrolled hungry creature: a
//! **metabolism** moves the pang (up while unmet, down while fed); the **drive**
//! reads the pang and, past a threshold, emits the `fed(actor)` goal; the
//! **driver** plans `take -> eat` and runs it through the same `perform` a player's
//! verb hits. There is no arbiter here: one drive has nothing to contend with (the
//! arbiter earns its keep on the magpie's two competing drives). Everything is game
//! content over `musce_agency`'s generic mechanism; the mid-search binding it leans
//! on is the one new engine-side capability.

use musce::action::{Outbound, SystemCtx};
use musce::agency::{Beat, Clause, Driver, Goal, Planner, Predicate, Term, UnitCost};
use musce::world::{Controls, EntityId, Id, NamedComponent, World};
use serde::{Deserialize, Serialize};

use crate::agency::{RefWorldModel, eat, known_here, take};
use crate::verbs::Outcome;

/// A creature that gets hungry, carrying its current pang. Its presence opts a
/// creature into the [`consume_drive`], exactly as [`Hoarder`](crate::hoard::Hoarder)
/// opts one into hoarding; the `pang` is the need-state the drive reads. Persisted,
/// so a hungry creature stays hungry across a reboot.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Hunger {
    pub pang: u32,
}

impl NamedComponent for Hunger {
    const TAG: &'static str = "hunger";
}

/// Sated: the marker `eat` grants the eater, and the goal `fed(actor)` the consume
/// drive chains toward. A satisfied need reads this back through the metabolism, so
/// a fed creature's pang cools; the drive itself reads only its own [`Hunger`]. A
/// zero-sized marker, so it persists and reloads as plain presence.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Fed;

impl NamedComponent for Fed {
    const TAG: &'static str = "fed";
}

/// Register the need and the sated marker so both persist and reload. Called from
/// the game's `register` hook before load.
pub fn register(world: &mut World) {
    world.register_component::<Hunger>();
    world.register_component::<Fed>();
}

/// How often, in ticks, a hungry creature's pang advances and it gets a chance to
/// eat. Sized like the seed's `PATROL_STEP` and the magpie's `HOARD_EVERY`, so a
/// creature seeded into a room a player-session test walks through does not act
/// before that test finishes and does not starve the e2e's response-burst gap.
pub const CONSUME_EVERY: u64 = 40;

/// The pang at which the drive starts seeking food. Below it the creature is content
/// and the drive stays quiet; the metabolism climbs it there over a few scheduled
/// ticks so an idle creature visibly grows hungry before it eats.
const THRESHOLD: u32 = 3;

/// The ceiling a pang saturates at, so a creature with nothing to eat does not count
/// up forever.
const NEED_MAX: u32 = 10;

/// How far the pang moves each scheduled tick: up by [`WARM`] while hungry, down by
/// [`COOL`] once fed. Relief is gradual (the pang does not snap to zero on eating)
/// and floored at zero, below [`THRESHOLD`], so a fed creature retires from the
/// running rather than deadlocking, mirroring the magpie's needs.
const WARM: u32 = 1;
const COOL: u32 = 1;

/// The consume drive: read the creature's own pang and, past the threshold, turn it
/// into a goal to be *fed*. The goal is `fed(actor)`, a fact about the eater, not
/// about any food; the food is the planner's mid-search binding, drawn from what the
/// creature knows, not the drive's concern. Returns `None` when the creature is
/// content, which is how a met need loses the drive without the drive ever reading
/// the world.
///
/// The drive reads only the creature's own pang, never the world's contents.
/// Whether anything edible is within reach is the planner's feasibility question:
/// an unreachable goal simply abandons.
pub fn consume_drive(world: &World, eater: EntityId) -> Option<Goal> {
    let pang = world.get::<Hunger>(eater)?.pang;
    if pang < THRESHOLD {
        return None;
    }
    let predicate = Clause(vec![
        Predicate::Tag {
            e: Term::var("actor"),
            comp: "fed".into(),
        }
        .into(),
    ]);
    Some(Goal {
        predicate,
        urgency: pang,
    })
}

/// Plan the fed goal and run it beat by beat through the shared narrating perform,
/// so the room sees the creature take its food and then eat it, the same lines a
/// player eating would produce (an already-fed creature runs an empty plan and
/// narrates nothing). The whole plan runs in this one call, as the magpie's does;
/// interleaving it a beat per tick is the deferred sim refinement.
fn pursue_goal(world: &mut World, out: &mut Vec<Outbound>, eater: EntityId, goal: &Clause) {
    let table = [take(), eat()];
    let known = known_here(world, eater);
    let planner = Planner::new(&table, &RefWorldModel, &UnitCost);
    let driver = Driver::new(&planner);
    driver.pursue(
        eater,
        goal,
        &known,
        world,
        |world, step| match crate::act::perform_narrated(
            world,
            eater,
            &step.affordance,
            &step.frame,
            out,
        ) {
            Outcome::Committed => Beat::Committed,
            Outcome::Refused(_) => Beat::Refused,
        },
    );
}

/// Run every uncontrolled hungry creature one turn of the consume loop, on ticks
/// that are a non-zero multiple of [`CONSUME_EVERY`]. A controlled creature (someone
/// piloting it) is left alone, mirroring [`wander`](crate::systems::wander) and
/// [`hoard`](crate::hoard::hoard). The creature is keyed by its [`Hunger`] need.
pub fn consume(ctx: &mut SystemCtx) {
    // Tick 0 is boot; only act on later scheduled ticks.
    if ctx.tick == 0 || !ctx.tick.is_multiple_of(CONSUME_EVERY) {
        return;
    }

    // Collect first: pursuing a plan below mutates the same world we would otherwise
    // be iterating.
    let eaters: Vec<EntityId> = ctx
        .world
        .query::<(&Id, &Hunger)>()
        .iter()
        .map(|(id, _)| id.0)
        .collect();

    // Split the world and the output buffer once: the driver's per-beat narration
    // emits into `out` while the pursuit mutates `world`.
    let (world, out) = ctx.world_and_out();
    for eater in eaters {
        // A controller halts it, exactly as it halts a wanderer or a magpie.
        if world.target_of::<Controls>(eater).is_some() {
            continue;
        }

        // Metabolism moves the pang from the current world state: up while hungry,
        // down once fed, so relief is a property of the world (read here), never of
        // the drive or the driver.
        metabolize(world, eater);

        let Some(goal) = consume_drive(world, eater) else {
            continue;
        };
        // The narrated perform emits each beat (the take, then the eat) to the room
        // itself, so there is no terminal narration to reconstruct here.
        pursue_goal(world, out, eater, &goal.predicate);
    }
}

/// Move a creature's pang one scheduled tick: up while hungry, down once fed.
/// Satisfaction is read from the world here (the [`Fed`] marker `eat` grants); the
/// drive itself reads only the resulting [`Hunger`].
fn metabolize(world: &mut World, eater: EntityId) {
    if let Some(pang) = world.get::<Hunger>(eater).map(|h| h.pang) {
        let sated = world.has::<Fed>(eater);
        world
            .insert(
                eater,
                Hunger {
                    pang: step_need(pang, sated),
                },
            )
            .expect("a read Hunger belongs to a live eater");
    }
}

/// One tick of the pang's curve: cool toward zero while fed, warm toward the ceiling
/// while hungry.
fn step_need(level: u32, satisfied: bool) -> u32 {
    if satisfied {
        level.saturating_sub(COOL)
    } else {
        (level + WARM).min(NEED_MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::{Creature, Edible, Item};
    use musce::action::{Audience, Outbound};
    use musce::world::hecs::EntityBuilder;
    use musce::world::{Description, Locus, Name};
    use std::time::SystemTime;

    struct Fixture {
        world: World,
        mouse: EntityId,
        bread: EntityId,
    }

    /// A pantry holding a hungry field mouse and a loose crust of bread, co-located
    /// so the mouse knows the bread.
    fn fixture() -> Fixture {
        let mut world = World::new();
        register(&mut world);
        crate::kinds::register(&mut world);
        crate::names::register(&mut world);

        let room = spawn(&mut world, |b| {
            b.add(Locus);
            b.add(Description("a stone pantry".into()));
        });
        let bread = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Edible);
            b.add(Name("a crust of bread".into()));
            b.add(Description("a dry crust of bread".into()));
        });
        world.move_entity(bread, room).unwrap();
        let mouse = spawn(&mut world, |b| {
            b.add(Creature);
            b.add(Hunger { pang: 0 });
            b.add(Name("a field mouse".into()));
            b.add(Description(
                "a small brown field mouse, whiskers twitching".into(),
            ));
        });
        world.move_entity(mouse, room).unwrap();

        Fixture {
            world,
            mouse,
            bread,
        }
    }

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    /// Run `consume` at an explicit tick, returning its emitted outbound buffer.
    fn tick(world: &mut World, tick: u64) -> Vec<Outbound> {
        let mut out = Vec::new();
        let mut ctx = SystemCtx::new(world, tick, SystemTime::UNIX_EPOCH, &[], &mut out);
        consume(&mut ctx);
        out
    }

    fn pang_of(w: &World, eater: EntityId) -> u32 {
        w.get::<Hunger>(eater).map(|h| h.pang).unwrap_or(0)
    }

    fn room_narration(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Locus(_)))
            .map(|o| o.event.text.clone())
            .collect()
    }

    /// The consume drive, live and exercising mid-search binding: an idle mouse grows
    /// hungry over scheduled ticks, then at the threshold the planner grounds the
    /// unnamed food in `eat`'s guard against what the mouse knows and runs
    /// `take -> eat` through the real `perform`. The mouse ends fed, the bread ends
    /// eaten (no longer edible), and the pang is relieved as it rests fed.
    #[test]
    fn a_hungry_mouse_finds_and_eats_the_bread() {
        let mut f = fixture();

        // One sub-threshold tick: the pang rose but the mouse has not eaten yet.
        tick(&mut f.world, CONSUME_EVERY);
        assert_eq!(pang_of(&f.world, f.mouse), 1);
        assert!(f.world.has::<Edible>(f.bread), "still uneaten");
        assert!(!f.world.has::<Fed>(f.mouse));

        // Advance to the threshold tick. The mouse eats: it is now fed and the bread
        // is spent (no longer edible). The pang peaked at the meal (relief is
        // gradual, not an instant reset).
        let mut out = Vec::new();
        for n in 2..=THRESHOLD as u64 {
            out = tick(&mut f.world, CONSUME_EVERY * n);
        }
        assert!(f.world.has::<Fed>(f.mouse), "the mouse is fed");
        assert!(
            !f.world.has::<Edible>(f.bread),
            "the bread is eaten down, no longer edible"
        );
        assert_eq!(
            pang_of(&f.world, f.mouse),
            THRESHOLD,
            "pang peaked at the meal"
        );
        assert!(
            room_narration(&out)
                .iter()
                .any(|t| t.contains("a field mouse eats a crust of bread")),
            "eating narration, got: {:?}",
            room_narration(&out)
        );

        // State-based relief: fed, the next scheduled tick cools the pang, and the
        // mouse does not eat again (the bread is spent and it is already fed).
        let out = tick(&mut f.world, CONSUME_EVERY * (THRESHOLD as u64 + 1));
        assert!(
            pang_of(&f.world, f.mouse) < THRESHOLD,
            "the fed mouse's pang cools over ticks"
        );
        assert!(
            room_narration(&out).is_empty(),
            "an already-fed mouse eats nothing more"
        );
    }

    /// The room hears *every* beat, not just the last. The mouse must take the loose
    /// crust before it can eat it, so the two-beat plan narrates the take and the eat
    /// in turn, the same lines a player would produce. Before the shared narrator the
    /// drive reconstructed one terminal line and the intermediate take was silent (an
    /// NPC grabbed its food invisibly). Both beats are the default affordance prose.
    #[test]
    fn the_room_hears_each_beat_of_the_meal() {
        let mut f = fixture();

        let mut out = Vec::new();
        for n in 1..=THRESHOLD as u64 {
            out = tick(&mut f.world, CONSUME_EVERY * n);
        }

        let lines = room_narration(&out);
        assert!(
            lines
                .iter()
                .any(|t| t == "a field mouse takes a crust of bread."),
            "the take beat narrates, got: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|t| t == "a field mouse eats a crust of bread."),
            "the eat beat narrates, got: {lines:?}"
        );
    }

    /// A controller halts the drive: piloting the mouse stops it eating, its pang
    /// frozen, exactly as controlling a wanderer stops it moving.
    #[test]
    fn a_controller_halts_it() {
        let mut f = fixture();
        let keeper = spawn(&mut f.world, |b| {
            b.add(Creature);
            b.add(Description("a mouser".into()));
        });
        f.world.relate::<Controls>(f.mouse, keeper).unwrap();

        for n in 1..=THRESHOLD as u64 + 1 {
            tick(&mut f.world, CONSUME_EVERY * n);
        }

        assert_eq!(
            pang_of(&f.world, f.mouse),
            0,
            "a piloted mouse's pang is frozen"
        );
        assert!(!f.world.has::<Fed>(f.mouse));
        assert!(f.world.has::<Edible>(f.bread)); // untouched
    }

    /// With nothing edible, a hungry mouse stays hungry: the drive still emits the
    /// fed goal past the threshold, but the planner finds no food to bind, so the
    /// pursuit abandons and the pang keeps climbing (the world never satisfies it).
    #[test]
    fn nothing_edible_leaves_it_hungry() {
        let mut f = fixture();
        f.world.remove::<Edible>(f.bread).unwrap(); // the only food loses its edibility

        for n in 1..=THRESHOLD as u64 + 1 {
            tick(&mut f.world, CONSUME_EVERY * n);
        }

        assert!(
            pang_of(&f.world, f.mouse) >= THRESHOLD,
            "no food within reach, so the pang is never relieved"
        );
        assert!(!f.world.has::<Fed>(f.mouse), "never fed");
    }
}
