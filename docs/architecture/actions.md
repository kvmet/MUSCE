# Actions and the Executor

> Status: **structural vocabulary built.** The full structural executor
> (`Action::Move`/`Create`/`Destroy`/`SetComponent`/`RemoveComponent` +
> `execute` + `ExecError`), the verb dispatch table, the verbs `look`, `go`/bare
> direction, `take`, `drop`, and `say`, the stub `@play` actor binding, the
> sim-side audience resolver, and the code-seeded starter world are implemented
> in `musce_proto` (shared vocabulary) and `musce_action`, and wired into
> `musce_host`. The admin verbs that ride the new primitives remain proposed; the
> rest of this document records that design.

## Action is the only thing that mutates the world

`Action` is the single vocabulary of world mutation. Several *sources* produce
actions; one *executor* applies them:

- a net `Command` (parsed input plus provenance) goes through a dispatch phase
  that validates and authorizes it into an `Action`, or rejects it
- a sequence step produces an `Action` (see `sequences.md`)
- an effect or other system produces an `Action`

```
execute(world, action, &mut sink) -> Result<(), ExecError>
```

`execute` is **structural only**: it applies the typed mutator and enforces only
the invariants that hold for every source (the entity exists, the relation stays
acyclic), returning `ExecError` on a structural violation. It runs no gameplay
rules and emits no events; the action set is just the typed reflection of the
`World` mutators.

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
| `@goto <t>` | `Move` | into `container_of(t)` | admin |
| `@summon <t>` | `Move` | into `container_of(me)` | admin |
| `@create <kind>` | `Create` | spawn, then `Move` into my room | admin |
| `@destroy <t>` | `Destroy` | `despawn(t)` | admin |
| `@dig <dir> [name]` | `Create` + link | spawn `Room`, add `Exits` both ways | admin |

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

## SetComponent granularity

Components are freely mutable. The whole-component behavior is a property of the
generic admin path, not the data.

- **Typed code mutates fields in place**: `world.get::<&mut Stats>(e)?.str += 1`.
  Fully granular, the normal gameplay path.
- **`SetComponent` is type-erased**, so it works at whole-component granularity:
  it receives a tag plus a JSON value, with no compile-time knowledge of fields,
  and deserializes-and-overwrites the whole component via the `ComponentRegistry`
  (the same registry that drives persistence is the reflection layer). A JSON
  merge-patch (`@set e stats {"str": 12}`) gives field-level editing as a
  read-modify-write: serialize the current component, patch the key, deserialize,
  overwrite. Reaching one field generically without this would need a
  reflection/path system, which the JSON layer makes unnecessary.

Implementation implications, grounded in `component.rs`:

- The registry today does serialize-entity and deserialize-into-`EntityBuilder`
  (spawn/load). A live `SetComponent` needs a third per-tag function:
  deserialize-and-`insert_one` into an existing entity. Merge needs a per-tag
  serialize-one-component-to-`Value` (exposed as `World::component_value`), so the
  command layer reads the current component, patches the key, and overwrites; the
  engine owns neither the merge nor the verb. Both are small extensions of the
  existing `ser_one`/`deser_one` pattern.
- `SetComponent` must **refuse relation forward-links** and the **identity tag**.
  Writing a forward-link directly bypasses the cycle check and the reverse-index
  bookkeeping; `Id` must stay in lockstep with the `EntityIndex`. Relation tags are
  registered via `register_relation`, so the registry recognizes and rejects them,
  directing the change to `Move`/`Relate`; the generic setter is for plain-data
  components only. Load is exempt from the relation guard because
  `rebuild_relations` reconstructs the reverse index after it, whereas a live
  mutation has no rebuild pass.

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

The verbs, the seed world, and name resolution currently live in `musce_action`
as the first-slice scaffolding that proved the plumbing; they are game content and
move to the reference game crate `musce_ref`, leaving `musce_action` pure engine
mechanism (the executor, the `CommandTable` lookup and registration, `Ctx` and its
emit API, the audience resolver). See
[engine-and-game.md](engine-and-game.md).

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

- `Action::Move { entity, into }` only, with `execute` and `ExecError`. `execute`
  carries a structural-event sink for reactions; `Move` emits nothing into it yet.
- Verbs `look`, `go <dir>` / bare direction, `take`, `drop`, and `say` (Room-
  addressed, no mutation), in a `CommandTable` looked up by exact name then first
  registered prefix (movement registered before `say`, so `s` is south and `sa`
  is say). The account floor's `@quit`/`@who`/`@help` stay.
