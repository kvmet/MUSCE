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
  relation layer, containment, and relation invariants.
- [world-queries-and-mutation.md](world-queries-and-mutation.md): the sealed public
  read boundary, typed mutator contracts, and allocation-free relation traversal.
  *(Built; a sanctioned bulk mutator remains consumer-shaped and deferred.)*
- [persistence.md](persistence.md): World-as-truth, the delta snapshot model, the
  blob schema, and the save/confirm contract.
- [cold-storage.md](cold-storage.md): the cold content store (`KvStore`), its async
  off-thread cold-op path, and why dedup and content-addressing are app concerns.
  *(Built: `KvStore` and the wired book `read`/`inscribe` path.)*
- [concurrency.md](concurrency.md): the threading model, the tick pipeline, and
  why there is no auto-scheduler. *(Built: the sim thread, the tick loop, and the
  system pipeline carrying `App.systems`.)*
- [actions.md](actions.md): the `Action` vocabulary as the single mutation path,
  the structural-only executor, atomicity, and where rules and perception live.
  *(Built: the executor; the core verbs and seed live in `musce_ref`.)*
- [facts.md](facts.md): the structural-fact/reaction channel: the selection
  principle (a fact recovers only what a reaction cannot reconstruct), the
  `Destroyed`/`Moved`/`LocusChanged` facts, and the carried-subtree boundary.
  *(Built.)*
- [indexes.md](indexes.md): the generic secondary-index crate (`musce_index`), the
  `World` resource store it is homed in, its `ComponentChanged`-driven maintenance,
  and the reference spatial consumer (`Xyz` on rooms, `@nearby`). *(Built.)*
- [command-dispatch.md](command-dispatch.md): the command/action boundary, the
  `CommandTable` dispatch registry with prefix lookup, and the `Event` output
  channel with sim-side audience resolution. *(Built.)*
- [admin-verbs.md](admin-verbs.md): the admin/builder `@`-verbs and the
  type-erased reflection primitives they ride (the full structural `Action` set,
  `SetComponent` granularity, the generic mutators and guards). *(Built.)*
- [authorization.md](authorization.md): authentication vs authorization, the
  account record and its columnar store, capabilities and the verdict (with the
  quell rule), and the async auth flow. *(Built: the account model and store, the
  interner and verdict, the off-thread account task, the host wiring, and real
  password login and self-service `@password` change, argon2 verify/hash off-thread.
  Operator-set passwords and OAuth deferred.)*
- [engine-and-app.md](engine-and-app.md): the boundary between the engine
  substrate and an app built on it, the `App` the runtime is parameterized over,
  and the in-repo reference app `musce_ref`. *(Built.)*
- [sequences.md](sequences.md): timed behavior as components, sequences and
  effects on a shared skeleton, and how they differ from systems. *(Built, in
  `musce_ref`: the `Steps`/`Sequences` components, the `sequence_sweep` system, and
  a seeded patroller and burning torch.)*
- [affordances.md](affordances.md): the engine-owned canonical affordance
  representation: actor plus typed inputs/results, functional state slots,
  derived locus, grounding and substitution, guards, resolution contracts, gates,
  and schema validation. *(Built: canonical identifiers, typed values and
  input/result declarations, formulas/effects, partial bindings, grounded actions,
  outcomes, typed state-reader registration, closed-condition evaluation, finite
  symbol domains, qualitative gauge regions, immutable schema/handler
  registration, structural schema validation, reverse effect indexing, the shared
  performer, its typed execution/observation/narration boundary, and one boot-time
  registry injected into command and system contexts, and the Rust `affordance!`
  authoring macro. Reference text, pointing, systems, sequences, and agency are
  migrated; a reusable generated effect-oracle harness remains pending.)*
- [affordance-authoring.md](affordance-authoring.md): the Rust `affordance!`
  description language, generated typed handler interface, closed-vocabulary
  boundary, and path to future non-Rust front ends. *(Built and used by all six
  reference gameplay affordances.)*
