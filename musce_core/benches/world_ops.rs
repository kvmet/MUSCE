//! Microbenchmarks for the core world's hot paths: containment lookups, the
//! move/relate path, the enclosing-locus walk, and full-world snapshot
//! serialization. These measure the in-memory ECS layer in isolation, with no
//! game content and no persistence I/O; the persistence crate benches the DB
//! round-trip separately. The scaling cases (`contents`, `snapshot`) sweep world
//! size so a regression that turns a linear cost superlinear shows up as a change
//! in slope, not just a single number.

use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use musce_core::hecs::EntityBuilder;
use musce_core::{Description, EntityId, Locus, Name, World};

/// A room (a `Locus`) holding `n` freshly spawned items, each named and
/// described, so every item carries a realistic handful of component rows.
fn room_with_items(n: usize) -> (World, EntityId, Vec<EntityId>) {
    let mut world = World::new();
    let room = {
        let mut b = EntityBuilder::new();
        b.add(Locus);
        b.add(Description("a room".into()));
        world.spawn(b)
    };
    let mut items = Vec::with_capacity(n);
    for i in 0..n {
        let mut b = EntityBuilder::new();
        b.add(Description("an item".into()));
        b.add(Name(format!("item{i}")));
        let id = world.spawn(b);
        world.move_entity(id, room).unwrap();
        items.push(id);
    }
    world.take_facts(); // discard the setup moves' facts
    (world, room, items)
}

/// A chain of nested containers `depth` deep, rooted in a locus; returns the
/// world and the innermost entity, whose `enclosing_locus` must walk the whole
/// chain to reach the locus.
fn nested_chain(depth: usize) -> (World, EntityId) {
    let mut world = World::new();
    let root = {
        let mut b = EntityBuilder::new();
        b.add(Locus);
        world.spawn(b)
    };
    let mut parent = root;
    let mut deepest = root;
    for _ in 0..depth {
        let child = {
            let mut b = EntityBuilder::new();
            b.add(Description("a box".into()));
            world.spawn(b)
        };
        world.move_entity(child, parent).unwrap();
        parent = child;
        deepest = child;
    }
    (world, deepest)
}

fn contents(c: &mut Criterion) {
    let mut group = c.benchmark_group("contents");
    for &n in &[100usize, 1_000, 10_000] {
        let (world, room, _items) = room_with_items(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(world.contents(room)));
        });
    }
    group.finish();
}

fn enclosing_locus(c: &mut Criterion) {
    let mut group = c.benchmark_group("enclosing_locus");
    for &depth in &[1usize, 8, 64] {
        let (world, deepest) = nested_chain(depth);
        group.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, _| {
            b.iter(|| black_box(world.enclosing_locus(deepest)));
        });
    }
    group.finish();
}

fn move_entity(c: &mut Criterion) {
    // Toggle one item between two rooms so each iteration does a real reparent
    // (unrelate + cycle-check + relate + fact emission), not a no-op self-move.
    let mut world = World::new();
    let spawn_locus = |world: &mut World| {
        let mut b = EntityBuilder::new();
        b.add(Locus);
        world.spawn(b)
    };
    let a = spawn_locus(&mut world);
    let d = spawn_locus(&mut world);
    let item = {
        let mut b = EntityBuilder::new();
        b.add(Description("x".into()));
        world.spawn(b)
    };
    world.move_entity(item, a).unwrap();
    world.take_facts();

    let mut into_a = false;
    c.bench_function("move_entity/reparent", |b| {
        b.iter(|| {
            let target = if into_a { a } else { d };
            into_a = !into_a;
            world.move_entity(item, target).unwrap();
            world.take_facts(); // keep the fact buffer from growing across iters
        });
    });
}

fn despawn_reparent(c: &mut Criterion) {
    // Despawning a full container reparents every child up one level, so this
    // measures the cascade cost, not a bare row delete. A fresh world per batch
    // keeps each measured despawn identical (the setup is not timed).
    let mut group = c.benchmark_group("despawn_reparent");
    for &n in &[100usize, 1_000] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter_batched(
                || {
                    let (mut world, room, _) = room_with_items(0);
                    let bag = {
                        let mut eb = EntityBuilder::new();
                        eb.add(Description("a bag".into()));
                        world.spawn(eb)
                    };
                    world.move_entity(bag, room).unwrap();
                    for i in 0..n {
                        let mut eb = EntityBuilder::new();
                        eb.add(Name(format!("c{i}")));
                        let child = world.spawn(eb);
                        world.move_entity(child, bag).unwrap();
                    }
                    world.take_facts();
                    (world, bag)
                },
                |(mut world, bag)| {
                    world.despawn(bag);
                    black_box(world.take_facts())
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn snapshot(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot");
    for &n in &[100usize, 1_000, 10_000] {
        let (mut world, _room, _items) = room_with_items(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(world.snapshot()));
        });
    }
    group.finish();
}

fn world_load(c: &mut Criterion) {
    // The inverse of `snapshot`: deserialize blobs into a fresh world and rebuild
    // the reverse relation lists. This is the CPU half of boot load (the DB read is
    // benched separately in `musce_persistence`); it exercises the reverse-relation
    // rebuild that persistence.md calls one O(n) pass, so the slope here is that
    // claim's check.
    let mut group = c.benchmark_group("world_load");
    for &n in &[100usize, 1_000, 10_000] {
        let (mut world, _room, _items) = room_with_items(n);
        let snap = world.snapshot();
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter_batched(
                World::new,
                |mut w| {
                    w.load(black_box(&snap.entities), snap.next_id).unwrap();
                    w
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    contents,
    enclosing_locus,
    move_entity,
    despawn_reparent,
    snapshot,
    world_load
);
criterion_main!(benches);
