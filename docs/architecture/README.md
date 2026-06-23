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
  why there is no auto-scheduler. *(Built: the sim thread, the tick loop, and the
  system pipeline carrying `Game.systems`.)*
- [actions.md](actions.md): the `Action` vocabulary as the single mutation path,
  the structural-only executor, the structural-fact channel reactions read,
  atomicity, and where rules and perception live. *(Built: the executor, the
  `Fact` channel; the core verbs and seed live in `musce_ref`.)*
- [command-dispatch.md](command-dispatch.md): the command/action boundary, the
  `CommandTable` dispatch registry with prefix lookup, and the `Event` output
  channel with sim-side audience resolution. *(Built.)*
- [admin-verbs.md](admin-verbs.md): the admin/builder `@`-verbs and the
  type-erased reflection primitives they ride (the full structural `Action` set,
  `SetComponent` granularity, the generic mutators and guards). *(Built.)*
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
  `Controls` and `Focus` relations behind durable embodiment), relation-backed exit
  entities (an `Exit` marker plus the general `Label` component, wired by
  `LeadsFrom`/`LeadsTo` with the `DespawnSources` cascade), the `Staff`
  permission marker, the structural-fact buffer (`Fact`, emitted at the `despawn`
  mutator layer), JSON snapshot.
- `musce_persistence`: World-as-truth save/load with a SQLite backend.
- `musce_host`: the runtime as a library, parameterized by an injected `Game`
  (`run(store, config, shutdown, game)`): the tick loop (fixed cadence, `TickCtx`
  carrying both clocks), boot load, periodic + graceful-shutdown persistence, the
  account floor (`@quit`/`@who`/`@help`/`@play`, the actor choice game-injected),
  and a single command dispatcher draining the inbox each tick: lifecycle `@`-verbs
  to the floor, other `@`-verbs to the game's staff-gated admin table, bare
  commands to the embodiment frame. After draining commands it runs the game's
  injected systems (`Game.systems`) on the phase pipeline, resolving their output
  through the same audience resolver, and runs `Game.register` against a fresh
  world before load so a game's own component types deserialize and persist. Holds
  no game content; library-only (no binary).
- `musce_net`: raw TCP line-mode transport behind a transport-agnostic
  `Connection`, plus the commands-in/events-out pipe and event router. The
  session floor (`@quit`/`@who`/`@help`/`@play`) is reachable; auth is stubbed.
- `musce_proto`: the shared command/event vocabulary (`Command`, `Event`,
  `Audience`, `EventKind`, `ConnectionId`, `Capabilities`), depended on by net,
  action, and host so the action layer never touches the transport.
- `musce_action`: the engine's action layer, free of game content. The
  structural executor (the full `Action` set:
  `Move`/`Relate`/`Unrelate`/`Create`/`Destroy`/`SetComponent`/`RemoveComponent`,
  returning the action's subject), the `CommandTable` lookup and public `register`,
  the `Gate`
  tiers (`Open`/`Staff`) and `dispatch_command` (run by both the embodiment and
  admin frames), `Ctx` and its public emit API (the surface a game's verb handlers
  program against), `SystemCtx` and the `System` type (the tick-loop analogue of
  `Ctx`/`Handler`: a system mutates through `execute` and emits room-addressed
  output, with both clocks and no actor), the conn->actor audience index
  (`Actors`, derived from the floor's session attachments resolved through
  `Focus`), and the sim-side audience resolver.
- `musce_ref`: the reference game and the worked example of standing a game up on
  the engine. Owns the bare verbs (`look`, `go`/bare direction, `take`, `drop`,
  `pilot`, `release`, `say`, `help`) and the admin/builder verbs
  (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`/`@destroy`/`@purge`/`@possess`/`@unpossess`)
  and their parsing, the unified
  name resolver (movement resolves an exit through it, matching a `Label`
  exact-then-prefix with a description-substring fallback), the takeable rule and
  the control rule, narration prose, the
  code-seeded starter world (with a controllable drone), the `@play` actor policy,
  and its own tick-loop systems (a `Wander` marker plus the `wander` system that
  drifts uncontrolled wanderers between rooms, and the `death_cry` reaction that
  narrates a destroyed thing's demise from the `Fact` channel); builds the `Game`
  and has `main` plus the end-to-end test. A real game forks this crate.

Deferred (with seams in place where noted):

- Game logic: timed behavior (sequences and effects) on a shared skeleton,
  designed in sequences.md. The phase pipeline itself now carries the game's
  systems (the reference game's wandering creature is the first; see
  concurrency.md), and the reaction / structural-fact channel is live (systems
  read `Fact`s from `SystemCtx::facts`; the reference `death_cry` reacts to
  destruction; see actions.md). The admin builder verbs
  (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`/`@destroy`/`@purge`/`@possess`/`@unpossess`)
  are built, riding the structural action set through the staff-gated admin frame.
- Networking: WebSocket/SSH transports, real accounts/auth, the gameplay
  possess-gate, the `p1`/`p2` multi-puppet slots, and modal overlays (designed in
  networking-and-sessions.md). Raw TCP, the session floor, the session attachment
  that `@play` sets, durable `Controls`/`Focus` embodiment, and the
  `@possess`/`@unpossess` admin verbs are built.
- Doors: the optional `Portal`/`Through` layer over the built exit entities (a
  two-sided lockable door reading identically from both rooms), and explicit exit
  aliases. Designed in ecs-and-relations.md.
- Postgres backend (same schema, JSONB).
- Sharding: locator, hub, entity handoff.
- A scripting layer for builders.
- Relationship traversal index, spatial proximity index, coordinates.
- Sense propagation (sound/smell/light) as timed exit-graph walks.
- Command journal for sub-snapshot crash recovery; dirty-tracked snapshots.
