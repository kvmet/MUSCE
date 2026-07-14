//! Per-tick simulation cost: the work the tick loop does every beat when no
//! player is typing. It runs the reference game's real system set (`wander`, the
//! sequence sweep, and the `death_cry` reaction) exactly as the host's
//! `run_systems` does: drain the tick's facts once, run each system over a
//! `SystemCtx`, and audience-resolve its output. Each iteration runs a fixed span
//! of ticks over a fresh seed and reports per-tick throughput, so the number is a
//! representative sim-second rather than the anomalously light first tick.
//!
//! This measures the seed world's fixed population. Watching per-tick cost scale
//! with entity count needs a way to seed N creatures, which the reference game
//! does not expose; add that hook (and a size sweep here) if the flat number ever
//! looks like it matters.

use std::hint::black_box;
use std::time::SystemTime;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use musce::action::{Actors, Outbound, System, SystemCtx, resolve};
use musce::world::World;
use musce::{Register, Seed};

/// How many ticks one measured iteration advances. Long enough to cover the
/// sentry's patrol cadence and the torch's burn-out, so the average blends the
/// active and idle beats of a real sim second.
const TICKS: u64 = 100;

/// A freshly registered and seeded reference world, plus the game's systems.
fn setup() -> (World, Vec<System>) {
    let game = musce_ref::game();
    let register: Register = game.register;
    let seed: Seed = game.seed;
    let mut world = World::new();
    register(&mut world);
    seed(&mut world);
    (world, game.systems)
}

/// One tick of the system pipeline, mirroring the host's `run_systems`: facts are
/// drained once up front so every system in the tick sees the same batch, and each
/// system's output is resolved against the (here empty) audience index.
fn tick(world: &mut World, systems: &[System], actors: &Actors, tick: u64) {
    let facts = world.take_facts();
    for system in systems {
        let mut out: Vec<Outbound> = Vec::new();
        {
            let mut sctx = SystemCtx::new(world, tick, SystemTime::UNIX_EPOCH, &facts, &mut out);
            system(&mut sctx);
        }
        for ob in out {
            resolve(world, actors, ob, &mut |o| {
                black_box(o);
            });
        }
    }
}

fn tick_work(c: &mut Criterion) {
    // No connections: audience resolution is exercised but drops every event,
    // isolating the systems' world work from delivery.
    let actors = Actors::default();
    let mut group = c.benchmark_group("tick_work");
    group.throughput(Throughput::Elements(TICKS));
    group.bench_function("seed_world", |b| {
        b.iter_batched_ref(
            setup,
            |(world, systems)| {
                for t in 1..=TICKS {
                    tick(world, systems, &actors, t);
                }
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, tick_work);
criterion_main!(benches);
