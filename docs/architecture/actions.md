# Actions and the Executor

> Status: **not implemented, pending review before implementation.** This records
> a proposed design and its rationale; nothing here is built yet.

## Action is the only thing that mutates the world

`Action` is the single verb vocabulary. Several *sources* produce actions; one
*executor* applies them:

- a net `Command` (parsed input plus provenance) goes through a dispatch phase
  that validates and authorizes it into an `Action`, or rejects it
- a sequence step produces an `Action` (see `sequences.md`)
- an effect or other system produces an `Action`

```
execute(world, action) -> Result<Vec<Event>, Failure>
```

This is the single place world-mutation rules live. The take-an-item logic exists
once, whether a player, a script, or another system triggers it.

## Command vs Action

The thread boundary is unchanged from `concurrency.md`: the net thread speaks
`Command` in, `Event` out. `Action` is internal to the sim and never crosses the
channel.

- A **Command** is a request with provenance. It may be rejected.
- An **Action** is the authorized, validated mutation.

The parser's whole job is `Command -> Action` (or a rejection event). Actions from
a sequence still flow through the same `execute`, so a scripted NPC walking into a
now-locked door fails exactly as a player would.

Reads are not actions. A `look` arrives as a `Command`, runs a query, and produces
an `Event`. It does not go through `execute` and is not journaled.

## Two layers: mutators below, actions above

The closed, minimal CRUD set already exists as the `World` API and is the
substrate actions compile down to:

- `spawn` / `despawn`
- `insert_component` / `remove_component` / `set_component`
- `relate` / `unrelate` (with `move_entity` as sugar over `relate`)

`Action` sits above these and is deliberately *open* and semantic, because actions
encode game rules and there is no closed minimal set of game rules. Two families:

- **gameplay actions**: specific verbs, rule-checked (`Move`, `Take`, `Drop`, `Say`).
- **admin/builder actions**: generic, CRUD-shaped, permission-gated, and
  rule-bypassing by design (`Create`, `Destroy`, `SetComponent`, `Link`). A
  builder spawning a sword should not run through "can you reach and take this."

Both are `Action`: both flow through `execute`, both are journaled, both are
authorized by provenance. The split is rule-checked vs permission-gated, not two
mechanisms.

## No command buffer needed

ECS command buffers (Bevy `Commands`, flecs deferred mode) are exactly this CRUD
set, buffered and flushed at a sync point because structural changes are illegal
during parallel system iteration. With no auto-scheduler (see `concurrency.md`),
the sim thread runs ordered systems with exclusive `&mut World`, so an action
mutates the world directly and immediately inside `execute`. The deferral
machinery those engines need does not arise here.

## Journal

Because every mutation is an action through one executor, the deferred crash-
recovery journal is an *action journal*: the deterministic replay log is the
action stream into `execute`. This is an intent log, not a component diff, so it
survives rule changes and stays auditable.

## MVP starting set

Engine mutators are already built; leave them as `World` methods. Do not pre-build
the `Action` enum; grow it from:

- `Move { actor, dir }`
- `Take { actor, item }`, `Drop { actor, item }`
- `Say { actor, text }` / `Emote`
- admin, once a builder is connected: `Create`, `Destroy`, `SetComponent`

Prior art: Bevy/flecs command buffers (CRUD at the engine layer); MOO/Diku
`@`-commands (the admin-action family).