- [affordance-contracts.md](affordance-contracts.md): ordered applicability
  guards, deterministic/contested/opaque resolution, unconditional effects, and
  executable-oracle obligations. *(Built: structural registration checks, shared
  grounding/gate/guard execution, resolution enforcement, result validation, and
  post-commit typed narration, plus reference content tests; reusable
  effect-oracle harness pending.)*
- [gauges.md](gauges.md): the split between stored components and derived gauges,
  raw normalized readings, registered qualitative regions, strict directional
  effects, and bounded QSIM regression. *(Built: value vocabulary, evaluator and
  region registration, and one-sided threshold evaluation; effect contracts,
  oracles, and planner integration pending.)*
- [offers.md](offers.md): parameter-aware affordance enumeration for pointing
  clients, partial typed input substitutions, result declarations,
  missing-input picks, and the generic perform wire shape. *(Built.)*
- [agency/](agency/README.md): autonomous behavior on the shared affordance layer:
  drives, goal arbitration, backward planning over comparable effects, knowledge-
  scoped parameter binding, QSIM gauges, and grounded execution with replanning.
  *(Canonical categorical planner, arbiter, and pursuit driver built; result
  dependencies, full interference, and QSIM regression deferred.)*
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
  is app vocabulary), the structural-fact channel
  (`Fact::Destroyed`/`Moved`/`LocusChanged`, emitted at the mutator layer; see
  facts.md), compile-time acyclic ancestor/descendant walking with caller-controlled
  pruning, JSON snapshot, and a transient `World` resource store for derived,
  non-persisted singletons (type-keyed, snapshot-excluded; see indexes.md).
  (Permissions are
  no longer a core marker: authorization is account-scoped, see authorization.md.)
- `musce_persistence`: World-as-truth save/load behind one `WorldStore` handle
  chosen by URL scheme, with SQLite and Postgres backends sharing one schema (the
  per-component-row layout, `data` as JSON text), plus the cold content store
  (`KvStore`: `kv_get`/`kv_put` over a `key -> BLOB`/`BYTEA` table) for large,
  rarely-read payloads kept off-heap, plus the `accounts` table (`AccountStore`:
  columnar per-account rows, `account_by_username`/`account_upsert`/`any_superuser`)
  holding the auth layer's records in the same store (see authorization.md).
- `musce_host`: the runtime as a library, parameterized by an injected `App`
  (`run(store, config, shutdown, app)`): the tick loop (fixed cadence, `TickCtx`
  carrying both clocks), boot load, periodic + graceful-shutdown persistence, the
  session floor (`@quit`/`@who`/`@help`/`@play`, the actor choice app-injected, plus
  the account-auth verbs `@operator`/`@login`/`@account`/`@grant`/`@revoke`/`@quell`,
  whose store-touching work runs off-thread on an account task), and a single command
  dispatcher draining the inbox each
  tick:
  lifecycle `@`-verbs to the floor, other `@`-verbs to the app's capability-gated
  admin table, bare commands to the embodiment frame. Authorization is resolved to a
  `Verdict` at the dispatch seam from each connection's session-cached account
  authorization, filled by the off-thread account task that owns account-store
  access and runs the app's login veto; the account record, its store, and the
  verdict primitive live in `musce_auth`/`musce_persistence`/`musce_action` (see
  authorization.md). It also runs a cold-content task that
  owns the `KvStore` and serves the app's cold reads/writes (`ColdOp`) off the sim
  thread, delivering results back through the event outbox, with an app-injected
  `decode_cold` turning opaque cold bytes into deliverable text. After draining commands it runs the app's
  injected systems (`App.systems`) on the phase pipeline, resolving their output
  through the same audience resolver, and runs `App.register` against a fresh
  world before load so an app's own component types deserialize and persist. Holds
  no app content; library-only (no binary).
- `musce_auth`: a pure domain leaf for account identity and authentication: the
  `Account` record (v7-UUID id, unique mutable username, nullable PHC credential
  hash, capability names, the `su` and `status` axes, opaque `app_data`) and
  argon2id password hashing (`hash_password`/`verify_password`, PHC-encoded, run off
  the sim thread). Holds no storage (that lives in `musce_persistence`) and no
  capability vocabulary or verdict (that lives in `musce_action`). See
  authorization.md.
