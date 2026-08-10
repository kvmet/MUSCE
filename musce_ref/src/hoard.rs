//! The reference game's first autonomous agent: a magpie with two competing drives.
//! It **hoards** (stow a shiny thing in its nest) and it **admires** (hold a shiny
//! thing to turn over), and those two wants pull the same bead in opposite
//! directions. This is the content slice that exercises the whole agency stack live,
//! and specifically the one thing that makes the arbiter's *commitment* observable:
//! two goals that wobble around each other tick to tick. See
//! `docs/architecture/agency/drives.md`.
//!
//! The loop, once per scheduled tick per uncontrolled magpie: a **metabolism** moves
//! both needs (each rises while unmet, falls while met); two **drives** read those
//! needs and, past a threshold, emit a `Goal`; the **arbiter**, resumed from the
//! bird's persisted commitment, picks one under hysteresis; the **driver** plans and
//! runs it through the same `perform` a player's verb hits. Nothing here is engine
//! machinery: the needs, their curves, the drives, and the serializable record of
//! what the bird is committed to are all game content over `musce_agency`'s generic
//! mechanism.

use std::cell::RefCell;

use musce::action::SystemCtx;
use musce::agency::{
    Arbiter, Beat, Clause, Driver, Goal, Planner, Predicate, Progress, Term, UnitCost,
};
use musce::wire::EventKind;
use musce::world::{Cascade, Controls, EntityId, Id, NamedComponent, Relation, World};
use serde::{Deserialize, Serialize};

use crate::agency::{RefWorldModel, known_here, perform, put, take};
use crate::kinds::Shiny;
use crate::names::display_name;
use crate::verbs::Outcome;

/// A creature that hoards, carrying its current urge to do so. Its presence opts a
/// creature into the [`hoard_drive`], exactly as [`Wander`](crate::systems::Wander)
/// opts one into wandering; the `urge` is the need-state the drive reads. Persisted,
/// so a restless magpie stays restless across a reboot.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Hoarder {
    pub urge: u32,
}

impl NamedComponent for Hoarder {
    const TAG: &'static str = "hoarder";
}

/// A creature that covets glitter for its own sake, carrying its current itch to hold
/// something shiny. Its presence opts a creature into the [`admire_drive`]; the `itch`
/// is the need-state that drive reads. On the magpie this contends with [`Hoarder`]:
/// admiring wants the bead in hand, hoarding wants it in the nest.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Curiosity {
    pub itch: u32,
}

impl NamedComponent for Curiosity {
    const TAG: &'static str = "curiosity";
}

/// Which drive an agent is currently committed to. The serializable record that lets
/// the arbiter's cross-tick commitment survive on the world: agency types do not
/// serialize, so the bird persists *which* drive it committed to, not the arbiter,
/// and the loop reconstructs the arbiter each tick from this tag (see
/// [`commit_and_select`]). Absent means uncommitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Drive {
    Hoard,
    Admire,
}

/// The drive a bird has committed to, if any. A one-field component so it persists,
/// reloads, and reads back as the arbiter's incumbent next tick.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Committed(pub Drive);

