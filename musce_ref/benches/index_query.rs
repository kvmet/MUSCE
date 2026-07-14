//! What a secondary index buys, and where it starts to pay. Compares the indexed
//! `spatial::near` (scan only the cells a query sphere touches) against a naive
//! full scan (distance-test every room), over worlds from 100 to 100k rooms.
//!
//! Rooms sit on a cube lattice with a fixed spacing, so a fixed-radius query keeps
//! a roughly constant neighborhood while the world grows: the naive scan is
//! O(world), the indexed query is O(neighborhood). The crossover, the point where
//! "indexed" overtakes "naive_scan", is the world size past which building the
//! index earns its keep. Below it the scan wins and an index is not worth it.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use musce::index::IndexRegistry;
use musce::world::hecs::EntityBuilder;
use musce::world::{EntityId, Id, World};
use musce_ref::spatial::{Xyz, dist2, near, register_indexes};

/// Coordinate spacing between adjacent lattice rooms. With `spatial::CELL = 10`
/// this puts a few rooms per cell, so a fixed-radius query returns a bounded
/// neighborhood no matter how large the world gets.
const SPACING: i64 = 4;

/// Query radius, held fixed across world sizes so only the world grows.
const RADIUS: i64 = 12;

/// The full-scan baseline the index competes with: distance-test every room, no
/// index. Mirrors `spatial::near`'s output (within-radius, nearest first) so the
/// two do equal work per result.
fn naive_near(world: &World, center: &Xyz, radius: i64) -> Vec<(EntityId, i64)> {
    let r2 = radius * radius;
    let mut out: Vec<(EntityId, i64)> = world
        .ecs()
        .query::<(&Id, &Xyz)>()
        .iter()
        .filter_map(|(id, p)| {
            let d2 = dist2(center, p);
            (d2 <= r2).then_some((id.0, d2))
        })
        .collect();
    out.sort_by_key(|(_, d2)| *d2);
    out
}

/// A world of `n` rooms on a cube lattice, its spatial index built, plus a query
/// point at the lattice center. Setup only; not measured.
fn build(n: usize) -> (World, Xyz) {
    let mut world = World::new();
    let side = (n as f64).cbrt().ceil() as i64;
    let mut count = 0usize;
    'fill: for i in 0..side {
        for j in 0..side {
            for k in 0..side {
                if count >= n {
                    break 'fill;
                }
                let mut b = EntityBuilder::new();
                b.add(Xyz {
                    x: i * SPACING,
                    y: j * SPACING,
                    z: k * SPACING,
                });
                world.spawn(b);
                count += 1;
            }
        }
    }

    let mut reg = IndexRegistry::default();
    register_indexes(&mut reg);
    reg.baseline(&world);
    world.insert_resource(reg);

    let mid = (side / 2) * SPACING;
    (
        world,
        Xyz {
            x: mid,
            y: mid,
            z: mid,
        },
    )
}

fn index_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("near_query");
    for n in [100usize, 1_000, 10_000, 100_000] {
        let (world, center) = build(n);
        group.bench_with_input(BenchmarkId::new("indexed", n), &n, |b, _| {
            b.iter(|| black_box(near(&world, &center, RADIUS)));
        });
        group.bench_with_input(BenchmarkId::new("naive_scan", n), &n, |b, _| {
            b.iter(|| black_box(naive_near(&world, &center, RADIUS)));
        });
    }
    group.finish();
}

criterion_group!(benches, index_query);
criterion_main!(benches);
