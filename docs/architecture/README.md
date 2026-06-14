# MUSCE Architecture

MUSCE is an ECS-based MUD engine in Rust, aimed at a deep, emergent simulation
(think "Dwarf Fortress MUD") on a room-based world rather than a continuous
grid. It is a long-term project; this directory records the decisions that shape
it and why, so they survive between bursts of work.

## Cross-cutting principles

These hold across every subsystem:

- **World-as-truth.** The in-memory ECS world is authoritative. The database is
  its persisted form, written and read but never queried at runtime.
- **One authoritative sim thread.** A single thread owns the world and the tick
  loop. Everything else (networking, persistence) runs on other threads and
  communicates by message: commands in, events out.
- **Atomic mutation, no rollback.** A mutation either passes all its checks and
  commits, or it is rejected before changing anything; once it begins mutating it
  cannot fail. The single sim thread gives this for free, so the engine never
  needs transactions, rollback, or two-phase commit within a tick. Do not reach
  for that machinery: the absence of it is a design choice, not a gap. Validation
  is the only veto point; reactions respond to what happened, they never unwind
  it.
- **Message-shaped interaction.** Entities affect each other through messages
  addressed by `EntityId`, never by synchronously reaching into another
  entity's components. This is what keeps later sharding reachable.
- **Global identity.** Every entity has a stable `EntityId` distinct from its
  local `hecs::Entity` handle, so references survive persistence and shard
  boundaries.
- **Seams, not machinery.** Sharding, scripting, and parallelism are designed
  for but not built. We keep the conventions that preserve the option and
  refuse to build the framework until the need is real.

## Documents

- [ecs-and-relations.md](ecs-and-relations.md) — the ECS, identity, the generic
  relation layer, containment, and how queries work.
- [persistence.md](persistence.md) — World-as-truth, the snapshot model, the
  blob schema, and the save/confirm contract.
- [concurrency.md](concurrency.md) — the threading model, the tick pipeline, and
  why there is no auto-scheduler.
- [actions.md](actions.md) — the `Action` vocabulary as the single mutation path,
  the source-to-executor model, and the mutator/action layering. *(Proposed; not
  implemented.)*
- [sequences.md](sequences.md) — timed behavior as components, sequences and
  effects on a shared skeleton, and how they differ from systems. *(Proposed; not
  implemented.)*
- [networking-and-sessions.md](networking-and-sessions.md) — transports behind one
  `Connection`, input modes, and the session/control model (embodiment vs modal
  overlay, the account floor, staff multi-puppet). *(Proposed; not implemented.)*
- [sharding.md](sharding.md) — the deferred sharding plan and the seams kept now
  to make it possible.

## Status

Built:

- `musce_core`: world, identity, relation layer, containment, JSON snapshot.
- `musce_persistence`: World-as-truth save/load with a SQLite backend.
- `musce_host`: the tick loop (fixed cadence, `TickCtx` carrying both clocks),
  boot load, periodic + graceful-shutdown persistence.

Deferred (with seams in place where noted):

- Game systems: the phase pipeline runs, but no systems yet (designed in
  actions.md and sequences.md).
- Networking: transports, sessions, accounts, puppets (designed in
  networking-and-sessions.md).
- Postgres backend (same schema, JSONB).
- Sharding: locator, hub, entity handoff.
- A scripting layer for builders.
- Relationship traversal index, spatial proximity index, coordinates.
- Sense propagation (sound/smell/light) as timed exit-graph walks.
- Command journal for sub-snapshot crash recovery; dirty-tracked snapshots.
