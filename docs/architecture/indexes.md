# Secondary indexes

> Status: **built.** The generic index crate (`musce_index`), the engine resource
> store it lives in, and the reference game's spatial consumer (`Xyz` on rooms with
> `@setpos`/`@pos`/`@nearby`) all exist and are exercised by unit tests and a
> crossover benchmark. This records why a maintained index is shaped the way it is.

A secondary index answers "which entities key to X" without scanning the world.
The world is authoritative and stored as an entity table; a query like "rooms near
here" or "everything on this z-level" would otherwise walk every entity every time.
`musce_index` maintains a `key -> entities` lookup that stays current as the world
changes, so the query reads the bucket instead of the table.

## What lives where

- **`musce_index`** (game-side crate) is generic and type-agnostic. It indexes an
  arbitrary component `C` under an arbitrary key `K` produced by a game-supplied key
  function. The default is a plain value hash; a custom key (a spatial cell hash) is
  just a different function, so the crate never learns what a coordinate means.
- **`musce_ref`** supplies the concrete consumer: an integer `Xyz` on rooms and the
  spatial queries over it. This is game vocabulary; the engine reads no coordinate.
- **The engine** contributes exactly two things the index rides on: the
  `Fact::ComponentChanged` trigger (see [facts.md](facts.md)) and a `World` resource
  store to home the index in. Nothing index-specific lives in the engine.

## The index is derived state, never persisted

An index is a pure function of the world; persisting it would store a second copy
of truth that can drift. So it is rebuilt on boot and never written to the database.

It cannot be homed on a marker *entity*: `snapshot` enumerates every live entity by
`Id`, so even an entity carrying only an unregistered component persists as a bare
`{id}` shell, and a fresh one spawned each boot leaks. Instead the index lives in a
**`World` resource**: type-keyed transient state (`insert_resource`/`resource`/
`take_resource`) that sits beside the entity table and that `snapshot` never sees,
the same category as the per-tick facts buffer. It costs nothing at save time and
starts empty every boot. This is the honest home for any derived, rebuilt-on-boot
state; the index is its first user.

## Maintenance: baseline once, then triggers

A read-model rebuilt by full scan on every query is not maintained: it just moves
the scan. So the index is built once and kept current by reacting to change:

- **Baseline** on the maintainer system's first run: a full scan seeds every index,
  and the registry is stored in the resource. This runs post-load, inside the tick
  loop, not from a host boot hook. No client is connected at tick 0 (sessions are
  not persisted) and commands drain before systems within a tick, so there is no
  observable window where a query sees an empty index. A host hook stays a cheap
  later addition if that ever changes; it was not worth adding for a non-problem.
- **Incremental** thereafter: each tick the maintainer applies the fact batch.
  `Fact::ComponentChanged { entity, tag }` is a payload-free trigger; the index
  *rereads* the entity's current key and moves it between buckets (a missing
  component drops it). `Fact::Destroyed` evicts by the reverse map. Reread makes the
  order within a batch irrelevant: a change and a destroy for one entity converge
  either way, and a duplicate trigger is idempotent.

The maintainer is registered **first** in `Game.systems`, so a later system in the
same tick reads the updated index. A command-phase reader (`@nearby`) runs before
the system loop, so it sees last-boundary (one-tick-lagged) values, which is
consistent with the rest of the reaction channel.

Only components a consumer opts into via `World::track_component` emit the trigger,
so the fact stream stays bounded to what is actually indexed.

## Many indexes over one component

An index is identified by its own **name**, not the component it reads, so several
indexes may read one component with different keys. Over `Xyz`, `xyz_cell` (a
spatial hash for range queries) and `xyz_level` (a bucket per z-level) are two
indexes both keyed off `"xyz"`. Because the trigger names the *component*, one
`Xyz` write emits one `ComponentChanged` that the maintainer fans out to every
index over that component. The engine never learns the index registry exists.

## Uniqueness is detected, not enforced

A per-index `Policy` records whether a key is expected to identify one entity
(`Unique`) or many (`Multi`, the default). A rebuilt read-model cannot intercept
writes, so it cannot *enforce* uniqueness; `Unique` only enables `conflicts()`, an
on-request scan reporting entities that share a key. Enforcement, if ever wanted, is
a game rule at the write site, not the index's job.

## The reference consumer

`musce_ref::spatial` puts an integer `Xyz` on rooms (only rooms; ordinary
containment stays room-based) and answers range queries over it:

- `@setpos #<room> <x> <y> <z>` writes the tracked `Xyz`, so the index updates.
- `@pos [#<thing>]` reports coordinates.
- `@nearby [<radius>]` lists rooms within a radius of the room you are in, using
  `near`, which expands the query sphere into the spatial-hash cells it touches,
  unions their buckets, and exact-distance-filters. The sphere-to-cells expansion is
  the only spatial-specific code; the index itself only does exact-key `get`.

## When an index is worth it

The `index_query` benchmark ([benchmarks.md](benchmarks.md)) compares the indexed
`near` against a naive full scan across world sizes. The indexed query is roughly
flat once its neighborhood is populated (O(cells the sphere touches)); the naive
scan is linear (O(rooms)). They cross around **~70k rooms**: below that the scan is
faster, because the index's bounded neighborhood walk has a larger constant than a
linear pass over a small world. So this index is a large-world tool. A typical MUD
with hundreds to low thousands of rooms is better served by the scan, and the value
of the crate at that scale is the *machinery* (a maintained, reusable read-model),
not a speedup. The cell size and query radius set the constant, and are the lever if
the crossover ever needs to move.
