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
the database for the write. It is a **delta**: the `World` marks an entity dirty at
every mutator chokepoint that writes a persisted component or forward relation link,
and `snapshot()` serializes only that dirty set, so a save costs O(changed), not
O(world), and carries no periodic tick-time spike. That is what makes a high save
cadence affordable: the loss window shrinks toward the save interval without a
whole-world write behind it. A freshly seeded world's first snapshot is still full
(every spawn dirtied it); a loaded world starts clean (the store already matches),
so nothing is rewritten until it mutates. The one exception is a schema migration:
a differing stored version marks every loaded entity dirty (`mark_all_dirty`) so the
migrated form is re-persisted.

The dirty set rides the same coverage boundary as `ComponentChanged` and the index
layer: a persisted component mutated below the mutator layer via `ecs().get::<&mut
_>()` is *not* seen, and is the caller's to route through `World::modify` (the same
discipline `forbid_tracking` enforces). Raw `&mut` that skips this desyncs a tracked
index already, so the delta introduces no new hole.

What is and isn't serialized:

- **Persisted:** real component data and forward relation links (`RelTarget`).
- **Not persisted:** reverse relation lists (`RelSources`) and the `EntityId`
  index. Both are derived and rebuilt on load. This shrinks the save and removes
  a class of "the two sides disagree on disk" bugs.
- **Not persisted:** `World` resources (`insert_resource`/`resource`/
  `take_resource`), type-keyed transient singletons for derived state such as a
  secondary index (see indexes.md). `snapshot` serializes entity rows only, so a
  resource is never written; it is rebuilt on boot, the same as the reverse lists.

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
  data      TEXT    NOT NULL,      -- that one component's value as JSON text (both backends)
  PRIMARY KEY (entity_id, tag)
)
meta (key TEXT PRIMARY KEY, value TEXT)  -- next_id high-water mark, schema version
```

`data` is **JSON text** on both backends: human-readable while the schema churns
(JSONB on Postgres is a deferred optimization, see Backends). A component's value is
stored as its JSON string, so a marker (which
serializes to `null`) is the text `"null"`, never a SQL `NULL`. The `entities` roster
carries entity-level columns (only `zone` today) and anchors existence for load; the
component rows hang off it.

The split costs more write volume than one blob per entity: a saved entity rewrites
`1 + components` rows instead of `1`. A delta save bounds that to the dirty set (see
the save contract), so only changed entities pay it, each with its whole component
set replaced. Per-component-row precision (rewriting only the changed rows *within* a
dirty entity, not its whole set) is a further optimization, deferred until it hurts.

## Hot and cold data

Registered components are the **hot** tier: resident in memory and written into the
per-component rows above when they change. Large, rarely-read payloads (a book's full
text, a mail archive, a long audit log) are wasteful to keep resident, so they belong
in **cold storage**: a separate `KvStore` keyspace of engine-opaque bytes, fetched on
demand off the sim thread, that never enters the component rows. That store, its
async cold-op path, and the dedup/ordering rules it deliberately leaves to the game
are their own concern: see [cold-storage.md](cold-storage.md). "Cold" means exactly
"not a registered hot component," so the `ComponentRegistry` stays the single
authority for what the world's rows contain.

## The save / confirm contract

A delta save carries two pending sets with **opposite** confirm rules, because a
delete and a live change fail differently. A despawned entity is gone from the live
world, so the pending-delete set is its only record; a dirtied live entity is still
in the world, but the dirty set is the only record that it is *unsaved*. The
asymmetry that follows is the crux of the contract:

**Deletes are copied.** A despawned entity is already gone from the live world, so
the pending-delete set is the *only* record of it. Therefore:

- `snapshot()` **copies** the pending deletes; it does not clear them.
- The persistence layer's `save()` runs in one transaction. Each upserted entity has
  its component set **replaced** (delete its rows, insert the current set), so a
  component dropped since the last save cannot leak and resurrect on reload. Each
  despawned id deletes its component rows then its roster row (the FK is RESTRICT, so
  correctness never rides on the `foreign_keys` pragma being on). The roster upserts,
  the per-entity component clears, and the component inserts are each issued as
  **batched multi-row** statements (chunked to stay under the backend's bind-variable
  limit: 999 on SQLite, 65535 on Postgres), not row-at-a-time, so save cost is bound by
  the number of batches, not the number of rows. The delete-then-insert semantics are
  unchanged: the batched form clears every live entity's old rows (`DELETE ... WHERE
  entity_id IN (…)`) before inserting the current set.
- Only after a successful save does the caller invoke
  `World::confirm_saved(&snapshot.deletes)`, which drops exactly those ids from
  the pending set. Deletes that accumulated since the snapshot are preserved.

**Dirty ids are drained, and restored on failure.** A full snapshot could copy the
live set too and rely on idempotent re-writes, but a delta cannot: a live entity
re-mutated *after* the snapshot must re-enter the set for the next one, so the
snapshot **drains** the dirty set rather than copying it. That makes a save failure
the fragile case, and it is handled explicitly:

- `snapshot()` **drains** the dirty set into the delta.
- On a **successful** save, nothing more is needed: the drained entities are durable
  at their snapshot state, and anything mutated since has already re-dirtied for the
  next delta.
- On a **failed** save, the persistence layer hands the delta's entity ids back
  (`Ack::Failed`), and the sim calls `World::remark_dirty(&ids)` to return them to
  the dirty set, so the next snapshot re-serializes them at their then-current state.
  A since-despawned id is dropped here (it rides `deletes` instead), never resurrected
  into the live set.

So a failed save loses nothing: deletes ride the next snapshot (copied), and the
live delta is put back (re-dirtied). The two rules are mirror images because a
delete is monotonic (an id, once dead, never re-dirties, so clear-on-confirm is
safe) while a live change is not (the same id re-dirties with new state, so
drain-and-restore is required). A command journal for sub-snapshot crash recovery
stays deferred; this contract is the minimum that keeps both durable across a save
failure.

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

Two backends exist behind the `Persistence` + `KvStore` traits: `SqliteStore`
(dev and embedded) and `PostgresStore` (production). The runtime holds a
`WorldStore` enum that forwards to whichever the connection URL's scheme names
(`sqlite://…` / `sqlite::memory:` vs `postgres://…`), so game code, `run`, and the
persistence task never name a backend. The account store has the same shape
(`AccountBackend` over `AccountStore`/`PostgresAccountStore`); world and accounts
select independently by their own URLs.