- `musce_net`: raw TCP line-mode transport behind a transport-agnostic
  `Connection`, plus the commands-in/events-out pipe and event router. The
  authenticated session floor lives above it in `musce_host`; password-bearing
  commands are loopback-only until encrypted transport is available.
- `musce_proto`: the wire vocabulary (`Command`/`Input`, `Outgoing` with its
  connection-bound `Delivery`, `EventKind`, `ConnectionId`, `Capabilities`), a
  dependency-free leaf shared by net and host. The world-addressed authoring form
  (`Event`/`Audience`) lives in `musce_action`, since it never crosses to net.
- `musce_action`: the engine's action layer, free of app content. The
  structural executor (the full `Action` set:
  `Move`/`Relate`/`Unrelate`/`Create`/`Destroy`/`SetComponent`/`RemoveComponent`,
  returning the action's subject), the `CommandTable` lookup and public `register`,
  the `Gate` variants (`Open`/`Cap(CapId)`) with the account-scoped capability check
  (`CapId`/`CapSet`/`Verdict`/`permits`, the `CapRegistry` name->id interner, and
  `Verdict::resolved` carrying the quell rule, plus the verdict carried read-only on
  `Ctx`), the foundational gauge value algebra (`GaugeId`/`GaugeLevel`/
  `GaugeDirection`/`GaugeTarget`), and the additive canonical affordance schema
  substrate (`schema::{AffordanceId, Parameter, Value, Formula, Effect,
  GroundAction, ActionOutcome, ...}`) with typed state readers and qualitative
  gauge evaluation (`state::StateRegistry`), plus immutable canonical
  registration and execution (`AffordanceRegistryBuilder`/`AffordanceRegistry`,
  `Ctx::perform`, `SystemCtx::perform`),
  and `dispatch_command` (run by both the embodiment and
  admin frames), `Ctx` and its public emit API (the surface an app's verb handlers
  program against), `SystemCtx` and the `System` type (the tick-loop analogue of
  `Ctx`/`Handler`: a system mutates through `execute` and emits room-addressed
  output, with both clocks and no actor), the conn->actor audience index
  (`Actors`, derived from the floor's session attachments resolved through
  `Focus`), and the sim-side audience resolver.
- `musce_macros`: the dependency-light procedural authoring front end. It parses
  app `affordance!` declarations, rejects declaration-local errors at compile
  time, and emits only the typed adapters and canonical `musce` action types.
- `musce_ref`: the reference app and the worked example of standing an app up on
  the engine. Owns the bare verbs (`look`, `examine`/`x`, `read`, `inscribe`,
  `inventory`/`i`, `go`/bare direction, `take`, `drop`, `put`, `eat`, `give`,
  `pilot`, `release`, `say`, `tell`, `wave`, `attack`/`kill`, `help`) and the
  admin/builder verbs
  (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`/`@destroy`/`@purge`/`@possess`/`@unpossess`)
  and their parsing (gated on the app's own `build`/`possess` capabilities), the
  unified
  name resolver (a typed noun matches a thing's `Name` exact-then-word-prefix, then
  its app-side `Aliases`, then a `Description` substring; movement resolves an exit
  through the same path), its own kind markers
  (`item`/`creature`/`container`/`exit`/the player avatar, all app vocabulary the
  engine never interprets, with `container` its first consumers: `put` stashes a
  held thing in it, `give` hands one to a being, and `examine` reveals its
  contents), the combat stat components (`Special`, the seven-stat
  block, and `Health`) landed with their first consumer `attack` (Strength drains a
  foe's `Health`; a lethal blow destroys it, converging on the `death_cry` reaction),
  the `Readable` book (the first cold-content consumer: a resident entity holds only
  the cold key, `read` fetches its text and `inscribe` overwrites it through the
  engine's async cold-op path, decoded by a UTF-8 `decode_cold`),
  the takeable rule, the control rule, and the app-owned canonical `take`, `drop`,
  `put`, `eat`, `give`, and `go` affordances (including the `Locked`-exit guard and
  fixed narration shared by commands, agency, wanderers, and sequences), the
  code-seeded starter world (with a controllable drone), the `@play` actor policy,
  and its own tick-loop systems (a `Wander` marker plus the `wander` system that
  drifts uncontrolled wanderers between rooms, the `death_cry` reaction that
  narrates a destroyed thing's demise from the `Fact` channel, and the sequence
  layer: the `Steps`/`Sequences` components, the `sequence_sweep` system, and a
  seeded patrolling sentry and burning torch); the `offers` affordance-enumeration
  query (the renderer-side "what can I do to this?" read over the veto model, with a
  three-way `OfferStatus`; see offers.md); builds the `App`
  and has `main` plus the end-to-end test. A real app forks this crate.
- `musce_index`: a generic, type-agnostic secondary index over a component (explicit
  many/unique registration, exact `get`, and keyed borrowed uniqueness diagnostics),
  maintained incrementally off the
  `ComponentChanged` trigger and
  `Destroyed`, homed in a `World` resource (transient, never persisted). Its
  reference consumer is `musce_ref`'s coordinate layer: an integer `Xyz` on rooms,
  the `xyz_cell`/`xyz_level` indexes, `near` range queries, and the
  `@setpos`/`@pos`/`@nearby` verbs. See indexes.md.

Deferred (with seams in place where noted):

- Affordance/agency extensions: generated effect-oracle infrastructure,
  fresh-result dependencies, complete functional-assignment interference,
  directional gauge-effect verification, and QSIM regression. The
  target design is in
  `affordances.md`, `affordance-authoring.md`, `affordance-contracts.md`,
  `gauges.md`, `offers.md`, and `agency/`.
- App logic: timed behavior (sequences and effects) on a shared skeleton is
  **built** in `musce_ref` (the `Steps`/`Sequences` components, the
  `sequence_sweep` system, a seeded patroller and torch; see sequences.md), over
  the phase pipeline that carries the app's systems and the reaction /
  structural-fact channel the torch converges with (`death_cry` narrates the
  burn-out; see actions.md and concurrency.md). What remains deferred: a runtime
  verb to attach/detach a sequence (it is seed-only for now), branch/condition
  intents (the scripting layer below), bounded-repeat effects (a repeat-count),
  and the seeded-world RNG for stochastic beats. The admin builder verbs
  (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`/`@destroy`/`@purge`/`@possess`/`@unpossess`)
  are built, riding the structural action set through the capability-gated admin
  frame.
- Networking: encrypted remote authentication, SSH, OAuth as an additional auth
  method, the
  gameplay possess-gate, the `p1`/`p2` multi-puppet slots, and modal overlays
  (designed in networking-and-sessions.md). Raw TCP, the session floor, the session
  attachment that `@play` sets, durable `Controls`/`Focus` embodiment, and the
  `@possess`/`@unpossess` admin verbs are built. The account/authorization layer is
  built, including real password login (argon2 verify on `@login`, hash on `@account
  new`, both off-thread) and self-service password change (`@password`/`@pw`), with
  every password-bearing command loopback-only until encryption lands and
  `@operator` the passwordless loopback bootstrap; operator-set passwords and OAuth
  are deferred (see authorization.md).
- Doors: the optional `Portal`/`Through` layer over the built exit entities (a
  two-sided lockable door reading identically from both rooms), and explicit exit
  aliases. Designed in ecs-and-relations.md. A minimal `Locked` exit marker now
  exists in `musce_ref` as the first `can_traverse` veto (the seam a richer door /
  skill-check check grows from), but two-sided door state is still deferred.
- Sharding: locator, hub, entity handoff.
- A scripting layer for builders.
- A non-Rust affordance authoring surface for hot reload or non-Rust content
  authors. The target Rust `affordance!` description language and its canonical
  AST are designed in
  [affordance-authoring.md](affordance-authoring.md).
- Relationship traversal index (the generic secondary index and the spatial
  proximity index over room coordinates are **built**; see indexes.md).
- Sense propagation (sound/smell/light) as timed exit-graph walks.
- Command journal for sub-snapshot crash recovery. (Dirty-tracked delta snapshots
  are **built**: a save serializes only entities changed since the last one, with a
  drain-and-restore confirm contract; see persistence.md.)
