//! Scripted timed behavior: the reference game's sequence layer. A **program** is
//! a step list (a `Steps` component) living on its own entity, shared and
//! referenced by id; an **instance** is a cursor over a program, carried on the
//! acting entity as a `Sequences(Vec<Instance>)` component and advanced by the
//! `sequence_sweep` system each tick. A step holds an `Intent` (a rule-checked
//! verb, not a raw `Action`), so a scripted actor runs the same gameplay rules a
//! player does. This is game content, not engine mechanism: the engine provides
//! only the generic persisted-component plumbing. See
//! `docs/architecture/sequences.md`.

use musce_action::{Action, SystemCtx};
use musce_core::{EntityId, Id, NamedComponent, World};
use serde::{Deserialize, Serialize};

use crate::commit_or_log;
use crate::names::{self, Scope, display_name};
use crate::verbs::{MoveOutcome, do_move};
use musce_proto::EventKind;

/// What a step does when it fires. A rule-checked intent (resolved and vetoed the
/// same way a player command is), not a raw structural `Action`. The MVP set is
/// deliberately two: a rule-checked `Move` and a rule-free self-`Destroy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Intent {
    /// Traverse the exit named `dir` out of the carrier's room, through the shared
    /// `do_move` path (so a locked exit vetoes a scripted mover too). A blocked or
    /// missing exit is a no-op beat; the sequence still advances.
    Move { dir: String },
    /// Despawn the carrier itself. Rule-free, the terminal beat of a finite
    /// sequence (a burning torch). The emitted `Fact::Destroyed` is what the
    /// `death_cry` reaction narrates one tick later.
    Destroy,
}

/// One beat of a program: the intent to run and the delay before it fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    /// Ticks to wait before THIS step fires. The inter-step delay is how "wait"
    /// beats are expressed; there is no separate wait intent.
    pub delay: u32,
    pub intent: Intent,
}

/// A program: the shared step list, carried on its own entity and referenced by an
/// instance's `program` id. Persisted, so a program survives a reload.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Steps(pub Vec<Step>);

impl NamedComponent for Steps {
    const TAG: &'static str = "steps";
}

/// A running cursor over a program, on the acting entity. Persisted, so a
/// half-played sequence resumes mid-loop after a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    /// The program entity whose `Steps` this instance plays.
    pub program: EntityId,
    /// Index of the next step to fire.
    pub cursor: usize,
    /// Absolute tick at which `steps[cursor]` fires.
    pub next_at: u64,
    /// Off the end, replay from the top instead of ending.
    pub repeat: bool,
}

/// The concurrent sequences an entity is running. A `Vec` because hecs is
/// one-component-per-type, so one entity running two behaviors at once (an effect
/// plus a patrol) is a list, not N components. The sweep iterates it, fires due
/// instances, and retains the unfinished ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sequences(pub Vec<Instance>);

impl NamedComponent for Sequences {
    const TAG: &'static str = "sequences";
}

/// Register the sequence components on a fresh world, before load or seed. Called
/// through the game's `register` so a half-played sequence deserializes.
pub fn register(world: &mut World) {
    world.register_component::<Steps>();
    world.register_component::<Sequences>();
}

/// Attach a fresh instance of `program` to `carrier`, validating the one structural
/// hazard: a repeating sequence whose every step has zero delay would loop forever
/// inside a single tick, so a repeat is required to have positive total cycle delay
/// (enforced here, at attach, making the infinite loop impossible rather than caught
/// by a per-tick fuse). Seed-only for now; a runtime attach verb would reuse this
/// and pass the current tick instead of boot.
pub fn attach(
    world: &mut World,
    carrier: EntityId,
    program: EntityId,
    repeat: bool,
) -> Result<(), &'static str> {
    let steps = program_steps(world, program).ok_or("program entity carries no Steps")?;
    if steps.is_empty() {
        return Err("a program must have at least one step");
    }
    if repeat && cycle_delay(&steps) == 0 {
        return Err("a repeating sequence must have positive total cycle delay");
    }

    let inst = Instance {
        program,
        cursor: 0,
        // Attach at boot (tick 0): the first step fires after its own delay.
        next_at: steps[0].delay as u64,
        repeat,
    };
    push_instance(world, carrier, inst);
    Ok(())
}

