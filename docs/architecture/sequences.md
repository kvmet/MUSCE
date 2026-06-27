# Sequences and Effects

> Status: **built (in the reference game).** The sequence layer ships as game
> content in `musce_ref` (`sequences.rs`): the `Steps` program component, the
> `Sequences(Vec<Instance>)` instance component, the `Intent` set, and the
> `sequence_sweep` system on the tick pipeline, with two seeded demonstrators (a
> patrolling sentry and a burning torch). The engine provides only the generic
> persisted-component plumbing; nothing here is engine-specific. It sits on two
> pieces of substrate: the tick-loop **system pipeline** (see
> [concurrency.md](concurrency.md)) and the **structural-fact channel** the torch
> converges with (`Fact::Destroyed`; see [actions.md](actions.md)).

## Timed behavior is a component, not a scheduler

Deferred and timed behavior (a fire burning out, a scripted patrol) is modeled as
a component on the entity, swept by a system each tick. It is never a side
scheduler heap of `(tick, closure)`.

- it lives in the `World`, so it persists and reloads for free; a half-played
  routine resumes mid-loop after a restart
- it stays deterministic and replayable
- a tick-indexed priority queue is then only a *derived acceleration index* over
  these components, added later if an every-tick sweep profiles hot. This mirrors
  the relation layer: forward link is truth, the index is derived

Effects (burning, poison) and sequences share one skeleton: a component holding
"when is my next beat" plus a sweep system. A **sequence** is the general case (N
beats with delays between them, optionally repeating); an **effect** is a
sequence with a shape, a finite program whose terminal beat removes or transforms
the thing. The torch is the canonical effect: a one-shot program whose last beat
destroys the carrier. There is no separate "repeat until tick N" mechanism; a
bounded effect (poison "damage x10") would later want a repeat-count, which is
deferred.

## Program vs instance

The step list is shared content, carried on its own entity and referenced by id.
Only the cursor lives on the acting entity.

```rust
// Content: a program entity carries this. Shared, referenced by Instance.program.
struct Steps(Vec<Step>);
struct Step {
    delay: u32,     // ticks to wait before THIS step fires
    intent: Intent, // a rule-checked intent, not a raw Action (see below)
}

// Instance: a component on the acting entity. Persisted; resumes mid-play.
struct Instance {
    program: EntityId, // the program entity whose Steps this plays
    cursor: usize,     // index of the next step to fire
    next_at: u64,      // absolute tick that step fires
    repeat: bool,      // off the end, replay from the top instead of ending
}
struct Sequences(Vec<Instance>); // the concurrent sequences on one entity
```

A program is a location-less entity carrying only `Steps`: data, not a thing in a
room. Programs persist for free (a registered component) and ids are stable across
a reload, so `Instance.program` still resolves after a restart. Keeping programs
in the world (rather than a runtime registry a bare-`fn` system could not close
over) is what lets the sweep stay a pure `System` while "World is truth" holds.

## Intents, not raw Actions

A step holds an **intent**, not a raw `Action`, so a scripted actor runs the same
gameplay rules a player command does. `execute` is rule-free (see actions.md), so
a step emitting a bare `Move` would skip the locked-door check; going through the
shared rule helper keeps a scripted mover subject to the same rules as a player.

The MVP intent set is exactly two:

- `Move { dir }` — resolves the named exit out of the carrier's room and traverses
  it through `do_move`, the same rule-checked path the `go` verb runs. A `Locked`
  exit (or any future door / skill-check veto `can_traverse` grows) stops a
  scripted mover exactly as it stops a player; a blocked or missing exit is a
  no-op beat and the sequence still advances.
- `Destroy` — despawns the carrier itself. Rule-free, the terminal beat of an
  effect. It emits `Fact::Destroyed`, and the `death_cry` reaction narrates the
  demise one tick later (see "Convergence with the fact channel").

Branches, conditionals, and world-state waits are deliberately absent: the moment
sequences gain them they become a per-entity scripting language, which is the
deferred builder-scripting layer, not MVP creep.

## The sweep

Each tick `sequence_sweep` queries the running instances and, for any with
`next_at <= tick`, fires `steps[cursor].intent`, advances the cursor, and sets
`next_at = tick + steps[cursor].delay`. Off the end it removes the instance, or
resets the cursor to 0 if `repeat`. The per-step `delay` is the configurable wait
between beats; a "wait" is just a delay, not a separate intent.

Two hazards shape the loop:

- **A beat can despawn its own carrier** (the torch's terminal `Destroy` removes
  the entity holding `Sequences`). The sweep therefore collects carriers first
  (the moves and destroys below mutate the world it would otherwise iterate), and
  after each beat checks whether the carrier still exists; if a beat removed it,
  the sweep abandons that carrier and never writes the advanced instance list back
  to a dead entity.
- **A 0-delay burst** resolves inline in one tick (an authored burst fires
  together). The one danger is `repeat` plus an all-zero-delay cycle, which would
  loop forever inside a single tick. The cursor advance is system-internal
  bookkeeping written directly to the component, not routed through
  `execute(SetComponent)`, so a high-frequency cursor tick never pollutes the
  (deferred) action journal; the beats themselves are the actions worth replaying.

## Multiple concurrent sequences

hecs is one-component-per-type, so concurrent sequences are a
`Sequences(Vec<Instance>)` component, not N components. The sweep iterates the vec,
fires due instances, and retains the unfinished ones. Each instance has its own
cursor / `next_at` / `repeat`, so one entity can run an effect and a patrol at
once (the case the moment effects exist). When two instances act on the same
entity in one tick, they resolve by execution order (later overrides earlier),
which is fine until real contention appears. Addressing a single running sequence
independently would mean promoting it to its own entity related to the actor; that
is heavier than the current need.

## Repeat, and why it is not a system

`repeat` is a plain cursor-loop, free, and covers patrols and idle emotes. It
rhymes with a system because both are cadenced behavior, but the distinguishing
axis is authorship:

- a **system** is engine/game code: compiled, global, query-driven, applies to a
  class of entities, fixed at boot. The sweep itself is a system.
- a **sequence** is content: data, per-entity, attached and detached at runtime.
  The sweep interprets it.

Sequences are how a builder gets system-like power without writing code. Rule of
thumb: uniform behavior across a kind of entity is a system; specific, authored,
varies-per-entity behavior is a sequence.

## Delay, tick resolution, and the attach guard

A step applies inline in the sweep, so 0-delay steps resolve in the same tick. The
infinite-loop hazard (a repeating all-zero-delay cycle) is made structurally
impossible at **attach**, not caught by a per-tick fuse: `attach` requires a
repeating sequence to have positive total cycle delay (the sum of step delays). A
one-shot zero-delay chain terminates and is fine. `attach` also fixes the first
`next_at` from boot (tick 0 + the first step's delay); a future runtime attach
verb reuses the same validator and passes the current tick instead.

For MVP, sequences are **seed-only**: the patroller and torch are wired in
`seed.rs`. A runtime verb to attach/detach a sequence is deferred.

## Convergence with the fact channel

The torch is the end-to-end exercise of the gate-2 reaction channel. Its terminal
`Destroy` beat emits `Fact::Destroyed`; because facts are drained once at the top
of the system loop, the `death_cry` reaction narrates "a guttering torch crumbles
to dust" on the **next** tick. The sequence layer ships zero narration for
destruction: the existing reaction handles it. A `Move` beat narrates its own
departure and arrival to the rooms it leaves and enters, the same third-person
prose `wander` emits.