- A stub `@play` binding the connection to an actor `EntityId` (session state, in
  `musce_action::Actors`), standing in for embodiment so bare commands have an
  actor. The full `Controls` relation + `Focus` component + `@play` flow is the
  next slice and replaces the pointer without touching handlers, which already
  take the actor explicitly.
- A code-seeded world (a hall, a garden, a cellar linked by `Exits`; a takeable
  key; a player avatar), built with `World::spawn` when the DB loads empty, as
  ground truth for tests and play.

Output is addressed semantically and resolved sim-side: handlers emit first-person
feedback to the acting connection and third-person narration to the room (the
actor excluded), and the audience resolver expands `Room`/`Entity` into the
connections that should see it before anything reaches net. Net is left a pure
`Connection` pipe.

The structural action vocabulary (`Create`/`Destroy`/`SetComponent`/
`RemoveComponent`) and the one reflection operation it needs are built, recorded
under "Completing the structural vocabulary" below. The admin verbs
(`@create`/`@destroy`/`@dig`/`@tel`/`@goto`/`@summon`/`@set`) that ride those
primitives are game/dispatch surface and come next.

## Completing the structural vocabulary

> Status: **built.** Engine-only: the structural action set and the reflection
> operation it needs exist in `musce_core` and `musce_action`. The admin verbs
> that ride these primitives remain deferred.

`Action` grows from `Move` to the full bucket-1 set, the typed reflection of the
`World` mutators: `Create { components }`, `Destroy { entity }`,
`SetComponent { entity, tag, value }`, `RemoveComponent { entity, tag }`. The
payloads are type-erased JSON; `musce_core` re-exports `serde_json`'s `Value`/`Map`
so the action layer names them without a `serde_json` dependency of its own.

Each action is a thin `execute` arm over a `World` method, the way `Move` wraps
`move_entity`: the mutation needs the private registry and ecs, so it lives in
`musce_core`.

- `World::create(&Value) -> EntityId` builds an entity from a tag->value blob and
  `spawn`s it. It is **location-less**: it makes a root entity and never places it.
  Placement is a separate `Move` the command layer composes only when it makes
  sense; an entity may legitimately stay location-less, or its container may be
  unknown at creation. Prescribing the move in the primitive would be wrong.
- `World::set_component` / `remove_component` deserialize-and-`insert_one` / remove
  one component on a live entity.
- `World::component_value(id, tag) -> Option<Value>` reads one component back as
  JSON. This is the read half of merge-patch (see the SetComponent section); the
  engine implements neither the merge nor the verb.

`execute` returns the action's **subject** `EntityId` (`Result<EntityId,
ExecError>`, widening `Move`'s shipped `Result<()>`). `Create` allocates its id
inside `spawn`, so returning it is the only way the caller learns the new id;
returning the subject uniformly keeps the other arms consistent.

Guards, enforced structurally (an `ExecError`, never player-facing):

- **Relation tags are refused** on the live paths (`create`/`set`/`remove`):
  writing a forward-link raw skips the cycle check and the reverse-index
  bookkeeping, so the change must go through `Move`/`Relate`. Load is exempt
  because `rebuild_relations` runs after it; a live mutation has no rebuild pass.
- **The identity tag is refused** on `set`/`remove`: `Id` must track the
  `EntityIndex`.
- Otherwise the usual structural checks: the entity exists, the tag is registered,
  the value deserializes.

`ComponentRegistry` gains, per registered tag, three small extensions of the
existing `ser_one`/`deser_one` pattern (deserialize-and-`insert_one` into a live
entity, remove-by-tag, serialize-one-to-`Value`) plus an `is_relation_tag`
predicate for the guard above.

Open questions:

- **`@destroy` vs `@purge`.** `despawn` reparents contents up (Reparent cascade in
  `containment.rs`), so `@destroy bag` spills its contents to the floor. Builders
  often expect destroy to take the contents with it. Decide whether `@destroy` is
  despawn-with-reparent plus a separate recursive `@purge`, or recursive by
  default.
- **`@dig` opposite-direction convention** (n/s, e/w, u/d) for the reverse exit, a
  content table, overridable per dig.

Prior art: Bevy/flecs command buffers (the mutator set at the engine layer);
MOO/Diku `@`-commands (the admin-verb bucket). Mirror the Diku surface builders
know, but resolve it to this action set over composable components plus the JSON
registry, not Diku's fixed struct fields.
