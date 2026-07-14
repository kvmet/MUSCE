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
  the structural-only executor, atomicity, and where rules and perception live.
  *(Built: the executor; the core verbs and seed live in `musce_ref`.)*
- [facts.md](facts.md): the structural-fact/reaction channel: the selection
  principle (a fact recovers only what a reaction cannot reconstruct), the
  `Destroyed`/`Moved`/`LocusChanged` facts, and the carried-subtree boundary.
  *(Built.)*
- [command-dispatch.md](command-dispatch.md): the command/action boundary, the
  `CommandTable` dispatch registry with prefix lookup, and the `Event` output
  channel with sim-side audience resolution. *(Built.)*
- [admin-verbs.md](admin-verbs.md): the admin/builder `@`-verbs and the
  type-erased reflection primitives they ride (the full structural `Action` set,
  `SetComponent` granularity, the generic mutators and guards). *(Built.)*
- [engine-and-game.md](engine-and-game.md): the boundary between the engine
  substrate and a game built on it, the `Game` the runtime is parameterized over,
  and the in-repo reference game `musce_ref`. *(Built.)*
- [authorization.md](authorization.md): the permission model that replaced the
  `Staff` gate: account-scoped capabilities, the superuser account bit, `@quell`
  (dropping su and quellable caps), and the out-of-band boundaries on su.
  *(Authorization built, incl. the runtime account surface; real authentication
  pending.)*
- [accounts.md](accounts.md): the implementation half of authorization: resolving a
  connection to a verdict at dispatch, the account authority (its own `musce_auth`
  crate) and its relational SQLite store, account identity, the runtime account
  mutators, and bootstrapping. *(Built, including the durable store; real
  authentication pending.)*
- [sequences.md](sequences.md): timed behavior as components, sequences and
  effects on a shared skeleton, and how they differ from systems. *(Built, in
  `musce_ref`: the `Steps`/`Sequences` components, the `sequence_sweep` system, and
  a seeded patroller and burning torch.)*
- [networking-and-sessions.md](networking-and-sessions.md): transports behind one
  `Connection`, input modes, and the session/control model (embodiment vs modal
  overlay, the account floor, staff multi-puppet). *(Built: raw TCP, session
  floor, and durable `Controls`/`Focus` embodiment; the rest proposed.)*
- [sharding.md](sharding.md): the deferred sharding plan and the seams kept now
  to make it possible.
- [benchmarks.md](benchmarks.md): the criterion benchmark set, where micro vs
  macro benches live and why, how to run and read them, and the named-baseline
  workflow for measuring a change's gain. *(Built.)*

## Status

Built:

- `musce_core`: world, identity, relation layer, containment and control (the
  `Controls` and `Focus` relations behind durable embodiment), relation-backed exit
  connectivity (the `LeadsFrom`/`LeadsTo` relations plus the general `Name`
  component, wired with the `DespawnSources` cascade; the `Exit` kind marker itself
  is game vocabulary), the structural-fact channel
  (`Fact::Destroyed`/`Moved`/`LocusChanged`, emitted at the mutator layer; see
  facts.md), JSON snapshot. (Permissions are
  no longer a core marker: authorization is account-scoped, see authorization.md.)
- `musce_persistence`: World-as-truth save/load behind one `WorldStore` handle
  chosen by URL scheme, with SQLite and Postgres backends sharing one schema (the
  per-component-row layout, `data` as JSON text), plus the cold content store
  (`KvStore`: `kv_get`/`kv_put` over a `key -> BLOB`/`BYTEA` table) for large,
  rarely-read payloads kept off-heap.
- `musce_host`: the runtime as a library, parameterized by an injected `Game`
  (`run(store, config, shutdown, game)`): the tick loop (fixed cadence, `TickCtx`
  carrying both clocks), boot load, periodic + graceful-shutdown persistence, the
  account floor (`@quit`/`@who`/`@help`/`@play`, the `@operator`/`@login` elevation
  stubs, the operator's `@account`/`@grant`/`@revoke` account admin, and `@quell`, the
  actor choice game-injected), and a single command dispatcher draining the inbox each
  tick:
  lifecycle `@`-verbs to the floor, other `@`-verbs to the game's capability-gated
  admin table, bare commands to the embodiment frame. It resolves each connection's
  account to an authorization verdict at the dispatch seam (the authority itself
  lives in `musce_auth`, re-exported as `musce_host::auth`), and persists account
  mutations through an async writer task fed by the sim loop's dirty-flag beat,
  the account analogue of the snapshot path. It also runs a cold-content task that
  owns the `KvStore` and serves the game's cold reads/writes (`ColdOp`) off the sim
  thread, delivering results back through the event outbox, with a game-injected
  `decode_cold` turning opaque cold bytes into deliverable text. After draining commands it runs the game's
  injected systems (`Game.systems`) on the phase pipeline, resolving their output
  through the same audience resolver, and runs `Game.register` against a fresh
  world before load so a game's own component types deserialize and persist. Holds
  no game content; library-only (no binary).
