# MUSCE

An ECS-based MUD engine in Rust, aimed at a deep, emergent, room-based
simulation. Early and under active design; the in-memory ECS world and its
persistence layer exist, networking and game systems do not yet.

## Workspace

- `musce_core` — the engine: the ECS world, global identity, the generic
  relation layer, containment, and the JSON snapshot model. Pure (no I/O).
- `musce_persistence` — World-as-truth save/load behind a `Persistence` trait;
  SQLite backend today, Postgres to follow.

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
