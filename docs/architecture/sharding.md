# Sharding

> Status: not built. Only the seams below are kept now. The point of this
> document is to record what those seams are and why, so sharding stays possible
> without being built prematurely.

Sharding means a large world split across processes, with each entity **owned by
one shard at a time**, partitioned spatially. It will not be necessary for a long
time, but a few cheap conventions kept now make it reachable without a rewrite.

## Seams kept now

- **Global `EntityId`** distinct from `hecs::Entity` (see
  [ecs-and-relations.md](ecs-and-relations.md)). The single most important seam:
  references that cross a shard boundary use the global id, not a local handle.
- **Message-shaped interaction.** Entities affect each other by message, never by
  synchronous reach-in, so an interaction can become a cross-process message
  later without changing call sites.
- **Zone as a first-class concept.** A shard is "a set of zones." The
  persistence schema already extracts a `zone` column for shard-scoped loads.
- **A locator indirection** (`EntityId -> shard`) that today always answers
  "local." All addressing routes through it, so there is one place to make it
  real.
- **DB-owned id allocation**, so a hub can later hand disjoint id ranges to
  shards from the same source.

## The room MUD advantage

A room-based world is far easier to shard than a continuous one. You generally
cannot act on something in another room, so the shard boundary goes **at room
exits**, where interaction is already discrete. Movement between rooms is already
an atomic event; when it crosses a shard line it simply becomes a handoff.

## Two kinds of crossing

- **Migration** (an entity changes owner, e.g. walking between zones): handled by
  **handoff**. This is the common case and the tractable one.
- **Interaction** (entities stay put, one affects another across the line): rare
  by construction in a room MUD. If ever needed it is a shard-to-shard message,
  never a DB read (the DB is the lagging saved state, not a live RPC bus).

## Handoff (when built)

Handoff is a transaction across two shards whose failure modes are entity
duplication or loss, so it commits **through the durable layer**:

1. Source shard freezes the entity and buffers messages addressed to it.
2. Serialize its state.
3. Commit the ownership change durably (the DB is the arbiter on crash).
4. Destination instantiates and acks; source removes its copy and flushes
   buffered messages onward.

The locator update must be atomic with the commit, and in-flight messages must
forward to the new owner.

## Topology: a leader, not a democracy

Coordination is **hub-and-spoke**, not peer-to-peer. At MUD scale (a handful of
shards, ever) distributed consensus is the wrong complexity. A single hub owns
the locator and handoff arbitration and hosts global services; spatial shards are
workers that route to it. The hub is not on the per-tick hot path, its
authoritative state is DB-backed and restartable, and a hot global service can
graduate into its own process individually.

## Global state is a separate concern

Things that belong to no room (chat channels, the economy, the world clock,
presence) are **services**, not spatial entities, addressed by message. Do not
force them into a spatial shard. And much "global" state is actually a pure
function of time (clock, day/night, formulaic weather): derive it locally from a
shared epoch with zero coordination rather than communicating it.
