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

- [ecs-and-relations.md](ecs-and-relations.md): the ECS, identity, the generic
  relation layer, containment, and how queries work.
- [persistence.md](persistence.md): World-as-truth, the snapshot model, the
  blob schema, and the save/confirm contract.
- [concurrency.md](concurrency.md): the threading model, the tick pipeline, and
  why there is no auto-scheduler.
- [actions.md](actions.md): the `Action` vocabulary as the single mutation path,
  the structural-only executor, command dispatch as a registry, atomicity, and
  where rules and perception live. *(Built: the executor, dispatch, and the
  sim-side audience resolver; the core verbs and seed live in `musce_ref`.)*
- [admin-verbs.md](admin-verbs.md): the admin/builder `@`-verbs and the
  type-erased reflection primitives they ride (the full structural `Action` set,
  `SetComponent` granularity, the generic mutators and guards). *(Built;
  `@destroy` and dynamic possession `@possess` deferred.)*
- [engine-and-game.md](engine-and-game.md): the boundary between the engine
  substrate and a game built on it, the `Game` the runtime is parameterized over,
  and the in-repo reference game `musce_ref`. *(Built.)*
- [sequences.md](sequences.md): timed behavior as components, sequences and
  effects on a shared skeleton, and how they differ from systems. *(Proposed; not
  implemented.)*
- [networking-and-sessions.md](networking-and-sessions.md): transports behind one
  `Connection`, input modes, and the session/control model (embodiment vs modal
  overlay, the account floor, staff multi-puppet). *(Built: raw TCP, session
  floor, and durable `Controls`/`Focus` embodiment; the rest proposed.)*
- [sharding.md](sharding.md): the deferred sharding plan and the seams kept now
  to make it possible.

## Status

Built:

- `musce_core`: world, identity, relation layer, containment and control (the
  `Controls` and `Focus` relations behind durable embodiment), the `Staff`
  permission marker, JSON snapshot.
- `musce_persistence`: World-as-truth save/load with a SQLite backend.
- `musce_host`: the runtime as a library, parameterized by an injected `Game`
  (`run(store, config, shutdown, game)`): the tick loop (fixed cadence, `TickCtx`
  carrying both clocks), boot load, periodic + graceful-shutdown persistence, the
  account floor (`@quit`/`@who`/`@help`/`@play`, the actor choice game-injected),
  and a single command dispatcher draining the inbox each tick: lifecycle `@`-verbs
  to the floor, other `@`-verbs to the game's staff-gated admin table, bare
  commands to the embodiment frame. Holds no game content; library-only (no
  binary).
- `musce_net`: raw TCP line-mode transport behind a transport-agnostic
  `Connection`, plus the commands-in/events-out pipe and event router. The
  session floor (`@quit`/`@who`/`@help`/`@play`) is reachable; auth is stubbed.
- `musce_proto`: the shared command/event vocabulary (`Command`, `Event`,
  `Audience`, `EventKind`, `ConnectionId`, `Capabilities`), depended on by net,
  action, and host so the action layer never touches the transport.
- `musce_action`: the engine's action layer, free of game content. The
  structural executor (the full `Action` set:
  `Move`/`Create`/`Destroy`/`SetComponent`/`RemoveComponent`, returning the
  action's subject), the `CommandTable` lookup and public `register`, the `Gate`
  tiers (`Open`/`Staff`) and `dispatch_command` (run by both the embodiment and
  admin frames), `Ctx` and its public emit API (the surface a game's verb handlers
  program against), the conn->actor audience index (`Actors`, derived from the
  floor's session attachments resolved through `Focus`), and the sim-side audience
  resolver.
- `musce_ref`: the reference game and the worked example of standing a game up on
  the engine. Owns the bare verbs (`look`, `go`/bare direction, `take`, `drop`,
  `pilot`, `release`, `say`, `help`) and the admin/builder verbs
  (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`) and their parsing, name
  resolution, the takeable rule and the control rule, narration prose, the
  code-seeded starter world (with a controllable drone), and the `@play` actor
  policy; builds the `Game` and has `main` plus the end-to-end test. A real game
  forks this crate.

Deferred (with seams in place where noted):

- Game logic: `@destroy` (deferred pending the `@destroy`/`@purge` decision), then
  systems on the phase pipeline (designed in actions.md and sequences.md). The rest
  of the admin builder verbs (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`) are
  built, riding the structural action set through the staff-gated admin frame.
- Networking: WebSocket/SSH transports, real accounts/auth, dynamic possession
  (the `@possess`/`@release` admin verbs), and modal overlays (designed in
  networking-and-sessions.md). Raw TCP, the session floor, the session attachment
  that `@play` sets, and durable `Controls`/`Focus` embodiment are built.
- Postgres backend (same schema, JSONB).
- Sharding: locator, hub, entity handoff.
- A scripting layer for builders.
- Relationship traversal index, spatial proximity index, coordinates.
- Sense propagation (sound/smell/light) as timed exit-graph walks.
- Command journal for sub-snapshot crash recovery; dirty-tracked snapshots.
