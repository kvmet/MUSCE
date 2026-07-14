# Benchmarks

The engine carries a small set of [criterion](https://docs.rs/criterion) benchmarks
so a performance decision is made against measured cost, not a guess about where the
time goes. They exist to answer one recurring question: *where should effort go
next?* A bench earns its place only if it can move that answer, so the set stays
small and each one targets a cost that could realistically drive a redesign.

## Layout: micro next to the code, macro at the assembly point

Benchmarks live with the layer they measure:

- **Microbenchmarks** sit in the crate that owns the hot path, so they measure it
  directly with no indirection: `musce_core/benches/world_ops.rs` for the in-memory
  ECS, `musce_persistence/benches/snapshot_roundtrip.rs` for the DB round-trip.
- **Macrobenchmarks** sit in `musce_ref`, the only crate that assembles the whole
  stack into a running game with a real command table and seed. A command-latency
  or tick-cost number is only meaningful end-to-end, so it belongs where the game
  is composed, not against a stubbed world.

This split is deliberate: a micro-bench wedged into `musce_ref` would measure the
hot path through three layers of game content and tell you nothing crisp; a macro
bench in a lower crate can't see the real verbs or systems.

| Bench | Crate | Question it answers |
|-------|-------|---------------------|
| `world_ops` | `musce_core` | How do containment lookups, the move/relate path, the cascade despawn, and in-memory snapshot serialize/deserialize (`world_load` rebuilds the reverse relation lists) scale with world size? |
| `snapshot_roundtrip` | `musce_persistence` | How does the DB save/load cost scale with entity count, and how much of it is SQL vs serialization? |
| `command_latency` | `musce_ref` | What does a full player command cost end-to-end, on the read path (`look`) and the write path (`go`)? |
| `tick_work` | `musce_ref` | What does one simulation tick cost when no one is typing (the system pipeline running over the seed)? |

## Running them

```
cargo bench                              # every bench in the workspace
cargo bench -p musce_core                # one crate
cargo bench -p musce_core --bench world_ops -- contents   # one bench, one filter
```

Criterion writes HTML reports and its own run-over-run comparison under
`target/criterion/`; a second run reports the delta against the previous one
automatically.

## Reading the numbers

- **Slope beats absolute time.** The size-swept benches (`contents`, `snapshot`,
  `save`, `load`, `despawn_reparent`) matter for how cost grows, not the raw µs on
  one machine. A cost that turns from linear to superlinear as the world grows is
  the signal; a single fast number is not. Absolute timings are machine-relative
  and not worth committing to a doc, which is why none are pasted here.
- **The scaling benches are the decision-drivers.** They exist to catch a cost
  that only bites at scale, which is exactly the cost a redesign is weighed
  against.

### What the set already shows

Two findings are durable enough to record, because they point at where the next
work is (re-check them, don't trust the prose blindly):

- **The world save is the cost that scales, and it was dominated by SQL
  round-trips, not serialization.** Building the in-memory `Snapshot` is roughly two
  orders of magnitude cheaper per entity than writing it through the SQLite backend.
  The backend wrote one row per component inside a transaction, so the row-at-a-time
  inserts, not the JSON work, were where the time went. Batched multi-row inserts
  were the cheaper first win, and they landed: `save` now flattens the snapshot and
  issues batched `INSERT`/`DELETE` statements (chunked under each backend's
  bind-variable limit) instead of a statement per row. Measured against a `pre-batch`
  baseline on the in-memory SQLite bench, that cut `save` by ~9-10x across the sweep
  (100/1k/10k entities: ~4.6->0.5 ms, ~52->5.1 ms, ~504->54 ms), leaving `load`
  unchanged (it was already three bulk SELECTs). The remaining save cost still scales
  linearly with world size, so the storage-layout redesign (the dirty-tracked /
  incremental snapshot and the EAV split discussed in
  [persistence.md](persistence.md)) is the next lever, now against a ~10x-lower base.
- **The in-memory engine loop is not currently a concern.** Command dispatch and a
  bare tick sit in the low microseconds against the seed; the ECS hot paths (`hecs`
  queries) are fast. Effort spent there now would be premature.

## Measuring a change's gain

The reason to keep baselines: when a performance change lands, its gain should be
*measured*, not asserted. Criterion supports named baselines for exactly this. Just
before the change merges, capture the pre-change numbers on the current code:

```
cargo bench -- --save-baseline pre-<change>
```

then after the change, compare against it:

```
cargo bench -- --baseline pre-<change>
```

The baseline must be captured on the immediately-preceding code, not weeks earlier,
or unrelated drift pollutes the delta. The first planned use is the relationship /
proximity index (deferred; see the README roadmap): when it lands, a
`pre-index` baseline taken right before the merge is what turns "indexing should
help" into a number.