/// Sum of a program's per-step delays, the cycle length a repeat loops over.
fn cycle_delay(steps: &[Step]) -> u64 {
    steps.iter().map(|s| s.delay as u64).sum()
}

/// Append `inst` to the carrier's `Sequences`, starting the component if absent.
/// Direct component access, not `execute(SetComponent)`: cursor bookkeeping is
/// system-internal state, kept out of the action funnel (and out of a future
/// action journal), the same way the sweep advances it.
fn push_instance(world: &mut World, carrier: EntityId, inst: Instance) {
    let Some(e) = world.index().get(carrier) else {
        return;
    };
    if world.has::<Sequences>(carrier) {
        if let Ok(mut seqs) = world.ecs().get::<&mut Sequences>(e) {
            seqs.0.push(inst);
        }
    } else {
        world.insert(carrier, Sequences(vec![inst]));
    }
}

/// Advance every running sequence one tick: fire each instance whose next beat is
/// due, advance its cursor, and reschedule it (or remove it off the end, or reset
/// it if `repeat`). Carriers are collected first because a beat mutates the world
/// the sweep would otherwise iterate, and a beat can despawn its own carrier.
pub fn sequence_sweep(ctx: &mut SystemCtx) {
    let carriers: Vec<(EntityId, Sequences)> = ctx
        .world
        .ecs()
        .query::<(&Id, &Sequences)>()
        .iter()
        .map(|(id, seqs)| (id.0, seqs.clone()))
        .collect();

    for (carrier, seqs) in carriers {
        let mut retained: Vec<Instance> = Vec::with_capacity(seqs.0.len());
        let mut carrier_alive = true;

        for inst in seqs.0 {
            if inst.next_at > ctx.tick {
                retained.push(inst); // not due yet, unchanged
                continue;
            }
            match advance(ctx, carrier, inst) {
                Advance::Retain(advanced) => retained.push(advanced),
                Advance::Finished => {} // ran off the end, dropped
                Advance::CarrierGone => {
                    // A beat despawned the carrier; its remaining instances died
                    // with it. Stop, and do not write back to a dead entity.
                    carrier_alive = false;
                    break;
                }
            }
        }

        if carrier_alive {
            write_back(ctx.world, carrier, retained);
        }
    }
}

/// The outcome of advancing one due instance through its beats this tick.
enum Advance {
    /// Still running: keep it, with its cursor and `next_at` advanced.
    Retain(Instance),
    /// A one-shot ran off the end: drop it.
    Finished,
    /// A beat despawned the carrier: abandon the whole carrier this tick.
    CarrierGone,
}

/// Fire one due instance, then any further steps that are also due this tick (a
/// 0-delay burst). The attach guard makes a repeating all-zero-delay cycle
/// impossible, so this cannot spin forever.
fn advance(ctx: &mut SystemCtx, carrier: EntityId, mut inst: Instance) -> Advance {
    let Some(steps) = program_steps(ctx.world, inst.program) else {
        // Program gone or carries no Steps: the instance can't play, so drop it.
        return Advance::Finished;
    };
    if inst.cursor >= steps.len() {
        // A corrupt cursor (only reachable from a bad persisted blob; the loop
        // keeps it in range otherwise). Drop the instance rather than panic the
        // long-lived sim, but log it loud.
        tracing::error!(
            cursor = inst.cursor,
            len = steps.len(),
            "sequence cursor out of range"
        );
        return Advance::Finished;
    }

    loop {
        let intent = steps[inst.cursor].intent.clone();
        fire(ctx, carrier, &intent);

        // The beat may have despawned the carrier (a self-Destroy). Its Sequences
        // went with it, so there is nothing to advance or write back.
        if ctx.world.index().get(carrier).is_none() {
            return Advance::CarrierGone;
        }

        inst.cursor += 1;
        if inst.cursor >= steps.len() {
            if inst.repeat {
                inst.cursor = 0;
            } else {
                return Advance::Finished;
            }
        }

        inst.next_at = ctx.tick + steps[inst.cursor].delay as u64;
        if inst.next_at > ctx.tick {
            return Advance::Retain(inst);
        }
        // The next step is also due this tick (0 delay): fire it inline.
    }
}

