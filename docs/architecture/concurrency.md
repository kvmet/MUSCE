# Concurrency and the Tick

> Status: **built.** The single sim thread, the tick loop, and the system pipeline
> run; `musce_host` carries the game's `Game.systems` on the pipeline every tick
> (the reference game's wandering creature is the first), so tick scheduling and
> the dual clocks are exercised, not just declared. Multi-rate cadences beyond a
> system's own `tick % N` gating are not yet a pipeline feature. This records the
> threading model, the tick shape, and their rationale.

## One authoritative sim thread

A single thread owns the `World` and runs the tick loop. It is the only thing
that mutates the world. Everything else runs on other threads:

- **Networking** (tokio): accepts connections, one task per connection, parses
  input into commands, routes events back to the right connection.
- **Persistence**: receives snapshots, writes them to the DB off the hot path.

The boundary is a typed message channel: **commands in, events out**, addressed
by `EntityId`. This gives determinism and freedom from data races by
construction, and it is the same boundary that keeps sharding reachable.

## The tick

A fixed-order pipeline of phases:

```
loop {
    drain command inbox        // the only entry point for external mutation
    run systems in fixed order // Game.systems, each scheduling by tick/now
    collect emitted events     // push to the outbox
    every N ticks: snapshot    // hand to the persistence thread
}
```

Fixed order means deterministic ticks: reproducible bugs and sane resolution
order. The game injects its systems as `Game.systems` (a `fn(&mut SystemCtx)`
list); the runtime runs them in registration order each tick. A `SystemCtx`
mirrors a verb's `Ctx` for the simulation half: it carries the world the system
mutates (through the same `execute`) and an emit buffer addressed to rooms, which
the runtime resolves to connections through the same audience resolver verbs use,
so a system's narration reaches players exactly as a verb's does. It has no actor
or connection, because a system acts for the world, not a player.

`SystemCtx` carries both clocks: `tick` (deterministic sim time, the default for
game logic) and `now` (wall-clock). A system schedules its own cadence off these,
e.g. `tick % N == 0` (the wanderer steps every `WANDER_EVERY` ticks) rather than
running its full body each tick. A pipeline-level multi-rate scheduler is not
built; per-system gating covers the need for now.

## Why no auto-scheduler

A bevy-style scheduler that analyzes system component access and runs
non-conflicting systems in parallel is deliberately *not* used.

- Auto-parallelism pays off when a tick is CPU-bound across many systems. A
  room-based MUD's tick is light; the scheduler would optimize work that is not
  slow while costing determinism.
- The real bottlenecks are I/O concurrency and persistence latency, which are
  already parallelized by running networking and persistence on their own
  threads.
- hecs's world is `Sync` with atomic borrow tracking, so when a single system
  ever profiles hot, it can be parallelized with rayon on that one loop. That is
  targeted data-parallelism, available any time, with no scheduler tax on every
  system.
- The real scaling lever for throughput is **sharding** (multiple worlds across
  threads/processes, split by zone), which is coarse-grained parallelism along
  the spatial structure, not splitting one tick across cores.

So: ordered, deterministic systems now; rayon on a hot loop if one appears;
sharding for real scale. (If the design ever shifts toward a heavy per-tick
field simulation in a single zone, that calculus changes; the room-based model is
specifically what keeps it from arising.)
