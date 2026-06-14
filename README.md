# MUSCE

An ECS-based MUD engine in Rust, aimed at a deep, emergent, room-based
simulation. Early and under active design; the in-memory ECS world, its
persistence layer, a TCP transport, and the first slice of the in-game command
layer exist. You can connect, `@play`, and `look`/`go`/`take`/`drop`/`say` in a
small seeded world.

## Workspace

- `musce_core` — the engine: the ECS world, global identity, the generic
  relation layer, containment, and the JSON snapshot model. Pure (no I/O).
- `musce_persistence` — World-as-truth save/load behind a `Persistence` trait;
  SQLite backend today, Postgres to follow.
- `musce_proto` — the shared command/event vocabulary that crosses the net/sim
  boundary. Transport-free, so the action layer never depends on networking.
- `musce_action` — the action layer: the structural executor (`Action::Move`),
  the verb dispatch table, the stub `@play` actor binding, the audience resolver,
  and a code-seeded starter world. Pure synchronous logic.
- `musce_net` — raw TCP line-mode transport behind a transport-agnostic
  `Connection`, plus the commands-in/events-out pipe and event router.
- `musce_host` — the runtime: the single sim thread, the tick loop, boot load and
  snapshot persistence, and the command dispatcher that wires it together.

## Architecture

Design decisions and their rationale live in
[docs/architecture/](docs/architecture/README.md). Start there for the
big picture: World-as-truth, a single authoritative sim thread, the relation
layer, the persistence model, and the seams kept for future sharding.

## Build

```sh
cargo build
cargo test
```

## License

See [LICENSE](LICENSE).