/// Run one beat's intent against the carrier. A `Move` runs through the shared
/// `do_move` rule path (so a scripted mover is vetoed exactly as a player is) and
/// narrates to the rooms it leaves and enters; a `Destroy` despawns the carrier.
fn fire(ctx: &mut SystemCtx, carrier: EntityId, intent: &Intent) {
    match intent {
        Intent::Move { dir } => {
            let Some(exit) = names::resolve(ctx.world, carrier, Scope::Exits, dir) else {
                return; // no such exit out of here: a no-op beat
            };
            let who = display_name(ctx.world, carrier);
            match do_move(ctx.world, carrier, exit) {
                MoveOutcome::Moved {
                    from,
                    dest,
                    direction,
                } => {
                    if let Some(from) = from {
                        ctx.emit_locus(
                            from,
                            EventKind::Narration,
                            format!("{who} leaves {direction}."),
                        );
                    }
                    ctx.emit_locus(dest, EventKind::Narration, format!("{who} arrives."));
                }
                // Blocked (a locked exit) or half-wired: a no-op beat, no narration.
                MoveOutcome::NoDestination | MoveOutcome::Blocked(_) => {}
            }
        }
        Intent::Destroy => {
            // Rule-free self-destruction. Cannot structurally fail for a live
            // entity; logged loud rather than swallowed if it ever does.
            commit_or_log(
                ctx.world,
                Action::Destroy { entity: carrier },
                "sequence: destroy beat",
            );
        }
    }
}

/// Overwrite the carrier's `Sequences` with the instances still running, or remove
/// the component when none remain. Skipped entirely if the carrier was despawned.
fn write_back(world: &mut World, carrier: EntityId, retained: Vec<Instance>) {
    if retained.is_empty() {
        world.remove::<Sequences>(carrier);
    } else {
        world.insert(carrier, Sequences(retained));
    }
}

