//! What a secondary index buys: retrieval time. A range query returns every room
//! whose cell falls in the queried region. The index enumerates the region's cell
//! keys and unions their buckets (O(keys + results)); the naive baseline scans
//! every room and keeps those in the region (O(world)). Both return the same set
//! in arbitrary order, so the numbers are pure retrieval cost, no sorting or
//! distance math. The crossover is the world size past which the index wins.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use musce::index::IndexRegistry;
use musce::world::hecs::EntityBuilder;
use musce::world::{EntityId, Id, World};
use musce_ref::spatial::{CELL, Xyz, near, register_indexes};

/// Coordinate spacing between adjacent lattice rooms, so a fixed-radius region
/// holds a bounded, world-size-independent set of rooms.
const SPACING: i64 = 4;

/// Query radius, held fixed across world sizes so only the world grows.
const RADIUS: i64 = 12;

/// A room's cell, mirroring `spatial`'s private `cell_of` so the naive baseline
/// selects the exact same region the index retrieves.
fn cell_of(p: &Xyz) -> (i64, i64, i64) {
    (
        p.x.div_euclid(CELL),
        p.y.div_euclid(CELL),
        p.z.div_euclid(CELL),
    )
}

/// The full-scan baseline: scan every room and keep those whose cell lies in the
/// same region `near` retrieves. O(world), the cost the index competes with.
fn naive_region(world: &World, center: &Xyz, radius: i64) -> Vec<EntityId> {
    let span = radius.div_euclid(CELL) + 1;
    let (cx, cy, cz) = cell_of(center);
    world
        .ecs()
        .query::<(&Id, &Xyz)>()
        .iter()
        .filter_map(|(id, p)| {
            let (ex, ey, ez) = cell_of(p);
            let inside =
                (ex - cx).abs() <= span && (ey - cy).abs() <= span && (ez - cz).abs() <= span;
            inside.then_some(id.0)
        })
        .collect()
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
            b.iter(|| black_box(naive_region(&world, &center, RADIUS)));
        });
    }
    group.finish();
}

criterion_group!(benches, index_query);
criterion_main!(benches);
