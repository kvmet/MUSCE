# Sequences and Effects

> Status: **not implemented, pending review before implementation.** This records
> a proposed design and its rationale; nothing here is built yet.

## Timed behavior is a component, not a scheduler

Deferred and timed behavior (a fire burning out, a corpse rotting, a scripted
routine) is modeled as a component on the entity, swept by a system each tick. It
is never a side scheduler heap of `(tick, closure)`.

- it lives in the `World`, so it persists and reloads for free; a half-played
  routine resumes after a restart
- it stays deterministic and replayable
- a tick-indexed priority queue is then only a *derived acceleration index* over
  these components, added later if an every-tick sweep profiles hot. This mirrors
  the relation layer: forward link is truth, the index is derived

Effects (burning, poison) and sequences share one skeleton: a component holding
"when is my next beat" plus a sweep system. An effect is the degenerate case (one
beat, repeating until an end tick); a sequence is the general case (N different
beats with delays between them).

## Program vs instance

The action list is shared content, referenced by id. Only the cursor lives on the
entity.

```
// Content: loaded at boot, shared, referenced by id.
struct Sequence { steps: Vec<Step> }
struct Step {
    delay: u32,  // ticks to wait before THIS step fires
    verb:  Verb, // a rule-checked intent, see actions.md
}

// Instance: a component on the entity. Persisted; resumes mid-play after a load.
struct Running {
    seq: SequenceId,
    cursor: usize,
    next_at: u64,   // tick this entity's next step fires
    repeat: bool,
}
```

Each tick the sweep system queries running instances, and for any with
`next_at <= tick` it runs `steps[cursor].verb`, advances the cursor, and sets
`next_at = tick + steps[cursor].delay`. Off the end it removes the instance, or
resets to 0 if `repeat`. "Configurable delay between steps" is the per-step
`delay`.

A step holds a **verb/intent**, not a raw `Action`, so it runs the same gameplay
rules a player command does. `execute` is rule-free (see actions.md), so a step
emitting a bare `Move` would skip the locked-door check; going through the verb
helper keeps a scripted actor subject to the same rules as a player.

## Multiple concurrent sequences

hecs is one-component-per-type, so concurrent sequences are a
`Sequences(Vec<Instance>)` component, not N components. The sweep iterates the vec,
fires due instances, and retains the unfinished ones. Each instance has its own
cursor / `next_at` / `repeat`. When two instances act on the same entity in one
tick, they resolve by action execution order (later overrides earlier), which is
fine until real contention appears. Addressing a single running sequence
independently would mean promoting it to its own entity related to the actor;
that is heavier than the current need.

## Repeat, and why it is not a system

`repeat` is a plain cursor-loop, free, and covers patrols and idle emotes. It
rhymes with a system because both are cadenced behavior, but the distinguishing
axis is authorship:

- a **system** is engine code: compiled, global, query-driven, applies to a class
  of entities, fixed at boot. The sequence sweeper itself is a system.
- a **sequence** is content: data, per-entity, authored by a builder, attached and
  detached at runtime. The sweeper interprets it.

Sequences are how a builder gets system-like power without writing engine code.
Hold the line at a dumb cursor-loop: the moment sequences gain branches,
conditionals, or world-state waits, they become a per-entity scripting language,
which is the deferred builder-scripting layer and a deliberate later effort, not
MVP creep.

Rule of thumb: uniform behavior across a kind of entity, core to the engine, is a
system. Specific, authored, runtime-attached, varies per entity, is a sequence.

## Delay and tick resolution

A step applies inline in the sweep, so 0-delay steps resolve in the same tick and
an authored burst fires together. The one hazard is `repeat` plus an
all-zero-delay cycle, which would loop forever inside a single tick. Guard it by
requiring a repeating sequence to have positive total cycle delay (sum of step
delays > 0), enforced when the sequence is attached, so the infinite loop is
structurally impossible rather than caught by a steps-per-tick fuse. One-shot
zero-delay chains terminate and are fine.