/// A program's steps, cloned out so the sweep can index them while it mutates the
/// world. `None` if the program entity is gone or carries no `Steps`.
fn program_steps(world: &World, program: EntityId) -> Option<Vec<Step>> {
    world
        .entity(program)
        .and_then(|er| er.get::<&Steps>().map(|s| s.0.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exits::{LeadsFrom, LeadsTo};
    use crate::kinds::{Creature, Exit, Item};
    use musce_action::Outbound;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Description, Locus, Name};
    use musce_proto::Audience;
    use std::time::SystemTime;

    /// Ticks per patrol step in the tests. The first beat fires at `next_at == STEP`
    /// (attach treats seed as tick 0).
    const STEP: u32 = 2;

    struct Fixture {
        world: World,
        carrier: EntityId,
        a: EntityId,
        b: EntityId,
    }

    /// Rooms A <-> B (north out of A, south out of B), with a carrier standing in A.
    /// No sequence attached yet; each test attaches what it exercises.
    fn fixture() -> Fixture {
        let mut world = World::new();
        // The full game register: exits (`LeadsFrom`/`LeadsTo`) are game-registered
        // now, so a world that must snapshot its exits needs the game hook, not just
        // the local sequence types.
        crate::systems::register(&mut world);

        let a = spawn(&mut world, |b| {
            b.add(Locus);
            b.add(Description("room A".into()));
        });
        let b = spawn(&mut world, |b| {
            b.add(Locus);
            b.add(Description("room B".into()));
        });
        link(&mut world, a, b, "north");
        link(&mut world, b, a, "south");

        let carrier = spawn(&mut world, |b| {
            b.add(Creature);
            b.add(Description("a clockwork sentry".into()));
        });
        world.move_entity(carrier, a).unwrap();

        Fixture {
            world,
            carrier,
            a,
            b,
        }
    }

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    fn link(w: &mut World, from: EntityId, to: EntityId, dir: &str) {
        let exit = spawn(w, |b| {
            b.add(Exit);
            b.add(Name(dir.into()));
        });
        w.relate::<LeadsFrom>(exit, from).unwrap();
        w.relate::<LeadsTo>(exit, to).unwrap();
    }

    /// A two-step patrol program (north, south), each step waiting `STEP` ticks.
    fn patrol_program(w: &mut World) -> EntityId {
        spawn(w, |b| {
            b.add(Steps(vec![
                Step {
                    delay: STEP,
                    intent: Intent::Move {
                        dir: "north".into(),
                    },
                },
                Step {
                    delay: STEP,
                    intent: Intent::Move {
                        dir: "south".into(),
                    },
                },
            ]));
        })
    }

    fn sweep(world: &mut World, tick: u64) -> Vec<Outbound> {
        let mut out = Vec::new();
        let mut ctx = SystemCtx::new(world, tick, SystemTime::UNIX_EPOCH, &[], &mut out);
        sequence_sweep(&mut ctx);
        out
    }

    fn instance(world: &World, carrier: EntityId) -> Option<Instance> {
        world
            .entity(carrier)
            .and_then(|er| er.get::<&Sequences>().map(|s| s.0.first().cloned()))
            .flatten()
    }

    fn room_narration(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Locus(_)))
            .map(|o| o.event.text.clone())
            .collect()
    }

    #[test]
    fn attach_rejects_a_zero_delay_repeat() {
        let mut f = fixture();
        // A repeating program whose every step has zero delay would spin forever in
        // one tick; the guard refuses it at attach.
        let prog = spawn(&mut f.world, |b| {
            b.add(Steps(vec![Step {
                delay: 0,
                intent: Intent::Move {
                    dir: "north".into(),
                },
            }]));
        });
        assert!(attach(&mut f.world, f.carrier, prog, true).is_err());
        // The same program as a one-shot is fine.
        assert!(attach(&mut f.world, f.carrier, prog, false).is_ok());
    }

    #[test]
    fn fires_a_due_beat_and_advances_the_cursor() {
        let mut f = fixture();
        let prog = patrol_program(&mut f.world);
        attach(&mut f.world, f.carrier, prog, true).unwrap();

        // Not due before STEP: the carrier stays put.
        let out = sweep(&mut f.world, STEP as u64 - 1);
        assert_eq!(f.world.enclosing_locus(f.carrier), Some(f.a));
        assert!(room_narration(&out).is_empty());

        // At STEP the first beat fires: the carrier moves north and the cursor and
        // schedule advance to the second step.
        let out = sweep(&mut f.world, STEP as u64);
        assert_eq!(f.world.enclosing_locus(f.carrier), Some(f.b));
        let inst = instance(&f.world, f.carrier).unwrap();
        assert_eq!(inst.cursor, 1);
        assert_eq!(inst.next_at, (STEP * 2) as u64);
        let lines = room_narration(&out);
        assert!(lines.iter().any(|t| t.contains("leaves north")));
        assert!(lines.iter().any(|t| t.contains("arrives")));
    }

    #[test]
    fn repeats_by_resetting_the_cursor_off_the_end() {
        let mut f = fixture();
        let prog = patrol_program(&mut f.world);
        attach(&mut f.world, f.carrier, prog, true).unwrap();

        sweep(&mut f.world, STEP as u64); // north, cursor -> 1
        sweep(&mut f.world, (STEP * 2) as u64); // south, cursor wraps -> 0

        assert_eq!(f.world.enclosing_locus(f.carrier), Some(f.a)); // back home
        let inst = instance(&f.world, f.carrier).unwrap();
        assert_eq!(inst.cursor, 0); // looped, did not end
        assert_eq!(inst.next_at, (STEP * 3) as u64);
    }

    #[test]
    fn a_one_shot_is_removed_off_the_end() {
        let mut f = fixture();
        // A finite one-step program (no repeat): after its beat it should be gone.
        let prog = spawn(&mut f.world, |b| {
            b.add(Steps(vec![Step {
                delay: STEP,
                intent: Intent::Move {
                    dir: "north".into(),
                },
            }]));
        });
        attach(&mut f.world, f.carrier, prog, false).unwrap();

        sweep(&mut f.world, STEP as u64);

        assert_eq!(f.world.enclosing_locus(f.carrier), Some(f.b)); // it ran
        // Off the end of a one-shot: the Sequences component is removed entirely.
        assert!(!f.world.has::<Sequences>(f.carrier));
    }

    #[test]
    fn a_destroy_beat_despawns_its_own_carrier_without_panicking() {
        let mut f = fixture();
        // The torch case: the terminal beat removes the entity holding the
        // Sequences. The sweep must not write back to the dead carrier.
        let prog = spawn(&mut f.world, |b| {
            b.add(Steps(vec![Step {
                delay: STEP,
                intent: Intent::Destroy,
            }]));
        });
        attach(&mut f.world, f.carrier, prog, false).unwrap();

        sweep(&mut f.world, STEP as u64);

        assert!(f.world.entity(f.carrier).is_none()); // gone, no panic
        // The despawn emitted a structural fact for a reaction (e.g. death_cry).
        assert!(!f.world.take_facts().is_empty());
    }

    #[test]
    fn a_torch_carrier_can_also_be_an_item() {
        // The seeded torch is an Item, not a Creature; the sweep does not care what
        // kind the carrier is, only that it holds Sequences.
        let mut f = fixture();
        let torch = spawn(&mut f.world, |b| {
            b.add(Item);
            b.add(Description("a guttering torch".into()));
        });
        f.world.move_entity(torch, f.a).unwrap();
        let prog = spawn(&mut f.world, |b| {
            b.add(Steps(vec![Step {
                delay: STEP,
                intent: Intent::Destroy,
            }]));
        });
        attach(&mut f.world, torch, prog, false).unwrap();

        sweep(&mut f.world, STEP as u64);
        assert!(f.world.entity(torch).is_none());
    }

    #[test]
    fn a_mid_loop_sequence_survives_a_reload() {
        let mut f = fixture();
        let prog = patrol_program(&mut f.world);
        attach(&mut f.world, f.carrier, prog, true).unwrap();

        // Advance partway: fire the north beat, leaving the cursor on the south step.
        sweep(&mut f.world, STEP as u64);
        let before = instance(&f.world, f.carrier).unwrap();
        assert_eq!(before.cursor, 1);

        // Snapshot and reload into a fresh world that registers the sequence types.
        let snap = f.world.snapshot();
        let mut reloaded = World::new();
        crate::systems::register(&mut reloaded);
        reloaded.load(&snap.entities, snap.next_id).unwrap();

        // The instance (cursor, next_at, program id) round-tripped, and the program
        // entity it points at came back too (ids are stable across load).
        let after = instance(&reloaded, f.carrier).unwrap();
        assert_eq!(after.cursor, before.cursor);
        assert_eq!(after.next_at, before.next_at);
        assert_eq!(after.program, prog);

        // Resumes mid-loop: the scheduled south beat fires in the reloaded world.
        sweep(&mut reloaded, before.next_at);
        assert_eq!(reloaded.enclosing_locus(f.carrier), Some(f.a)); // south, back to A
        assert_eq!(instance(&reloaded, f.carrier).unwrap().cursor, 0); // looped
    }
}
