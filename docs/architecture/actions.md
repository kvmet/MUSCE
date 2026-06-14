# Actions and the Executor

> Status: **not implemented, pending review before implementation.** This records
> a proposed design and its rationale; nothing here is built yet.

## Action is the only thing that mutates the world

`Action` is the single vocabulary of world mutation. Several *sources* produce
actions; one *executor* applies them:

- a net `Command` (parsed input plus provenance) goes through a dispatch phase
  that validates and authorizes it into an `Action`, or rejects it
- a sequence step produces an `Action` (see `sequences.md`)
- an effect or other system produces an `Action`

```
execute(world, action, &mut events) -> Result<(), Failure>
```

This is the single place world-mutation rules live, so the take-an-item logic
exists once whether a player, a script, or another system triggers it. An action
handler also receives the event sink and emits perception events as a side effect
of the mutation (a move emits "X arrives from the north").

## Command vs Action

The thread boundary is unchanged from `concurrency.md`: the net thread speaks
`Command` in, `Event` out. `Action` is internal to the sim and never crosses the
channel.

- A **Command** is a request with provenance. It may be rejected.
- An **Action** is the authorized, validated mutation.

The parser's whole job is `Command -> Action` (or a rejection event). Actions from
a sequence still flow through the same `execute`, so a scripted NPC walking into a
now-locked door fails exactly as a player would.

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
  (`to: Entity(player), kind: Narration`). No actor, no action. The net thread
  routes room-addressed events to every connection puppeting an entity in that
  room, entity-addressed events to that entity's connection.
- `Say`/`Emote`/`look` are commands whose handlers emit Events and mutate nothing.
  Just as take/drop/give collapse to one `Move`, speech/emote/narrate collapse to
  one emit. The difference: `Move` is an action (it mutates) and emit is not (it
  reports).
- Gating (a silence effect blocks speech) is a rule check in the Say command
  handler, where a take's reachability check also lives, not a property of an
  action.
- An NPC overhearing speech is the perception layer reading Events off the bus
  (deferred sense-propagation), not a dependency on speech being an action.

So mutation funnels through `execute`; output flows out as Events; the event sink
is handed to both action handlers and pure-output command handlers.

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
  deserialize-and-`insert_one` into an existing entity. Merge also needs a per-tag
  serialize-one-component-to-`Value`. Both are small extensions of the existing
  `ser_one`/`deser_one` pattern.
- `SetComponent` must **refuse relation forward-links and derived indexes**
  (`RelTarget`, `RelSources`). Writing them directly bypasses the cycle check and
  corrupts the reverse index. Those tags are registered via `register_relation`,
  so the registry can recognize and reject them, directing the change to
  `Move`/`Relate` instead. The generic setter is for plain-data components only.

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

## MVP starting set

Engine mutators are already built; leave them as `World` methods. Do not pre-build
the `Action` enum; grow it from:

- `Move { entity, into }` (take, drop, give, put resolve to it)
- `Create`, `Destroy`
- `Say` / `Emote` commands that emit Events (not actions)
- admin, once a builder is connected: `@create`, `@destroy`, `@dig`,
  `@tel`/`@goto`/`@summon`, `@set` (`SetComponent`)

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