- `musce_auth`: the account authority as its own leaf crate, so account identity
  can serve consumers beyond the sim host (a web or oauth frontend, admin
  tooling). The caps registry, the account records and `AccountsSnapshot`, the
  live `Accounts` authority (first-account-su bootstrap, the last-su boot check,
  the runtime mutators, `verdict_for`), and the durable relational store: its own
  SQLite database (`accounts` / `account_caps` / `meta` with the persisted
  `next_id` high-water mark, so a deleted account's id is never reissued).
- `musce_net`: raw TCP line-mode transport behind a transport-agnostic
  `Connection`, plus the commands-in/events-out pipe and event router. The
  session floor (`@quit`/`@who`/`@help`/`@play`) is reachable; auth is stubbed.
- `musce_proto`: the wire vocabulary (`Command`/`Input`, `Outgoing` with its
  connection-bound `Delivery`, `EventKind`, `ConnectionId`, `Capabilities`), a
  dependency-free leaf shared by net and host. The world-addressed authoring form
  (`Event`/`Audience`) lives in `musce_action`, since it never crosses to net.
- `musce_action`: the engine's action layer, free of game content. The
  structural executor (the full `Action` set:
  `Move`/`Relate`/`Unrelate`/`Create`/`Destroy`/`SetComponent`/`RemoveComponent`,
  returning the action's subject), the `CommandTable` lookup and public `register`,
  the `Gate` variants (`Open`/`Cap(CapId)`) with the account-scoped capability check
  (`CapId`/`CapSet`/`Verdict`/`permits`, plus the verdict carried read-only on `Ctx`),
  and `dispatch_command` (run by both the embodiment and
  admin frames), `Ctx` and its public emit API (the surface a game's verb handlers
  program against), `SystemCtx` and the `System` type (the tick-loop analogue of
  `Ctx`/`Handler`: a system mutates through `execute` and emits room-addressed
  output, with both clocks and no actor), the conn->actor audience index
  (`Actors`, derived from the floor's session attachments resolved through
  `Focus`), and the sim-side audience resolver.
- `musce_ref`: the reference game and the worked example of standing a game up on
  the engine. Owns the bare verbs (`look`, `examine`/`x`, `read`, `inscribe`,
  `inventory`/`i`, `go`/bare direction, `take`, `drop`, `put`, `give`, `pilot`,
  `release`, `say`, `tell`, `wave`, `attack`/`kill`, `help`) and the
  admin/builder verbs
  (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`/`@destroy`/`@purge`/`@possess`/`@unpossess`)
  and their parsing (gated on the game's own `build`/`possess` capabilities), the
  unified
  name resolver (a typed noun matches a thing's `Name` exact-then-word-prefix, then
  its game-side `Aliases`, then a `Description` substring; movement resolves an exit
  through the same path), its own kind markers
  (`item`/`creature`/`container`/`exit`/the player avatar, all game vocabulary the
  engine never interprets, with `container` its first consumers: `put` stashes a
  held thing in it, `give` hands one to a being, and `examine` reveals its
  contents), the combat stat components (`Special`, the seven-stat
  block, and `Health`) landed with their first consumer `attack` (Strength drains a
  foe's `Health`; a lethal blow destroys it, converging on the `death_cry` reaction),
  the `Readable` book (the first cold-content consumer: a resident entity holds only
  the cold key, `read` fetches its text and `inscribe` overwrites it through the
  engine's async cold-op path, decoded by a UTF-8 `decode_cold`),
  the takeable rule and
  the control rule, the shared `do_move` traversal helper (the one rule-checked
  move path, with a `Locked`-exit veto, run by `go`, `wander`, and sequences
  alike), narration prose, the
  code-seeded starter world (with a controllable drone), the `@play` actor policy,
  and its own tick-loop systems (a `Wander` marker plus the `wander` system that
  drifts uncontrolled wanderers between rooms, the `death_cry` reaction that
  narrates a destroyed thing's demise from the `Fact` channel, and the sequence
  layer: the `Steps`/`Sequences` components, the `sequence_sweep` system, and a
  seeded patrolling sentry and burning torch); builds the `Game`
  and has `main` plus the end-to-end test. A real game forks this crate.

Deferred (with seams in place where noted):

- Game logic: timed behavior (sequences and effects) on a shared skeleton is
  **built** in `musce_ref` (the `Steps`/`Sequences` components, the
  `sequence_sweep` system, a seeded patroller and torch; see sequences.md), over
  the phase pipeline that carries the game's systems and the reaction /
  structural-fact channel the torch converges with (`death_cry` narrates the
  burn-out; see actions.md and concurrency.md). What remains deferred: a runtime
  verb to attach/detach a sequence (it is seed-only for now), branch/condition
  intents (the scripting layer below), bounded-repeat effects (a repeat-count),
  and the seeded-world RNG for stochastic beats. The admin builder verbs
  (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`/`@destroy`/`@purge`/`@possess`/`@unpossess`)
  are built, riding the structural action set through the capability-gated admin
  frame.
- Networking: WebSocket/SSH transports, real authentication (the loopback-only
  `@operator`/`@login` stubs stand in for now, resolving to real accounts), the
  gameplay possess-gate, the `p1`/`p2` multi-puppet slots, and modal overlays
  (designed in networking-and-sessions.md). Raw TCP, the session floor, the session
  attachment that `@play` sets, durable `Controls`/`Focus` embodiment, the account
  authority and capability gate (authorization slice 1), and the
  `@possess`/`@unpossess` admin verbs are built.
- Doors: the optional `Portal`/`Through` layer over the built exit entities (a
  two-sided lockable door reading identically from both rooms), and explicit exit
  aliases. Designed in ecs-and-relations.md. A minimal `Locked` exit marker now
  exists in `musce_ref` as the first `can_traverse` veto (the seam a richer door /
  skill-check check grows from), but two-sided door state is still deferred.
- Sharding: locator, hub, entity handoff.
- A scripting layer for builders.
- Relationship traversal index, spatial proximity index, coordinates.
- Sense propagation (sound/smell/light) as timed exit-graph walks.
- Command journal for sub-snapshot crash recovery; dirty-tracked snapshots.
