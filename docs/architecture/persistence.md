# Persistence

The database is the **saved** form of the world; the in-memory world is the live
truth. The DB is written and read, never queried during runtime. That single
decision shapes everything here. See
[musce_persistence](../../musce_persistence/src/lib.rs) and
[snapshot.rs](../../musce_core/src/snapshot.rs).

## Snapshot model

A `Snapshot` is a point-in-time save payload produced on the sim thread and handed
to the persistence thread, which does the actual writes, so the sim never blocks on
the database for the write. Building the snapshot is **not** free, though: it is a
full serialize of every live entity on the sim thread, O(entities) of allocation
and JSON work each save, which surfaces as a periodic tick-time spike that grows
with the world. Dirty-tracked partial snapshots are the fix (see below).

What is and isn't serialized:

- **Persisted:** real component data and forward relation links (`RelTarget`).
- **Not persisted:** reverse relation lists (`RelSources`) and the `EntityId`
  index. Both are derived and rebuilt on load. This shrinks the save and removes
  a class of "the two sides disagree on disk" bugs.

Load is order-independent: forward links are `EntityId`s resolved through the
index, so a target need not exist when its source is spawned. After all entities
are spawned, reverse lists are rebuilt in one O(n) pass.

## Storage shape

Because the DB is never queried at runtime, components do not need to be
SQL-queryable. So: **one JSON blob per entity**, plus a couple of extracted
columns for future shard-scoped loading.

```sql
entities (
  entity_id  INTEGER PRIMARY KEY,   -- the global EntityId
  zone       INTEGER,               -- extracted for shard-scoped load (unused yet)
  data       TEXT,                  -- components as JSON (Postgres: JSONB)
  updated_at INTEGER
)
meta (key TEXT PRIMARY KEY, value TEXT)  -- next_id high-water mark, schema version
```

The blob is **JSON**: human-readable for debugging while the schema churns, and
Postgres can store it as JSONB (admin-queryable) later. Switching to a binary
format is an option only if save size ever becomes a real problem.

## The save / confirm contract

Deletes are the fragile part of save. A despawned entity is already gone from the
live world, so the pending-delete set is the *only* record of it. Therefore:

- `snapshot()` **copies** the pending deletes; it does not clear them.
- The persistence layer's `save()` applies upserts and deletes in one
  transaction.
- Only after a successful save does the caller invoke
  `World::confirm_saved(&snapshot.deletes)`, which drops exactly those ids from
  the pending set. Deletes that accumulated since the snapshot are preserved.

So a failed save loses nothing: the deletes ride along in the next snapshot. (A
command journal for sub-snapshot crash recovery is deferred; this contract is the
minimum that keeps deletes durable across a save failure.)

Upserts are idempotent (the whole live world is written each save), so a failed
save simply re-writes everything next time. Dirty-tracked partial snapshots are
deferred until full-snapshot size hurts.

## ID allocation

`EntityId`s come from a monotonic counter whose high-water mark lives in `meta`.
The DB owns it, which also pre-solves shard allocation: a future hub hands
disjoint id ranges to shards from the same source.

## Integrity and evolution

- On load, the DB primary key and the entity's own `Id` component are checked to
  agree (a `debug_assert`), catching corrupt or wrongly-keyed blobs instead of
  letting index and component silently diverge.
- An unknown component tag on load is a hard error, not a silent skip, and a load
  error is **fatal**: the runtime refuses to boot rather than run an empty world.
  Running empty would reissue ids from 1 that the next save would write over the
  still-stored entities, so refusing to boot is what keeps a load failure from
  becoming data loss. Surfacing the mismatch beats silently dropping data.
- **Schema version and the migration seam.** Every save stamps a `schema_version`
  into `meta` (`SCHEMA_VERSION` in `musce_persistence`). On load, the stored
  version is compared against the current one and the blobs pass through a
  migration seam (`migrate_blobs` in `musce_host`) before they are deserialized.
  Bumping `SCHEMA_VERSION` and adding a transform keyed by the version it migrates
  from is how a renamed or reshaped component lands without breaking old saves: the
  transform is a function you write at the seam, not surgery on the load path. The
  version marker exists from the start precisely so the *first* migration is
  possible; retrofitting versioning onto already-written worlds is the harder
  problem it avoids. No transforms exist yet (the schema has only ever been at
  version 1), and the seam is a no-op for a current-version world; a world written
  before versioning existed has no marker and is read as current (those are
  dev-only worlds carrying today's schema). Whole-world or structural migrations
  (splitting an entity, not just renaming a tag) may need more than the per-blob
  transform; that is deferred until a concrete case asks for it.

## Backends

`SqliteStore` exists now (dev and embedded). Postgres will follow with the same
schema and JSONB, selected by configuration, behind the `Persistence` trait. A
remote Postgres only adds latency to the async save path, never to the tick,
which is the intended growth lever: move the DB off-box before sharding the sim.
