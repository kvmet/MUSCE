# Actions and the Executor

> Status: **structural vocabulary built; engine/game split done.** The engine
> owns the structural executor
> (`Action::Move`/`Relate`/`Unrelate`/`Create`/`Destroy`/`SetComponent`/`RemoveComponent` +
> `execute` + `ExecError`), the `CommandTable` lookup and registration, `Ctx` and
> its emit API, and the sim-side audience resolver (`musce_action`), plus the
> shared vocabulary (`musce_proto`). The game content (the verbs `look`, `go`/bare
> direction, `take`, `drop`, `say`, `help`, name resolution, the seed world, the
> takeable rule, and the `@play` actor policy) lives in the reference game
> `musce_ref`,
> which builds the `Game` the runtime is parameterized over (see
> [engine-and-game.md](engine-and-game.md)). This document covers the core
> executor, the action vocabulary, the structural-fact channel, and atomicity; the
> command/action boundary, the dispatch registry, and the `Event` output channel
> are in [command-dispatch.md](command-dispatch.md), and the type-erased reflection
> primitives and the admin/builder `@`-verbs that ride them
> (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`/`@destroy`/`@purge`/`@possess`/
> `@unpossess`) are in [admin-verbs.md](admin-verbs.md).

## Action is the only thing that mutates the world

`Action` is the single vocabulary of world mutation. Several *sources* produce
actions; one *executor* applies them:

- a net `Command` (parsed input plus provenance) goes through a dispatch phase
  that validates and authorizes it into an `Action`, or rejects it
- a sequence step produces an `Action` (see `sequences.md`)
- an effect or other system produces an `Action`

```
execute(world, action) -> Result<EntityId, ExecError>
```

`execute` is **structural only**: it applies the typed mutator and enforces only
the invariants that hold for every source (the entity exists, the relation stays
acyclic), returning the action's subject `EntityId`, or `ExecError` on a
structural violation. It runs no gameplay rules and emits no perception events;
the action set is just the typed reflection of the `World` mutators.

### The structural-fact channel

Structural mutations emit typed **facts** for game logic to react to. A fact is an
*observation* of a mutation, not a mutation, so the rule that an action is the only
thing that mutates still holds: a reaction reads facts and may produce its own
actions, but the fact stream changes nothing on its own.

Facts are emitted at the **`World` mutator layer (`despawn`), not `execute`**, and
that placement is load-bearing. A single `@destroy` cascades through the relation
layer *below* `execute` (a destroyed room takes its exits with it via
`DespawnSources`); only the mutator recursion observes those cascade removals, so
emitting from `execute` would catch the targeted entity and miss its collateral.
`execute` and every verb call site therefore stay untouched.

The one fact today is `Fact::Destroyed { entity, last_room, name, cause }`.
`last_room` and `name` are a **pre-removal snapshot** (captured while the entity is
still live, between the cascade-handler loop and the index removal, because a
reaction reads them after it is gone): `name` is the `Description` (`None` if
absent, e.g. an exit carries a `Label`), `last_room` the `enclosing_room` (`None`
for a top-level room or a location-less entity). `cause` is `Direct` for the
targeted entity and `Cascade` for one swept up by a cascade; this discriminator
lets one reaction catch every removal in a recursive `@purge` (all `Direct`) yet
skip the collateral of a single `@destroy <room>` (room `Direct`, exits
`Cascade`). A `Cascade { root }` enrichment is deferred until a reaction needs to
group a cascade by origin.

Facts buffer on a transient `World` field, drained **once per tick** by
`Dispatch::run_systems` at the top of the system loop into the read-only
`SystemCtx::facts` slice every system sees. That timing sets the latency: a
command-driven mutation (`@destroy`/`@purge`, drained before `run_systems`) is
reacted to the **same tick**, while a fact a system emits is seen the **next tick**
(buffered after the drain), so no system sees another's fact within a pass and
system order is cosmetic. A reaction is just a `System` iterating `ctx.facts`; the
reference game's `death_cry` narrates a destroyed thing's demise to its room.

Gameplay rules and perception prose live one layer up, in the verb handlers. "The
take logic exists once" is achieved by shared rule/perception helpers (e.g. a
`do_move` used by both the player `go` command and, later, AI and sequences), not
by pushing rules into `execute`. Each engine primitive stays atomic and free of
intent: `execute` owns the world, handlers own meaning.

A **Command** is a request with provenance (it may be rejected); an **Action** is
the authorized, validated mutation it parses into. The command/action boundary,
the `CommandTable` registry that dispatches a parsed command to a verb, and the
`Event` output channel verbs emit into are covered in
[command-dispatch.md](command-dispatch.md). This document is the action set the
verbs resolve to.

## Three buckets

Sorting everything into three buckets keeps the layering clear:

1. **Mutators** (the `World` API, not player-facing): `spawn`, `despawn`,
   `relate`/`unrelate`, `insert`/`remove`/`set` component. The machine
   instructions, mostly built already in `world.rs`.
2. **Gameplay verbs** (rule-checked, event-emitting): `Move` (with take, drop,
   give, put), later attack, open. They compile to mutators and emit perception
   events.
3. **Admin verbs** (permission-gated, rule-bypassing by design): `@create`,
   `@destroy`, `@dig`, `@tel`/`@goto`/`@summon`, `@set`. They compile to the same
   mutators, directly, skipping gameplay rules. A builder spawning a sword should
   not run through "can you reach and take this."

Both verb buckets resolve to actions; the split is rule-checked vs
permission-gated, not two mechanisms. `@set`/`SetComponent` lives in bucket 3,
mapping straight to the component-insert mutator the way `@tel` maps to
`move_entity`. Gameplay never calls the generic setter.

## The action set, and verbs as sugar over it

The executor vocabulary is small and is pure world-mutation:

- `Move(entity, into)` — all containment movement
- `Relate / Unrelate(source, target, kind)` — non-containment relationships;
  `Move` is the containment face of `Relate`
- `Create` / `Destroy`
- `SetComponent / RemoveComponent`

Most verbs are parse-layer sugar that resolve to one action by computing a
destination and applying a rule predicate. Containment movement is the clearest
case: a room is just another container, so drop is give-to-the-room. All of these
are one `Move`:

| Verb | Action | Destination it computes | Rule |
|------|--------|------------------------|------|
| `take <i>` | `Move` | into me | reachable and takeable |
| `drop <i>` | `Move` | into my container (room) | I hold it |
| `give <i> <who>` | `Move` | into `who` | recipient accepts |
| `put <i> <c>` | `Move` | into container `c` | reachable, `c` accepts |
| `@tel <t> <dest>` | `Move` | into `dest` | admin |
| `@goto <t>` | `Move` | into `enclosing_room(t)` | admin |
| `@summon <t>` | `Move` | into `container_of(me)` | admin |
| `@create <kind>` | `Create` | spawn, then `Move` into my room | admin |
| `@destroy <t>` | `Destroy` | `despawn(t)` | admin |
| `@dig <dir> [name]` | `Create` + `Relate` | spawn a `Room`, then `Create` + `Relate` an exit entity each way | admin |

Communication mutates nothing, so it is not in the action vocabulary: mutation
funnels through `execute` (which emits no perception events), while output flows
out as `Event`s from the verb and system handlers, audience-resolved sim-side. The
Event channel and its audience model are covered in
[command-dispatch.md](command-dispatch.md).

## Atomicity: validate, then commit

Every handler is shaped validate -> mutate -> emit, and the boundary between
validate and mutate is the commit point. **All fallible checks precede the first
mutation, and the mutate phase is infallible by construction.** On the single sim
thread with exclusive `&mut World` this makes an action atomic for free: no
concurrency can interleave it, and there is no failure point partway through to
unwind. The engine therefore needs no transactions, rollback, or two-phase commit
inside a tick, and we deliberately do not add them. This is a standing decision,
not a missing feature; see the README principle.

`relate` in `world.rs` already embodies this: it returns `Err` for missing
entities and cycles up front, and only then runs `remove_source` / `insert_one` /
`add_source`, none of which can bail.

Two consequences:

- **Reactions respond, they do not veto.** A trap firing on entry does not
  un-move the entity; it reacts to a move that already committed, possibly by
  issuing a new Move to throw it back out. "You cannot enter" must be a pre-commit
  rule, not a post-event reaction. The veto/react split is exactly the
  validate/mutate line.
- **Compound actions front-load every check.** `@dig` creates a room and two exit
  links. No concurrency can split it, but a precondition that fails after the room
  exists would leave a half-dug room. Validate the whole compound before the first
  mutation, then the mutation sequence runs clean.

## No command buffer needed

ECS command buffers (Bevy `Commands`, flecs deferred mode) are exactly the mutator
set, buffered and flushed at a sync point because structural changes are illegal
during parallel system iteration. With no auto-scheduler (see `concurrency.md`),
the sim thread runs ordered systems with exclusive `&mut World`, so an action
mutates the world directly and immediately inside `execute`. The deferral
machinery those engines need does not arise here.

## Journal

The deferred crash-recovery journal is an *action journal*: because every mutation
is an action through one executor, the deterministic replay log is the action
stream into `execute`. This is an intent log, not a component diff, so it survives
rule changes and stays auditable. Speech changes no world state, so it is not in
this journal; an optional chat/experience log would be a separate log over the
Event stream.

## Where it lives

The verbs, the seed world, and name resolution are game content and live in the
reference game crate `musce_ref`, over the world queries and the public command
surface the engine exposes. `musce_action` is pure engine mechanism: the
executor, the `CommandTable` lookup and registration, `Ctx` and its emit API, and
the audience resolver. See [engine-and-game.md](engine-and-game.md).

The action layer is its own crate, `musce_action`, depending on `musce_core` and
`musce_proto` and free of `tokio`, so it stays pure synchronous logic and fast to
test. The
commands-in / events-out vocabulary (`Command`, `Event`, `Audience`, `EventKind`,
`ConnectionId`, ...) lives in a small `musce_proto` crate shared by `musce_action`,
`musce_net`, and `musce_host`, so the action layer never depends on the transport.
`musce_host` invokes the dispatcher and holds no command knowledge.

## MVP starting set

Engine mutators are already built; they stay `World` methods. The `Action` enum
is only as large as the verbs need. The first slice (built) is deliberately
minimal:

- `Action::Move { entity, into }` only, with `execute` and `ExecError`. (The
  action set has since grown to the full structural vocabulary, and the
  structural-fact channel is now live; see "The structural-fact channel" above.)
- Verbs `look`, `go <dir>` / bare direction, `take`, `drop`, `say`, and `help`
  (the game documents its own in-world surface), in a `CommandTable` looked up by
  exact name then first registered prefix (movement registered before `say`, so
  `s` is south and `sa` is say). The account floor's `@quit`/`@who`/`@help` stay,
  and `@help` lists only those account commands, not the game's verbs.
- `@play` records a connection's actor `EntityId` as a session attachment on the
  floor (session state), so bare commands have an actor; the audience resolver
  reads a conn->actor index derived from those attachments
  (`musce_action::Actors`). Durable embodiment (the `Controls` relation + `Focus`
  component, world state) is deferred and will back the attachment without
  touching handlers, which already take the actor explicitly.
- A code-seeded world (a hall, a garden, a cellar linked by exit entities; a
  takeable key; a player avatar), built with `World::spawn` when the DB loads empty,
  as ground truth for tests and play.

Output is addressed semantically and resolved sim-side: handlers emit first-person
feedback to the acting connection and third-person narration to the room (the
actor excluded), and the audience resolver expands `Room`/`Entity` into the
connections that should see it before anything reaches net. Net is left a pure
`Connection` pipe.

The full structural action set (`Create`/`Destroy`/`SetComponent`/
`RemoveComponent`), the type-erased reflection primitives it rides
(`World::create`/`set_component`/`component_value`, the guards, the registry
extensions), and the admin/builder `@`-verbs built over them
(`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`/`@destroy`/`@purge`/`@possess`/
`@unpossess`) live in [admin-verbs.md](admin-verbs.md).
