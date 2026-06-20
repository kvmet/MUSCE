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
> executor, the action vocabulary, command dispatch, and atomicity; the
> type-erased reflection primitives and the admin/builder `@`-verbs that ride them
> (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`, all built; `@destroy` and
> `@possess` deferred) are in [admin-verbs.md](admin-verbs.md).

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
structural violation. It runs no gameplay rules and emits no events; the action
set is just the typed reflection of the `World` mutators. There is no
structural-fact channel yet: rather than thread a dead sink through every call
site, it lands with the first reaction system that reads it, and as a typed
mutation fact (`Moved`/`Created`/...), not a perception `Event` (see "Reactions
respond" below).

Gameplay rules and perception prose live one layer up, in the verb handlers. "The
take logic exists once" is achieved by shared rule/perception helpers (e.g. a
`do_move` used by both the player `go` command and, later, AI and sequences), not
by pushing rules into `execute`. Each engine primitive stays atomic and free of
intent: `execute` owns the world, handlers own meaning.

## Command vs Action

The thread boundary is unchanged from `concurrency.md`: the net thread speaks
`Command` in, `Event` out. `Action` is internal to the sim and never crosses the
channel.

- A **Command** is a request with provenance. It may be rejected.
- An **Action** is the authorized, validated mutation.

The parser's whole job is `Command -> Action` (or a `Rejection`, rendered as a
`Feedback` event). Two distinct error channels: a handler's pre-commit rule check
produces a player-facing `Rejection`; `execute` produces a structural `ExecError`,
which a correct handler has already ruled out, so it signals a bug rather than
ordinary play.

Scripted behavior reaches the same rules by going through the same verb helpers a
player command does, not by emitting raw actions. A sequence step references a
**verb/intent**, not a bare `Action`, so a scripted NPC walking into a now-locked
door fails exactly as a player would; a raw `Action` would skip the gameplay rule
and is reserved for the rule-bypassing admin path. (See `sequences.md`.)

## Dispatch: a command table the runtime invokes

The parser is a **registry**, not one growing `match`. Verbs register into a
command table keyed by name, looked up by longest matching prefix so
abbreviations fall out for free (`n` → `north`, `inv` → `inventory`). Each entry
is a small parse function plus its permission gate; verbs group by module
(movement, combat, communication, building) and register themselves, so adding a
verb is a local change, not an edit to a central switch. Lookup is O(verb length)
and stays flat from fifty verbs to thousands.

Two things keep a large command surface cheap:

- **N verbs are not N mutation paths.** Most verbs are thin parse functions over
  the tiny action set (the sugar table below): `take`/`drop`/`give`/`put`/`@tel`/
  `@goto`/`@summon` are all one `Move` with a different computed destination and
  rule. What grows with the game is parse rules, not the executor, which stays
  small and central.
- **Dispatch is a library layer the runtime invokes, not part of it.** The sim
  thread (`musce_host`) drains the inbox and calls one dispatch entry point with
  the world and an event sink; the command table, parse rules, and `execute` live
  in the action layer. The runtime holds no command knowledge.

Which table a command hits is the active input-stack frame (see
[networking-and-sessions.md](networking-and-sessions.md)): the `@`-namespace
routes to the account/admin table, bare commands to the active in-game frame.

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

## Output is the Event channel, not an action

Communication mutates nothing, so it is not in the action vocabulary. The
primitive is the **Event**: the output side of the commands-in / events-out
boundary, addressed and typed.

```
Event { to: Audience, kind: EventKind, .. }
Audience  = Room(id) | Entity(id) | Connection(id)
EventKind = Speech | Emote | Narration | System | Feedback | ...
```

- Showing text to a player is just emitting an Event addressed to them
  (`to: Entity(player), kind: Narration`). No actor, no action. **Audience
  resolution is sim-side:** turning `Room`/`Entity` into the connections that
  should see it needs world state (who is in the room) and the
  connection-to-entity map, so the sim expands those audiences into
  `Connection`-addressed events before output reaches net. Net is a pure
  `Connection` pipe and never resolves audiences. (See
  [networking-and-sessions.md](networking-and-sessions.md).)
- `Say`/`Emote`/`look` are commands whose handlers emit Events and mutate nothing.
  Just as take/drop/give collapse to one `Move`, speech/emote/narrate collapse to
  one emit. The difference: `Move` is an action (it mutates) and emit is not (it
  reports).
- Gating (a silence effect blocks speech) is a rule check in the Say command
  handler, where a take's reachability check also lives, not a property of an
  action.
- An NPC overhearing speech is the perception layer reading Events off the bus
  (deferred sense-propagation), not a dependency on speech being an action.

So mutation funnels through `execute` (which emits nothing); output flows out as
Events from the verb and command handlers; the sim resolves their audiences to
connections on the way out.

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

- `Action::Move { entity, into }` only, with `execute` and `ExecError`. The
  reaction/structural-fact channel is deferred: rather than thread a dead sink
  through every call site, it lands with the first system that consumes it, typed
  as a mutation fact rather than a perception `Event`.
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
(`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`; `@destroy` deferred) live in
[admin-verbs.md](admin-verbs.md).
