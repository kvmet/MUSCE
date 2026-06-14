# MUSCE

ECS-based MUD engine in Rust. Architecture and design decisions live in
`docs/architecture/` ([index](docs/architecture/README.md)) and are the source of
truth for *why* the engine is shaped the way it is.

## Keep the architecture docs in sync

The docs exist to survive long gaps between work, so they are only worth anything
if they stay accurate. Treat them as part of the code, not separate from it:

- When you change a subsystem's behavior or design, update its doc in the **same
  change**, not afterward.
- When you implement something currently marked proposed or deferred, flip its
  `> Status:` blockquote and the README's Built/Deferred lists to match.
- When a decision is reversed, edit the doc to state the decision that won. Record
  enduring rationale, not the history of how it got there; version control already
  holds the history.
- New subsystem with no doc? Add one under `docs/architecture/` and link it from
  the README index.

Touch the doc when you touch the code:

| Area | Doc |
|------|------|
| world, identity, relations, containment | `ecs-and-relations.md` |
| snapshot and `musce_persistence` | `persistence.md` |
| sim thread, tick loop, scheduling | `concurrency.md` |
| actions, command dispatch, the executor | `actions.md` |
| sequences, effects, timers | `sequences.md` |
| zones, locator, entity handoff | `sharding.md` |

## Status markers

A doc describing unbuilt design carries a `> Status:` blockquote directly under
its title (e.g. `> Status: not implemented, pending review before
implementation.`). Keep it honest: it is how a reader tells proposed design from
shipped reality.

## Formatting

The codebase is `cargo fmt` clean. Run `cargo fmt` before committing, and format
only the files your change touches (not a workspace-wide sweep that churns
unrelated code). The enforcing gate is `cargo fmt --check` in CI once CI exists;
until then this convention is the gate.
