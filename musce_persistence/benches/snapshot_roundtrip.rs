//! Benchmarks the world save/load round-trip through the SQLite backend, swept
//! across world size. This is the bench that watches the per-component-row cost:
//! `save` writes every component as a row inside a transaction (batched multi-row
//! inserts, chunked under the bind-variable limit), so its slope against entity
//! count is the concrete evidence for or against the persistence-layout redesign
//! (the EAV split on the roadmap). An in-memory database is used so the numbers
//! isolate serialization + SQL work from disk latency.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use musce_core::hecs::EntityBuilder;
use musce_core::{Description, Locus, Name, Snapshot, World};
use musce_persistence::{Persistence, SqliteStore};
use tokio::runtime::Runtime;

/// A snapshot of a world holding `n` items in a room, each carrying a description,
/// a name, and its containment link, so every entity persists several rows.
fn snapshot_of(n: usize) -> Snapshot {
    let mut world = World::new();
    let room = {
        let mut b = EntityBuilder::new();
        b.add(Locus);
        b.add(Description("a room".into()));
        world.spawn(b)
    };
    for i in 0..n {
        let mut b = EntityBuilder::new();
        b.add(Description("an item".into()));
        b.add(Name(format!("item{i}")));
        let id = world.spawn(b);
        world.move_entity(id, room).unwrap();
    }
    world.snapshot()
}

/// A fresh in-memory store with the world tables created.
fn fresh_store(rt: &Runtime) -> SqliteStore {
    rt.block_on(async {
        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();
        store
    })
}

fn save(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("save");
    group.sample_size(20); // each sample writes thousands of rows; keep it snappy
    for &n in &[100usize, 1_000, 10_000] {
        let snap = snapshot_of(n);
        let store = fresh_store(&rt);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            // Re-saving the same snapshot upserts the same rows, so each iteration
            // does identical write work (delete-then-insert per entity).
            b.iter(|| rt.block_on(store.save(black_box(&snap))).unwrap());
        });
    }
    group.finish();
}

fn load(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("load");
    group.sample_size(20);
    for &n in &[100usize, 1_000, 10_000] {
        let snap = snapshot_of(n);
        let store = fresh_store(&rt);
        rt.block_on(store.save(&snap)).unwrap();
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| black_box(rt.block_on(store.load()).unwrap()));
        });
    }
    group.finish();
}

criterion_group!(benches, save, load);
criterion_main!(benches);
