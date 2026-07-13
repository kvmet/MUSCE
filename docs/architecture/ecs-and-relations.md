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

An entity's kind is a zero-sized marker component. This lets archetypal queries
filter by kind, e.g. "all loci with coordinates". The engine defines only the one
kind it reads: `Locus` (the perception boundary; a scope in the containment tree
found by `enclosing_locus`, neutral of any "room" meaning). Permissions are not a
kind: authorization is account-scoped, not a marker on the actor (see
[authorization.md](authorization.md)). Game kinds like
`Item`/`Creature`/`Container`/an exit/a player avatar are game
vocabulary and live in the game, registered through `Game.register` (see
[engine-and-game.md](engine-and-game.md)); the engine stores them but never
interprets them. Exit connectivity is game-side in full: an exit entity is a
game-owned kind tag plus game-owned `LeadsFrom`/`LeadsTo` relations, defined in
`musce_ref` over the engine's public relation layer (see Exits below).

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

The reverse list is **unordered**: because it is rebuilt from the forward links on
load rather than preserving live insertion order, the order of `sources_of` (and
its wrappers `contents`, exit lists) is unspecified and not stable across a
save/load. The engine promises membership, not order. A caller that wants a stable
display order sorts at the display site by something meaningful to it (a name, a
recency), which is presentation and so game-side anyway. Preserving true insertion
order would mean persisting a per-source sequence and giving up the "reverse lists
are derived" property; that is a deliberate future feature to build only if a
concrete need for it appears, not a default we pay for.

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
  `enclosing_locus` (walk up to the nearest `Locus`, the perception boundary).

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
  a bespoke despawn path that would have to infer the focuser from `Controls`. The
  cursor must stay *within* the control chain: `set_focus` rejects (with
  `FocusError::NotControlled`) a target the controller does not transitively
  control, since a `Focus` outside the `Controls` subtree is a structurally
  invalid state, not rejected play. Establishing control in the first place stays
  game policy; where an existing cursor may land is structure.
- Helpers: `focus_of`, `set_focus`, `clear_focus`, and `control_root` (the topmost
  controller of an entity, walking `Controls` up; the inverse of resolving a
  driven actor down through `Focus`).

## Exits

> Status: **built.** Exits are relation-backed entities (an `Exit` marker plus a
> general `Name` component, wired by `LeadsFrom` and `LeadsTo` with the
> `DespawnSources` cascade) and are wired through the `Relate` action. The
> Portal/Through door layer remains deferred.

The room graph is **game vocabulary**, not engine machinery: the connectivity
relations (`LeadsFrom`/`LeadsTo`) and the exit queries live in `musce_ref`
(`exits.rs`), defined over the engine's public relation layer and registered
through `Game.register`, exactly like the kind markers. The engine never reads exit
connectivity; it owns only the generic relation + cascade mechanism that
connectivity is built on. What follows is the reference game's model.

A locus connects to many loci and is reachable from many, so connectivity is
**many-to-many**, while the relation layer is one-to-many. The game does not
generalize the primitive for it. Connectivity is carried by an intermediate **exit
entity** whose two endpoints are each one-to-many and so fit the existing layer
exactly: an exit has one origin and one destination. (This is the general move for
many-to-many in this engine: an intermediate entity, not a new relation kind.)

It also keeps every cross-reference *inside* the relation layer, so there is no raw
`EntityId` in a JSON blob invisible to the despawn cascade. As relation-wired
entities, exits join the cascade like everything else.

### The model

An exit is an entity carrying:

- an **`Exit`** zero-sized kind marker (game-defined vocabulary the game filters
  on; never takeable; the engine stores but never reads it),
- a general **`Name`** component (`"north"`, the handle a player types and sees;
  defined beside `Description`, and shared by every nameable thing), and
- two relation links:
  - **`LeadsFrom`**: exit → its origin room. A room's exit list is this relation's
    reverse index, so listing a room's exits is an index read, not a scan.
  - **`LeadsTo`**: exit → its destination room.

The match key is the general `Name` component; a direction is just a common
name, not a dedicated field.

Both endpoints are one-to-many (an exit has exactly one origin, one destination)
and **not acyclic**: their sources (exits) and targets (rooms) are disjoint kinds,
so a chain can never close on itself, and the *room* graph is free to contain
cycles (mazes, loops) precisely because that graph is no single relation's chain.

Asymmetry is the default and costs nothing: a `north` exit from `hall` to `garden`
is one exit; the return `south` exit from `garden` to `hall` is a second,
independent one, and a one-way
drop is simply an exit with no reciprocal. The link is cascade-visible and
reverse-indexed.

### Cascade: no dangling exits

Both endpoint relations use the **`DespawnSources`** cascade. Destroying a room
despawns every exit that is a source of `LeadsFrom` *or* `LeadsTo` against it, so it
takes its outgoing **and** incoming exits with it. There is never an exit to or
from a room that no longer exists, which closes the dangling-pointer hole that
blocked `@destroy` (see [admin-verbs.md](admin-verbs.md)).

### Doors and thresholds (deferred)

A plain opening is just the exit. A richer doorway (examinable, lockable,
breakable) is door state living *on* the exit as components for a one-sided thing
(a ladder, a hatch), or on a shared **`Portal`** entity for a two-sided door that
must read and lock identically from both rooms: two opposing exits reference one
portal via a **`Through`** relation (exit → portal), so locking the portal once
locks both directions. The portal layer is **additive and deferred** (build it when
doors exist); exits work without it.

### Traversal and veto

Movement through an exit is the usual validate -> mutate -> emit (see
[actions.md](actions.md)), and the veto is a **game rule, not an engine concept**.
The game defines the exit entity and a home for door/lock state; the engine bakes
in no lock semantics. The game's `go` handler: (1) finds the exit out of the mover's room
whose `Name` matches (reverse index of `LeadsFrom`, resolved through the unified
name resolver: exact then whole-or-word prefix on the `Name`, then aliases, then a
description substring), (2) runs a shared `can_traverse(world, mover, exit) -> Result<(),
Reason>` game rule (a locked portal, a guard, a size limit) *before* committing,
and (3) on pass `Move`s the mover into the exit's `LeadsTo` destination.
`can_traverse` is a shared helper (like `is_takeable`), so a scripted NPC walking
into a locked door fails exactly as a player does; "you cannot enter" is always a
pre-commit rule, never a reaction. With no doors yet, `can_traverse` is a game-side
stub returning `Ok`.

### Wiring exits: the `Relate` action

Exits are wired through the executor, not by hand. The `Relate` / `Unrelate`
actions (in the [actions.md](actions.md) vocabulary) are the typed face of
`World::relate_tag`/`unrelate_tag`, so wiring an exit goes through `execute` and the
future action journal like every other mutation. `@dig` `Create`s the exit entity
(marker + `Name`), then `Relate`s it `LeadsFrom` its room and `LeadsTo` the new
room, with the reciprocal a second exit the other way.

The `Name` is general, not exit-specific: every nameable thing (items, creatures,
the player) carries one as its primary in-character handle, with `Description` the
longer prose an `examine` reveals. Extra match keywords live in a game-side
`Aliases` component the resolver also reads.

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