impl NamedComponent for Committed {
    const TAG: &'static str = "committed";
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

/// Register the two need components, the commitment record, and the nest relation so
/// all persist, reload, and (for the nest) cascade. Called from the game's `register`
/// hook before load.
pub fn register(world: &mut World) {
    world.register_component::<Hoarder>();
    world.register_component::<Curiosity>();
    world.register_component::<Committed>();
    world.register_relation::<Nest>();
}

/// How often, in ticks, a magpie's needs advance and it gets a chance to act. Sized
/// like the seed's `PATROL_STEP`, not small like `WANDER_EVERY`, and for the same
/// reason: unlike a wanderer or a one-shot, the magpie oscillates *forever* (admiring,
/// then stowing, the same bead), narrating each time it acts. At the e2e's 10ms tick
/// rate a 40-tick cadence is 400ms between actions, longer than the 300ms gap the e2e
/// uses to delimit a response burst, so a magpie in a room a player-session test walks
/// through never starves that gap and hangs the test.
pub const HOARD_EVERY: u64 = 40;

/// The need level at which a drive starts emitting a goal. Below it the bird is
/// content on that axis and the drive stays quiet; the metabolism climbs it there
/// over a few scheduled ticks so an idle magpie visibly grows restless before it acts.
const THRESHOLD: u32 = 3;

/// The ceiling a need saturates at, so a magpie with an unmeetable want does not count
/// up forever.
const NEED_MAX: u32 = 10;

/// How far a need moves each scheduled tick: up by [`WARM`] while unmet, down by
/// [`COOL`] while met. Relief is deliberately *gradual* (a need does not snap to zero
/// on satisfaction) and its floor is zero, below [`THRESHOLD`]: that gives a satisfied
/// drive a multi-tick window where it still offers its goal, which is what makes the
/// arbiter's commitment matter, and guarantees it eventually retires so pursuit never
/// deadlocks.
const WARM: u32 = 1;
const COOL: u32 = 1;

/// The arbiter's hysteresis band. With two competing drives this is live: a challenger
/// steals commitment only when its urgency clears the incumbent's by more than this,
/// so the bird finishes admiring a bead before stowing it (and vice versa) instead of
/// yo-yoing it between hand and nest every tick. Cross-tick commitment is what makes
/// the band bite; see [`commit_and_select`] and `docs/architecture/agency/arbiter.md`.
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
    if urge < THRESHOLD {
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

/// The admiring drive: read the bird's own itch and, past the threshold, turn it into
/// a goal to *hold* some shiny thing. The goal's fungible slot (`x`) is bound by the
/// planner against what the bird knows; `actor` is the bird itself. Returns `None`
/// when the bird is content, so a met need loses the ranking.
///
/// Its predicate differs from the hoard goal's only in the container (`actor` here,
/// the nest there), so the two are never equal: the arbiter can hold one as an
/// incumbent while the other challenges, and the commitment record round-trips
/// unambiguously by predicate.
pub fn admire_drive(world: &World, bird: EntityId) -> Option<Goal> {
    let itch = world.get::<Curiosity>(bird)?.itch;
    if itch < THRESHOLD {
        return None;
    }
    let predicate = Clause(vec![
        Predicate::Related {
            a: Term::var("x"),
            b: Term::var("actor"),
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
        urgency: itch,
    })
}

/// Both drives' current goals, each labeled with which drive emitted it, in a fixed
/// order. A drive that is content contributes nothing. The label is what lets the loop
/// map the persisted commitment tag to *this tick's* goal and back.
fn drive_goals(world: &World, bird: EntityId) -> Vec<(Drive, Goal)> {
    let mut goals = Vec::new();
    if let Some(g) = hoard_drive(world, bird) {
        goals.push((Drive::Hoard, g));
    }
    if let Some(g) = admire_drive(world, bird) {
        goals.push((Drive::Admire, g));
    }
    goals
}

/// Select this tick's goal under cross-tick commitment, and persist the choice.
///
/// The stateful `Arbiter` cannot be persisted (agency types do not serialize), so the
/// commitment is reconstructed from the world each tick: the bird's [`Committed`] tag
/// names the drive it committed to, which is mapped to *this tick's* goal from that
/// drive (the live incumbent, or `None` if that drive has gone quiet, so a stale tag
/// never resurrects a goal), and the arbiter is [`resumed`](Arbiter::resume) with it.
/// The chosen goal is mapped back to its drive and written to [`Committed`]. An empty
/// candidate set clears the commitment.
fn commit_and_select(world: &mut World, bird: EntityId, hysteresis: u32) -> Option<(Drive, Goal)> {
    let candidates = drive_goals(world, bird);
    if candidates.is_empty() {
        world
            .remove::<Committed>(bird)
            .expect("a drive candidate names a live bird");
        return None;
    }

    let incumbent = world.get::<Committed>(bird).map(|c| c.0).and_then(|tag| {
        candidates
            .iter()
            .find(|(drive, _)| *drive == tag)
            .map(|(_, goal)| goal.clone())
    });

    let goals: Vec<Goal> = candidates.iter().map(|(_, goal)| goal.clone()).collect();
    let chosen = Arbiter::resume(hysteresis, incumbent).select(&goals)?;

    let (drive, goal) = candidates
        .into_iter()
        .find(|(_, goal)| goal.predicate == chosen.predicate)
        .expect("the chosen goal came from a candidate");
    world
        .insert(bird, Committed(drive))
        .expect("the selected drive belongs to a live bird");
    Some((drive, goal))
}

/// What a magpie can perceive for planning: its surroundings, its own claws, and its
/// own nest. The engine's [`known_here`] gives co-located room contents; a bird also
/// knows what it is *holding* and what sits in the cache it owns, because those are
/// the bird's own state, not the world at large. This is game policy, not an engine
/// guarantee: a creature is not assumed to see into every container (a locked box it
/// carries would not qualify), only into its own inventory and nest. General
/// perception into arbitrary containers stays deferred (see the agency docs).
fn magpie_known(world: &World, bird: EntityId) -> Vec<EntityId> {
    let mut known = known_here(world, bird);
    known.extend_from_slice(world.contents(bird));
    if let Some(nest) = world.target_of::<Nest>(bird) {
        known.extend_from_slice(world.contents(nest));
    }
    known
}

/// Plan and run the committed goal through the *silent* [`perform`], returning how it
/// ended and the object of the last committed beat (what the bird actually moved, for
/// narration). This is the deliberate flavor-override path: the magpie's line is
/// goal-flavored ("tucks it into its nest" for a `put` serving the hoard drive, not
/// the default "puts it in the nest"), which the affordance-level narrator cannot
/// express, so the bird keeps its beats silent and emits one evocative line itself
/// (see [`hoard`] and `crate::act`). The whole plan runs in this one call;
/// interleaving it a beat per tick is the deferred sim refinement (see
/// `docs/architecture/agency/execution.md`).
fn pursue_goal(world: &mut World, bird: EntityId, goal: &Clause) -> (Progress, Option<EntityId>) {
    let table = [take(), put()];
    let known = magpie_known(world, bird);
    let planner = Planner::new(&table, &RefWorldModel, &UnitCost);
    let driver = Driver::new(&planner);

    // The object of the last beat that committed, captured so the narration names what
    // the bird acted on after the pursuit frees the world borrow.
    let acted: RefCell<Option<EntityId>> = RefCell::new(None);
    let progress = driver.pursue(bird, goal, &known, world, |world, step| {
        let out = perform(world, &step.affordance, &step.frame);
        if matches!(out, Outcome::Committed)
            && let Some(item) = step.frame.object
        {
            *acted.borrow_mut() = Some(item);
        }
        match out {
            Outcome::Committed => Beat::Committed,
            Outcome::Refused(_) => Beat::Refused,
        }
    });
    let acted = *acted.borrow();
    (progress, acted)
}

/// Run every uncontrolled magpie one turn of the agency loop, on ticks that are a
/// non-zero multiple of [`HOARD_EVERY`]. A controlled bird (someone piloting it) is
/// left alone, mirroring [`wander`](crate::systems::wander). The bird is keyed by its
/// [`Hoarder`] need; its [`Curiosity`], if present, contends through the arbiter.
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

        // Metabolism moves both needs from the current world state: each rises while
        // unmet and falls while met, so relief is a property of the world (a shiny in
        // the nest, a shiny in hand), read here, never of the arbiter or the driver.
        metabolize(ctx.world, bird);

        // Drives -> arbiter (resumed from the persisted commitment) -> driver ->
        // perform. Commitment is what stops the two drives thrashing the bead.
        let Some((drive, goal)) = commit_and_select(ctx.world, bird, HYSTERESIS) else {
            continue;
        };
        let (progress, acted) = pursue_goal(ctx.world, bird, &goal.predicate);

        // Narrate only a beat that actually moved something (an already-satisfied goal
        // runs an empty plan and moves nothing), by the drive it served.
        if progress == Progress::Achieved
            && let Some(item) = acted
            && let Some(room) = ctx.world.enclosing_locus(bird)
        {
            let who = display_name(ctx.world, bird);
            let what = display_name(ctx.world, item);
            let text = match drive {
                Drive::Hoard => format!("{who} tucks {what} into its nest."),
                Drive::Admire => format!("{who} turns {what} over, admiring it."),
            };
            ctx.emit_locus(room, EventKind::Narration, text);
        }
    }
}

/// Move both of a bird's needs one scheduled tick: each need rises while its want is
/// unmet and falls while it is met. Satisfaction is read from the world here (a shiny
/// in the nest relieves the urge; a shiny in hand relieves the itch); the drives
/// themselves read only the resulting component.
fn metabolize(world: &mut World, bird: EntityId) {
    if let Some(urge) = world.get::<Hoarder>(bird).map(|h| h.urge) {
        let sated = shiny_in_nest(world, bird);
        world
            .insert(
                bird,
                Hoarder {
                    urge: step_need(urge, sated),
                },
            )
            .expect("a read Hoarder belongs to a live bird");
    }
    if let Some(itch) = world.get::<Curiosity>(bird).map(|c| c.itch) {
        let sated = holds_shiny(world, bird);
        world
            .insert(
                bird,
                Curiosity {
                    itch: step_need(itch, sated),
                },
            )
            .expect("a read Curiosity belongs to a live bird");
    }
}

/// One tick of a need's curve: cool toward zero while the want is met, warm toward the
/// ceiling while it is unmet.
fn step_need(level: u32, satisfied: bool) -> u32 {
    if satisfied {
        level.saturating_sub(COOL)
    } else {
        (level + WARM).min(NEED_MAX)
    }
}

/// Whether a shiny thing sits in the bird's nest (the hoard want, satisfied).
fn shiny_in_nest(world: &World, bird: EntityId) -> bool {
    world
        .target_of::<Nest>(bird)
        .is_some_and(|nest| world.contents(nest).iter().any(|&e| is_shiny(world, e)))
}

/// Whether the bird is holding a shiny thing (the admire want, satisfied).
fn holds_shiny(world: &World, bird: EntityId) -> bool {
    world.contents(bird).iter().any(|&e| is_shiny(world, e))
}

fn is_shiny(world: &World, entity: EntityId) -> bool {
    world.get::<Shiny>(entity).is_some()
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

    /// A single room holding a magpie, its (empty) nest chest, and a loose shiny bead,
    /// all co-located so the bird knows all three. `itch` seeds the admiring need; a
    /// zero leaves the bird a pure hoarder (the single-drive fixture), a high value
    /// makes it contend with hoarding (the two-drive oracle).
    fn fixture_with(itch: u32) -> Fixture {
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
            if itch > 0 {
                b.add(Curiosity { itch });
            }
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

    /// The single-drive fixture: a pure hoarder, no admiring need.
    fn fixture() -> Fixture {
        fixture_with(0)
    }

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    /// Run `hoard` at an explicit tick, returning its emitted outbound buffer.
    fn tick(world: &mut World, tick: u64) -> Vec<Outbound> {
        let affordances = musce::action::AffordanceRegistry::empty(world).unwrap();
        let mut out = Vec::new();
        let mut ctx = SystemCtx::new(
            world,
            &affordances,
            tick,
            SystemTime::UNIX_EPOCH,
            &[],
            &mut out,
        );
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

    /// The hoard drive, live: an idle magpie grows restless over scheduled ticks, then
    /// at the threshold plans and runs take-then-put through the real `perform` to stow
    /// the bead in its nest, and the urge is relieved as the hoard rests there. Nothing
    /// moves the bead but the arbiter/driver loop, and nothing moves the urge but the
    /// metabolism.
    #[test]
    fn an_idle_magpie_grows_restless_then_stows_a_shiny() {
        let mut f = fixture();

        // One sub-threshold tick: the urge rose but the bird has not acted yet.
        tick(&mut f.world, HOARD_EVERY);
        assert_eq!(urge_of(&f.world, f.magpie), 1);
        assert_eq!(f.world.container_of(f.bead), Some(f.room)); // still loose

        // Advance to the threshold tick. The bead ends up in the nest; the urge peaked
        // at the stow (relief is gradual, not an instant reset).
        let mut out = Vec::new();
        for n in 2..=THRESHOLD as u64 {
            out = tick(&mut f.world, HOARD_EVERY * n);
        }
        assert_eq!(
            f.world.container_of(f.bead),
            Some(f.nest),
            "stowed in the nest"
        );
        assert_eq!(
            urge_of(&f.world, f.magpie),
            THRESHOLD,
            "urge peaked at the stow"
        );
        assert!(
            room_narration(&out)
                .iter()
                .any(|t| t.contains("a magpie tucks a glass bead into its nest")),
            "stow narration, got: {:?}",
            room_narration(&out)
        );

        // State-based relief: with the bead resting in the nest, the next scheduled
        // tick cools the urge, and nothing disturbs the hoard.
        tick(&mut f.world, HOARD_EVERY * (THRESHOLD as u64 + 1));
        assert!(
            urge_of(&f.world, f.magpie) < THRESHOLD,
            "the resting hoard relieves the urge over ticks"
        );
        assert_eq!(
            f.world.container_of(f.bead),
            Some(f.nest),
            "the hoard is left in place"
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

        for n in 1..=THRESHOLD as u64 + 1 {
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
    /// pursuit abandons and the urge keeps climbing (the world never satisfies it).
    #[test]
    fn nothing_to_steal_leaves_it_restless() {
        let mut f = fixture();
        f.world.remove::<Shiny>(f.bead).unwrap(); // the only glittery thing loses its shine

        for n in 1..=THRESHOLD as u64 + 1 {
            tick(&mut f.world, HOARD_EVERY * n);
        }

        assert!(
            urge_of(&f.world, f.magpie) >= THRESHOLD,
            "no shiny within reach, so the urge is never relieved"
        );
        assert_eq!(f.world.container_of(f.bead), Some(f.room)); // nothing stowed
    }

    /// The arbiter earns its keep: with hoarding and admiring both pressing, the bead
    /// is pulled toward the nest and toward the hand at once. Cross-tick commitment
    /// makes the bird finish one before switching; without it the two drives thrash
    /// the bead between hand and nest almost every tick. The move count is the oracle:
    /// the committed bird moves the bead far less, while still serving both drives.
    #[test]
    fn commitment_stops_the_two_drives_thrashing_the_bead() {
        const TICKS: u64 = 24;

        // Treatment: the real loop, with the persisted incumbent and the hysteresis
        // band.
        let mut t = fixture_with(5);
        let mut treated_moves = 0;
        let mut visited_hand = false;
        let mut visited_nest = false;
        let mut prev = t.world.container_of(t.bead);
        for n in 1..=TICKS {
            tick(&mut t.world, HOARD_EVERY * n);
            let now = t.world.container_of(t.bead);
            if now != prev {
                treated_moves += 1;
            }
            visited_hand |= now == Some(t.magpie);
            visited_nest |= now == Some(t.nest);
            prev = now;
        }

        // Control: the same metabolism, drives, and pursuit, but no commitment (a fresh
        // zero-band arbiter each tick). Only the selection policy differs, so the move
        // gap is attributable to commitment alone.
        let mut c = fixture_with(5);
        let mut control_moves = 0;
        let mut prev = c.world.container_of(c.bead);
        for n in 1..=TICKS {
            tick_no_commitment(&mut c.world, HOARD_EVERY * n);
            let now = c.world.container_of(c.bead);
            if now != prev {
                control_moves += 1;
            }
            prev = now;
        }

        assert!(
            visited_hand && visited_nest,
            "commitment still serves both drives (held and stowed), not frozen: \
             hand={visited_hand} nest={visited_nest}"
        );
        assert!(treated_moves >= 2, "the committed bird still acts");
        assert!(
            control_moves >= 2 * treated_moves,
            "commitment cuts the thrash: committed {treated_moves} moves vs \
             uncommitted {control_moves}"
        );
    }

    /// The no-commitment control loop: the same shared metabolism, drives, and pursuit
    /// as `hoard`, but selecting with a fresh zero-band arbiter each tick and never
    /// persisting a commitment. This is the 2a behaviour the arbiter's commitment
    /// improves on, run side by side in the oracle above.
    fn tick_no_commitment(world: &mut World, tick: u64) {
        if tick == 0 || !tick.is_multiple_of(HOARD_EVERY) {
            return;
        }
        let birds: Vec<EntityId> = world
            .query::<(&Id, &Hoarder)>()
            .iter()
            .map(|(id, _)| id.0)
            .collect();
        for bird in birds {
            if world.target_of::<Controls>(bird).is_some() {
                continue;
            }
            metabolize(world, bird);
            let candidates = drive_goals(world, bird);
            if candidates.is_empty() {
                continue;
            }
            let goals: Vec<Goal> = candidates.iter().map(|(_, goal)| goal.clone()).collect();
            let Some(chosen) = Arbiter::new(0).select(&goals) else {
                continue;
            };
            let goal = candidates
                .into_iter()
                .find(|(_, goal)| goal.predicate == chosen.predicate)
                .map(|(_, goal)| goal)
                .expect("the chosen goal came from a candidate");
            pursue_goal(world, bird, &goal.predicate);
        }
    }
}
