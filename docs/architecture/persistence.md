# Persistence

The database is the **saved** form of the world; the in-memory world is the live
truth. The DB is never queried on the runtime tick path. It is written and read for
save/load, and, because entities are stored shredded into per-component rows (see
Storage shape), it also serves **out-of-band** analytics run by tooling off the hot
path (a component census, a cross-entity aggregation). That single decision (the tick
never waits on the DB) shapes everything here. See
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

The tick never queries the DB, so components need not be individually SQL-queryable
for the *engine*. They are still stored **one row per component**, because that shape
costs nothing on the save/load path and makes the data legible to out-of-band tools
(a `GROUP BY tag` census, a `json_extract` filter across entities). An entity is a
roster row plus its component rows:

```sql
entities (
  entity_id INTEGER PRIMARY KEY,   -- the global EntityId
  zone      INTEGER                -- extracted for shard-scoped load (unused yet)
)
components (
  entity_id INTEGER NOT NULL REFERENCES entities(entity_id),
  tag       TEXT    NOT NULL,      -- the component's stable string tag
  data      TEXT    NOT NULL,      -- that one component's value as JSON (Postgres: JSONB)
  PRIMARY KEY (entity_id, tag)
)
meta (key TEXT PRIMARY KEY, value TEXT)  -- next_id high-water mark, schema version
```

`data` is **JSON text**: human-readable while the schema churns, JSONB on Postgres
later. A component's value is stored as its JSON string, so a marker (which
serializes to `null`) is the text `"null"`, never a SQL `NULL`. The `entities` roster
carries entity-level columns (only `zone` today) and anchors existence for load; the
component rows hang off it.

The split costs more write volume than one blob per entity: a full save rewrites
`~N x (1 + components)` rows instead of `~N`, all of them every save (snapshots are
whole-world). Dirty-tracked partial saves (deferred; see the save contract) reclaim
it, and become *more* precise here, rewriting only the changed component rows.

## Hot and cold data

> Status: the cold content store (`KvStore`) is built as a primitive
> (`kv_init`/`kv_get`/`kv_put`); the async cold-read path that wires it to a verb is
> deferred (see below), and cold *entities* (paging) remain unbuilt.

Everything registered is **hot**: resident in memory and written into the component
rows every save. That is the only tier that reaches the world, and is right for the
data an entity reasons about each tick. Some data is different: large, rarely read,
and wasteful to keep resident (a book's full text, a mail archive, a long audit log).
The home for it is **cold storage**, and the model already leaves room for it without
a migration.

- **Cold content, `KvStore`.** The entity stays resident (its name, location, and a
  small hot component holding a **key**), but the heavy payload lives in a separate
  content store, `kv (key TEXT PRIMARY KEY, value BLOB)`, fetched on demand (a `read`
  verb pulls a book's text by key). The store is a flat, game-owned keyspace, exactly
  like an object store (S3): the game namespaces by key prefix (`book:<hash>`,
  `notes:<id>`), and a **shared key is many-to-one dedup** (every copy of one book
  points at a single row). Values are engine-opaque bytes the game encodes and
  decodes; the engine never interprets cold data, which is why it can stay off-heap.
  `KvStore` is a **separate trait** from `Persistence` (the whole-world save/load
  contract) precisely so cold storage can later be backed differently (object
  storage) without disturbing world save. Core needs no change: a cold payload is
  simply *not a registered hot component*, so it never enters the component rows. The
  invariant to preserve: the `ComponentRegistry` stays the single authority for what
  the component rows contain, so "cold" means exactly "not registered hot," with no
  competing notion of what is persisted.
- **No cascade, no cross-store transaction (yet).** A deleted entity does **not**
  delete `kv` rows: a row may be shared, so its lifetime is not entity-scoped.
  Orphans (a `kv` row no entity references) are an accepted storage cost; a GC pass
  (mark-sweep over referenced keys, or a refcount) is a future addition. The cold
  read/write path is also not yet wired; when it is, the ordering contract is **cold
  data first, then the referencing component**, so a failed `kv_put` leaves a harmless
  orphan rather than a dangling reference to a missing key.
- **Transparency is deferred.** We may later want cold access to feel like an
  ordinary component (auto-materialized on read, written back on change), or a way
  to pre-flag a component type as cold. That is a larger abstraction touching the
  registry and the access path; we are **not** building it yet, and specifically
  not engine-side. The book case is served by an explicit fetch by key.
- **Cold entities (paging) is a different feature.** A vast library where most
  entities are not resident until browsed is not field-laziness; it is paging, and
  it lands on the `EntityIndex`, which today is binary (an id resolves to a live
  handle or is absent) and would need a third "exists but not loaded" state. It
  overlaps sharding's zone-scoped load (the same `zone` column selects a subset),
  so whenever one is built the other should be looked at with it. Deferred until a
  concrete need appears.

## The save / confirm contract

Deletes are the fragile part of save. A despawned entity is already gone from the
live world, so the pending-delete set is the *only* record of it. Therefore:

- `snapshot()` **copies** the pending deletes; it does not clear them.
- The persistence layer's `save()` runs in one transaction. Each upserted entity has
  its component set **replaced** (delete its rows, insert the current set), so a
  component dropped since the last save cannot leak and resurrect on reload. Each
  despawned id deletes its component rows then its roster row (the FK is RESTRICT, so
  correctness never rides on the `foreign_keys` pragma being on).
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

On load, `next_id` is clamped to `max(stored marker, max live id + 1)`, so it can
never fall to or below a live id and let the next save reissue an id over a stored
entity. A missing or stale-low marker (a restored dump with no `meta`) is corrected
and warned, not silently trusted.

## Integrity and evolution

- On load, the DB primary key and the entity's own `Id` component are checked to
  agree (a `debug_assert`), catching corrupt or wrongly-keyed data instead of
  letting index and component silently diverge.
- Per-component storage can represent states a single blob could not, so load rejects
  them rather than dropping data: an entity whose rows reassemble with no `Id`
  component is a hard error (an Id-less entity would otherwise only detonate at the
  next snapshot, since the id-agreement check above is a release-disabled
  `debug_assert`), and component rows referencing an entity absent from the roster
  (orphans a roster-driven load would silently skip) are a hard error too.
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
  dev-only worlds carrying today's schema). The seam takes the full
  `&mut Vec<EntityBlob>` (not a fixed slice), so a transform can add or drop whole
  entities, not just mutate them in place; whole-world or structural migrations
  (splitting an entity, not just renaming a tag) are still deferred until a concrete
  case asks for it.
- **`schema_version` versions the vocabulary, not the layout.** It marks the
  component tag/value shape, and the seam runs on already-reassembled blobs, so it
  cannot express or migrate a change to the *physical* table layout. Pointing new
  code at a database written in an older physical layout fails to load and refuses to
  boot (it never corrupts): the stored data is left untouched. With only dev worlds
  today that means recreating the DB; a real layout migration would be a one-time
  operational step outside the seam.

## Backends

`SqliteStore` exists now (dev and embedded). Postgres will follow with the same
schema and JSONB, selected by configuration, behind the `Persistence` trait. A
remote Postgres only adds latency to the async save path, never to the tick,
which is the intended growth lever: move the DB off-box before sharding the sim.
