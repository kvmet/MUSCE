# World Queries and Mutation

> Status: **built; sanctioned bulk mutator deferred until a consumer fixes its shape.**

This document defines the public `World` read/write boundary. The identity and
relation invariants it preserves are in
[ecs-and-relations.md](ecs-and-relations.md); dirty/fact bookkeeping is detailed in
[persistence.md](persistence.md) and [facts.md](facts.md).

## Query kinds

Two kinds, and the split drives what machinery exists:

- **Archetypal** ("which entities have components X?") is what hecs does natively
  and fast. It needs only marker components to filter by kind.
- **Relational** ("which entity is related to this one?") hecs does not do. MUSCE
  answers it with relation components as indexes plus the `EntityId` index.

Reads go through `World`'s addressed-by-id surface, never a raw hecs handle:
`world.query::<Q>()` for archetypal iteration, `world.get::<C>(id)` for one
component, `world.has::<C>(id)`, and `world.contains(id)`. The raw `hecs::World` and
`EntityRef` are not reachable outside the crate: there is no `ecs()` accessor, and
`entity_ref` is `pub(crate)` for trusted internal use such as snapshot
serialization.

This is load-bearing. hecs does runtime borrow checking, so a shared
`&hecs::World` can still hand out `&mut C`; exposing it would let a reader mutate
below the mutator layer, silently skipping the identity index, despawn cascade,
reverse lists, persistence dirty set, and structural facts. `query` is therefore
bounded by the privately sealed
[`ReadQuery`](../../musce_core/src/world.rs): shared borrows and their tuples only,
never `&mut`, and an app cannot add an implementation. `get` returns a shared
`Ref`. Making the raw handle unreachable turns this boundary from convention into
a compile error.

## Typed mutation contract

The only public way to change persisted components is through `World` mutators:
`set_component`/`insert`/`remove`/`modify` and `move_entity`/`relate`. A raw
`ecs.despawn` would skip cascades, a raw spawn could create an `Id`-less entity,
and a raw `get::<&mut C>` would omit a change from the next delta snapshot.

Typed `insert<C>`/`remove<C>`/`modify<C>` enforce the same structural guards as
tag-driven mutation. `Id` and every registered `RelTarget<R>` are rejected before
a mutable component borrow or callback can run. Their absence contract is exact:

- missing entity: contextual `MutateError` for all three;
- missing component on `remove` or `modify`: `Ok(false)`, with no dirty mark or
  fact, and `modify` does not invoke its closure;
- successful `insert`: `Ok(())`.

The relation guard depends on every app relation being registered before load or
seed, as required by [engine-and-app.md](engine-and-app.md).

## Bulk mutation boundary

Current write mutators are per-entity: `modify(id, f)` performs an `EntityId` to
hecs-handle lookup and archetype fetch on every call. A system that writes one
component across many entities each tick therefore pays that lookup per entity,
where a raw mutable query would make one columnar pass.

A sanctioned bulk mutator (`modify_each`) can close that gap without reopening the
raw-borrow hole: iterate internally, collect touched ids, then mark them dirty and
emit `ComponentChanged` after releasing the ECS query. It remains deferred until a
real hot consumer determines whether the API needs one component or a filtered
multi-component query, and whether its closure reports whether it changed a value.
This is an additive performance seam, not permission to expose raw mutation.

## Relation traversal

Relations promising acyclicity implement `AcyclicRelation`; ancestor and descendant
walks require that marker, so a cyclic relation cannot enter a tree walk.
`ancestors` is an allocation-free iterator, immediate target first, letting
`find`/`any`/`last` terminate without materializing the chain. A caller needing
ownership across mutation explicitly collects. `walk_descendants` owns only its DFS
stack and gives the consumer `Walk::{Descend, Prune, Stop}` control. Descendant
order is unspecified because reverse lists are unordered.

Proximity queries are different: they need an app-defined secondary index rather
than a relation walk. See [indexes.md](indexes.md).