The schema is **logically identical** across backends and stays that way by
construction: each table is written once as a template whose only variable is the
dialect's type word, so structural drift is impossible. The forced differences are
exactly three, dictated by Postgres's type system and how sqlx maps Rust types:

| Column kind | SQLite | Postgres |
|-------------|--------|----------|
| 64-bit ids  | `INTEGER` | `BIGINT` (SQLite `INTEGER` is already 64-bit; PG's is 32) |
| cold bytes (`kv.value`) | `BLOB` | `BYTEA` (PG has no `BLOB`) |
| `is_su` flag | `INTEGER` | `BOOLEAN` (sqlx binds a Rust `bool` per dialect) |

Everything else, including `data` as **JSON text** on both, is identical. JSONB is a
Postgres-specific optimization left deferred behind the trait, not used yet; the two
backends stay parallel until a concrete need earns it.

The risky load logic (the id-less and orphan checks, the `next_id` floor, the
account schema-version refusal) is a single backend-free function each store hands
its extracted rows, so both inherit the same invariants and it is unit-tested
without a database. Cross-backend parity is verified in CI, which reruns the
black-box store tests against a real Postgres.

**Deployment note:** unlike SQLite (`create_if_missing`), Postgres has no
create-on-connect. The database must already exist; `init` only creates tables
within it. A remote Postgres only adds latency to the async save path, never to the
tick, which is the intended growth lever: move the DB off-box before sharding the
sim.
