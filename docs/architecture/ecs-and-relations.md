# ECS and Relations

## Why hecs

hecs is a minimal archetypal ECS. We chose it over the alternatives for what it
does *not* impose:

- **flecs** has the best relationships available, but it is a C library. The FFI
  boundary fights our custom World-as-truth persistence (we want to drive our
  own serialization into our own schema) and our multi-world sharding (plain
  owned Rust worlds are simpler to shard than C worlds across FFI).
- **bevy_ecs** is pure Rust and now has native relationships and a scheduler,
  but it moves fast and breaks APIs often, which is costly for a project touched
  intermittently, and bending its reflection to a custom DB schema is more
  framework-fighting than rolling our own.

hecs gives us full control over serialization and trivially cheap, ownable
worlds, at the cost of providing relationships ourselves. That trade favors this
project.

## Identity

A `hecs::Entity` is a generational index local to one world; it means nothing
across persistence or shard boundaries. So every entity also carries a global
[`EntityId`](../../musce_core/src/id.rs) (a `u64`), and an `EntityIndex` maps
`EntityId -> hecs::Entity` per world.

- `EntityId` is the currency for anything that crosses an entity boundary or is
  persisted. Local hot paths still use the fast `hecs::Entity` handle.
- The id is stored both as the DB primary key and as an `Id` component, so an
  entity is self-describing and the two are checked to agree on load.
- The index is derived, never persisted: it is rebuilt as entities load.

## Kinds

An entity's kind is a zero-sized marker component (`Room`, `Item`, `Creature`,
`Container`, `Player`). This lets archetypal queries filter by kind, e.g. "all
rooms with coordinates".

## The relation layer

hecs has no relationships, so we provide one generic, write-once layer rather
than hand-rolling each relationship type. See
[relation.rs](../../musce_core/src/relation.rs).

Relations are **one-to-many**: a source has at most one target; a target has
many sources. (One-to-one is the degenerate case; many-to-many would be a
different primitive, not yet needed.)

- `RelTarget<R>` on the source is the **forward link and the source of truth**.
  It is persisted.
- `RelSources<R>` on the target is the **reverse list, a derived index**. It is
  rebuilt from the forward links on load and never persisted.

Each relation kind is a marker type implementing the `Relation` trait, which
carries two `const` policies: `ACYCLIC` (whether `relate` rejects cycles) and
`ON_TARGET_DESPAWN` (the cascade: `DespawnSources`, `Reparent`, or `Detach`).

Two small registries are populated at world construction: a component registry
(drives JSON serialization) and a relation registry (type-erased despawn and
rebuild hooks per relation).

### Important: relations are ergonomics, not speed

The relation layer compiles down to the same components you would hand-roll, so
it does **not** make traversal faster. Its value is writing the bidirectional
bookkeeping, cascade, and acyclicity once and reusing it across every relation
type. If traversal ever profiles hot, the fix is a separate derived index (a
dirty-flagged cache or arena tree) invalidated at the mutator, not moving
relations out of the ECS. That index is deferred.

## Containment

Containment is the first relation instance. The key unification: "what room am I
in", "what's in this chest", and "what's in my pack" are the same relationship.
Rooms, containers, and inventories are all containers. See
[containment.rs](../../musce_core/src/containment.rs).

- It is acyclic with a `Reparent` cascade (a destroyed container spills its
  contents to its own parent).
- `move_entity` is the **single mutator** for containment. It enforces
  acyclicity and keeps both sides consistent. Because that invariant is enforced
  at the one mutation point, every recursive reader is a simple, cycle-free walk.
- Helpers: `contents` (one level), `container_of` (immediate parent),
  `enclosing_room` (walk up to the nearest `Room`).

## Control and focus

The embodiment primitives are the second and third relation instances: how a
session resolves a driven actor (see
[networking-and-sessions.md](networking-and-sessions.md)). See
[control.rs](../../musce_core/src/control.rs).

- **`Controls`** is the capability wiring: source = the controlled entity (one
  controller), target = the controller (many sources). Acyclic chains
  (character -> mech -> drone) with a `Detach` cascade, so a controller's death
  reverts each controlled entity to its own AI rather than destroying it.
- **`Focus`** is the cursor: source = the controller, target = the single entity
  its input is live on. One per controller, persisted; absence means "drive
  yourself". It is a relation rather than a lone component precisely so a focused
  entity's despawn clears the cursor through the same `Detach` cascade, instead of
  a bespoke despawn path that would have to infer the focuser from `Controls`.
- Helpers: `focus_of`, `set_focus`, `clear_focus`.

## Queries

Two kinds, and the split drives what machinery exists:

- **Archetypal** ("which entities have components X?") is what hecs does
  natively and fast. Needs only marker components to filter by kind.
- **Relational** ("which entity is related to this one?") hecs does not do. We
  answer it with the relation components as indexes plus the `EntityId` index.

The recursive contents walk (`descendants`) is a predicate-driven, visitor-based
tree walk: the engine is the mechanism, the caller supplies the descent policy
(e.g. stop at creatures or closed containers for looting; descend everywhere for
persistence). Visitor-based so callers can early-exit without allocating.

Proximity queries ("things near `[x,y]`") are a different beast needing a spatial
index, and belong to game logic once coordinates exist. Deferred.
