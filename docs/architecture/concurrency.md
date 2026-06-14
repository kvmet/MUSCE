# Concurrency and the Tick

> Status: the threading model and tick shape are decided; the systems and tick
> pipeline are not yet built. This records the intended design and its rationale.

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
    drain command inbox       // the only entry point for external mutation
    run phases in fixed order // each phase may have its own cadence
    collect emitted events    // push to the outbox
    every N ticks: snapshot   // hand to the persistence thread
}
```

Fixed order means deterministic ticks: reproducible bugs and sane resolution
order. Phases support **multi-rate** cadences so subsystems run at their natural
frequency rather than all every tick.

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
