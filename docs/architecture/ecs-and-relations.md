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
[authorization.md](authorization.md)). App kinds like
`Item`/`Creature`/`Container`/an exit/a player avatar are app
vocabulary and live in the app, registered through `App.register` (see
[engine-and-app.md](engine-and-app.md)); the engine stores them but never
interprets them. Exit connectivity is app-side in full: an exit entity is an
app-owned kind tag plus app-owned `LeadsFrom`/`LeadsTo` relations, defined in
`musce_ref` over the engine's public relation layer (see Exits below).

## The relation layer

hecs has no relationships, so we provide one generic, write-once layer rather
than hand-rolling each relationship type. See
[relation.rs](../../musce_core/src/relation.rs).

Relations are **one-to-many**: a source has at most one target; a target has
many sources. (One-to-one is the degenerate case; many-to-many would be a
different primitive, not yet needed.)

- `RelTarget<R>` on the source is the **forward link and the source of truth**.
  It is a persisted component.
- The **reverse list** (a target's sources) is a **derived index**, rebuilt from
  the forward links on load and never persisted. It lives in a side map on `World`
  (`reverse: R -> (target -> sources)`), maintained inline by the same mutators
  that write the forward link, *not* as a component: it is only ever
  point-looked-up by target (via `sources_of`), never iterated archetypally, so a
  component would fragment archetypes and force a raw `&mut` to maintain for no
  columnar benefit. It is homed beside the other derived indexes (`resources`,
  [`musce_index`](indexes.md)), the one place all rebuilt-on-load state lives. It
  cannot instead be a fact-reactive `musce_index`, because the despawn cascade
  reads `sources_of` synchronously mid-tick and a deferred index would be a tick
  stale.

The reverse list is **unordered**: because it is rebuilt from the forward links on
load rather than preserving live insertion order, the order of `sources_of` (and
its wrappers `contents`, exit lists) is unspecified and not stable across a
save/load. The engine promises membership, not order. A caller that wants a stable
display order sorts at the display site by something meaningful to it (a name, a
recency), which is presentation and so app-side anyway. Preserving true insertion
order would mean persisting a per-source sequence and giving up the "reverse lists
are derived" property; that is a deliberate future feature to build only if a
concrete need for it appears, not a default we pay for.

Each relation kind is a marker type implementing the `Relation` trait, whose
`const` policies are `ACYCLIC` (whether `relate` rejects cycles),
`ON_TARGET_DESPAWN` (the cascade: `DespawnSources`, `Reparent`, or `Detach`), and
`EMITS_MOVEMENT` (whether a change to this relation emits the `Moved`/`LocusChanged`
facts; default `false`, true only for `Containment`, the one spatial relation, see
[facts.md](facts.md)).

Two small registries are populated at world construction: a component registry
(drives JSON serialization) and a relation registry (type-erased despawn, rebuild,
and tag-driven relate/unrelate hooks per relation, the last backing the `Relate`
action).

### Important: relations are ergonomics, not speed

The forward link compiles down to the same component you would hand-roll, and the
reverse list to the same side map, so the layer does **not** make traversal
faster. Its value is writing the bidirectional bookkeeping, cascade, and
acyclicity once and reusing it across every relation type. If traversal ever profiles hot, the fix is a separate derived index (a
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
- As the one spatial relation (`EMITS_MOVEMENT`), a containment change emits the
  `Moved`/`LocusChanged` facts, for the moved entity alone, not its carried
  subtree (see [facts.md](facts.md)).
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
  app policy; where an existing cursor may land is structure.
- Helpers: `focus_of`, `set_focus`, `clear_focus`, and `control_root` (the topmost
  controller of an entity, walking `Controls` up; the inverse of resolving a
  driven actor down through `Focus`).

## Exits

> Status: **built.** Exits are relation-backed entities (an `Exit` marker plus a
> general `Name` component, wired by `LeadsFrom` and `LeadsTo` with the
> `DespawnSources` cascade) and are wired through the `Relate` action. The
> Portal/Through door layer remains deferred.

The room graph is **app vocabulary**, not engine machinery: the connectivity
relations (`LeadsFrom`/`LeadsTo`) and the exit queries live in `musce_ref`
(`exits.rs`), defined over the engine's public relation layer and registered
through `App.register`, exactly like the kind markers. The engine never reads exit
connectivity; it owns only the generic relation + cascade mechanism that
connectivity is built on. What follows is the reference app's model.

A locus connects to many loci and is reachable from many, so connectivity is
**many-to-many**, while the relation layer is one-to-many. The app does not
generalize the primitive for it. Connectivity is carried by an intermediate **exit
entity** whose two endpoints are each one-to-many and so fit the existing layer
exactly: an exit has one origin and one destination. (This is the general move for
many-to-many in this engine: an intermediate entity, not a new relation kind.)

It also keeps every cross-reference *inside* the relation layer, so there is no raw
`EntityId` in a JSON blob invisible to the despawn cascade. As relation-wired
entities, exits join the cascade like everything else.

### The model

An exit is an entity carrying:

- an **`Exit`** zero-sized kind marker (app-defined vocabulary the app filters
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
[actions.md](actions.md)), and the veto is a **app rule, not an engine concept**.
The app defines the exit entity and a home for door/lock state; the engine bakes
in no lock semantics. The app's `go` handler: (1) finds the exit out of the mover's room
whose `Name` matches (reverse index of `LeadsFrom`, resolved through the unified
name resolver: exact then whole-or-word prefix on the `Name`, then aliases, then a
description substring), (2) runs a shared `can_traverse(world, mover, exit) -> Result<(),
Reason>` app rule (a locked portal, a guard, a size limit) *before* committing,
and (3) on pass `Move`s the mover into the exit's `LeadsTo` destination.
`can_traverse` is a shared helper (like `is_takeable`), so a scripted NPC walking
into a locked door fails exactly as a player does; "you cannot enter" is always a
pre-commit rule, never a reaction. With no doors yet, `can_traverse` is an app-side
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
longer prose an `examine` reveals. Extra match keywords live in an app-side
`Aliases` component the resolver also reads.

## Queries

Two kinds, and the split drives what machinery exists:

- **Archetypal** ("which entities have components X?") is what hecs does
  natively and fast. Needs only marker components to filter by kind.
- **Relational** ("which entity is related to this one?") hecs does not do. We
  answer it with the relation components as indexes plus the `EntityId` index.

Reads go through `World`'s own addressed-by-id surface, never a raw hecs handle:
`world.query::<Q>()` for archetypal iteration, `world.get::<C>(id)` for one
component, `world.has::<C>(id)`, `world.contains(id)`. The raw `hecs::World` and
`EntityRef` are **not reachable outside the crate**: there is no `ecs()` accessor,
and `entity_ref` (the raw `EntityRef`) is `pub(crate)` for trusted internal use only
(snapshot serialization). This is deliberate and load-bearing for correctness, not
just tidiness. hecs does *runtime* borrow checking, so a shared `&hecs::World` can
still hand out `&mut C` (interior borrow); exposing it would let any reader mutate a
component below the mutator layer, silently skipping the `EntityId` index, the
despawn cascade, the reverse lists, and the persistence dirty set. So `query` is
bounded by [`ReadQuery`](../../musce_core/src/world.rs) (shared borrows only, never
`&mut`), and `get` returns a shared `Ref`. The **only** way to change a persisted
component is through `World`'s mutators (`set_component`/`insert`/`remove`/`modify`,
`move_entity`/`relate`), which keep all of that consistent; a raw `ecs.despawn`
would skip the cascade, a raw `ecs.spawn` would make an `Id`-less entity, and a raw
`get::<&mut C>` would drop a change from the next delta snapshot. Making the raw
handle unreachable turns that boundary from a convention into a compile error.

Those write mutators are all **per-entity**: `modify(id, f)` is a point lookup
(`EntityId` -> `hecs::Entity` -> archetype fetch) each call. A system that writes one
component across many entities every tick therefore pays that lookup per entity where
a raw `query::<(&mut C,)>()` would do one columnar pass. A sanctioned bulk mutator
(`modify_each`) closes that gap without reopening the hole: iterate the mutable query
under the hood, collect the touched ids, then mark them dirty and emit
`ComponentChanged` in a second pass (the fact push cannot borrow `self` while the
query holds `ecs`). It is **deferred** on purpose, not forgotten: keeping hecs's
archetypal storage is precisely what leaves this escape valve open, but the right
signature is consumer-shaped (a single-`C` form vs a filtered multi-component query,
and whether the closure returns "did I change it" to keep fact emission precise), so
it is built with the first hot bulk-write system that defines those, as a pure
addition.

The recursive contents walk (`descendants`) is a predicate-driven, visitor-based
tree walk: the engine is the mechanism, the caller supplies the descent policy
(e.g. stop at creatures or closed containers for looting; descend everywhere for
persistence). Visitor-based so callers can early-exit without allocating.

Proximity queries ("things near `[x,y]`") are a different beast needing a spatial
index, and belong to app logic once coordinates exist. Deferred.
