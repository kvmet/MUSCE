# Cold storage

> Status: the cold content store (`KvStore`) is built (`kv_init`/`kv_get`/`kv_put`)
> and wired to verbs: the reference game's `read`/`inscribe` fetch and overwrite a
> book's cold text through the async cold-op path (below). Game-chosen shared keys
> (deliberate dedup) work today; automatic content-addressed dedup is deliberately a
> game/plugin concern, not an engine feature (see "Dedup is a game decision"). Cold
> *entities* (paging) remain unbuilt.

MUSCE splits durable data into two tiers. Everything registered is **hot**: resident
in memory and written into the world's per-component rows when it changes (the world
save/load model is [persistence.md](persistence.md)). That is the only tier that
reaches the world, and is right for the data an entity reasons about each tick. Some
data is different: large, rarely read, and wasteful to keep resident (a book's full
text, a mail archive, a long audit log). The home for it is **cold storage**, and the
model already leaves room for it without a migration.

- **Cold content, `KvStore`.** The entity stays resident (its name, location, and a
  small hot component holding a **key**), but the heavy payload lives in a separate
  content store, `kv (key TEXT PRIMARY KEY, value BLOB)`, fetched on demand (the
  reference game's `read` verb pulls a book's text by key; `inscribe` overwrites it).
  The store is a flat, game-owned keyspace, exactly like an object store (S3): the game
  namespaces by key prefix (`book:<id>`, `notes:<id>`), and a **game-chosen shared key
  is many-to-one dedup** (every copy of one book points at a single row; see "Dedup is
  a game decision"). Values are
  engine-opaque bytes the game encodes and decodes; the engine never interprets cold
  data, which is why it can stay off-heap. `KvStore` is a **separate trait** from
  `Persistence` (the whole-world save/load contract) precisely so cold storage can
  later be backed differently (object storage) without disturbing world save. Core
  needs no change: a cold payload is simply *not a registered hot component*, so it
  never enters the component rows. The invariant to preserve: the `ComponentRegistry`
  stays the single authority for what the component rows contain, so "cold" means
  exactly "not registered hot," with no competing notion of what is persisted.
- **The wired path: async, off the sim thread, decode injected by the game.** A verb
  cannot touch the store (the sim holds none, and `kv_get`/`kv_put` are async), so it
  records a cold request (`ColdOp`) exactly as it records perception output; the
  runtime routes these to a cold task that owns the store, and the task's result rides
  the normal event outbox **straight to the reader's connection** (no round-trip back
  through the sim, since a read mutates nothing). Decoding a fetched value into
  deliverable text is **game knowledge** (the game encoded it), so it is an injected
  `Game.decode_cold` the task calls; the engine still interprets nothing. A read of an
  absent key delivers a blank-page line, not an error. The task applies same-key ops in
  issue order (read-your-writes, no lost writes), free today from the single consumer;
  a future parallel cold path must preserve that *per-key* order (route by `hash(key)`).
  This is the task's processing order, distinct from the cold-data-first write
  sequencing below (which concerns only content-derived keys).
- **No cascade, no cross-store transaction.** A deleted entity does **not** delete
  `kv` rows: a row may be shared, so its lifetime is not entity-scoped. Orphans (a `kv`
  row no entity references) are an accepted storage cost; a GC pass (mark-sweep over
  referenced keys, or a refcount) is a future addition. The reference game's book path
  keys cold rows to the entity id (`book:<id>`), so the key is fixed at spawn and the
  referencing component is written *before* any bytes exist: reading an unwritten key
  reads as blank, not as a dangling reference, so write ordering is moot for it (a
  failed `kv_put` just leaves the page blank until the next `inscribe`). The stricter
  **cold-data-first** ordering matters only when a key is *derived from its bytes* (a
  content hash), where the key changes on every edit. That is deliberately not an
  engine feature (see the next bullet).
- **Dedup is a game decision, and that is where the seam ends.** There are two ways to
  make copies share a `kv` row, and the engine owns neither, on purpose:
  - **A game-chosen shared key**, used *deliberately*: a game points many entities at
    one key (`book:<title-version>`) because it means them to be the same, and treats
    that row as **immutable by convention** (you author a canonical book, you do not
    `inscribe` it). This needs nothing beyond the primitive: arbitrary string keys
    already allow it. It is the reference path with a hand-picked key in place of the
    entity id, and it works today.
  - **A content-addressed key** (`book:<hash(bytes)>`) makes dedup *automatic* but only
    stays coherent while the content is immutable: editing one copy changes its hash,
    so the key must be reallocated and the referencing hot component rewritten
    (copy-on-write), and orphaned hashes then need GC. That is the mutable-and-shared
    corner, the most complex place to add automatic collapse and the least often
    wanted (immutable shared content is already served by the shared-key case above,
    with no hashing). So the engine stays out of it: a consumer that wants content
    addressing builds a hash-id scheme *on top of* `KvStore` (a game module today, a
    plugin crate later); the engine provides the opaque byte store and the async
    read/write path, and nothing about identity or dedup.

  Not an offline cleanup job, either: the key lives in a **hot** component, so
  collapsing rows after the fact means rewriting live entities' keys, i.e. mutating
  world-truth out of band, which fights "the world is authoritative, the DB is
  derived." Sharing is decided at authoring time or not at all.
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
