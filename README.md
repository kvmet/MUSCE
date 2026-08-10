# MUSCE

An ECS-based MUD engine in Rust, aimed at a deep, emergent, room-based
simulation. Early and under active design. One authoritative simulation drives
two interfaces over the same command stream: a text MUD over TCP, and a thin
"pointing" web client over WebSocket. You can connect, `@play`, and
`look`/`go`/`take`/`drop`/`say` in a small seeded world.

## Quickstart

Run the reference app (the engine parameterized with `musce_ref`'s content). It
listens for the text MUD on `127.0.0.1:4000` and the web client on
`127.0.0.1:4001`, and persists to a SQLite file:

```sh
cargo run -p musce_ref
```

Text client: connect with any line-mode client, seat a guest, and look around.

```sh
nc localhost 4000
@play
look
```

Web client (the pointing UI): its dev server proxies to the WebSocket above.

```sh
cd webclient
npx vite            # dev server; open the printed URL
```

Append `?mock` to the URL to run the in-browser stand-in with no server. See
[webclient/README.md](webclient/README.md).

The backend is chosen by the `MUSCE_DB` URL scheme (`sqlite://…` default,
`postgres://…`); the app code is identical either way.

## Workspace

- `musce` — the facade: the one crate an app depends on, re-exporting the
  engine's app-facing surface (`run`, `Config`, `App`, the wire types) and
  nothing internal.
- `musce_core` — the engine: the ECS world, global identity, the generic
  relation layer, containment, and the JSON snapshot model. Pure (no I/O).
- `musce_index` — a generic secondary index over one component (`key ->
  entities`), so the world answers "which entities key to X" without a scan.
- `musce_persistence` — World-as-truth save/load; an entity shredded to one row
  per component. SQLite and Postgres behind one trait.
- `musce_proto` — the wire vocabulary crossing the net/sim boundary: commands in,
  events out, and the web envelope. Transport-free; TS bindings behind the `ts`
  feature.
- `musce_action` — the action layer: the structural executor, verb dispatch, the
  affordance vocabulary and guards. Pure synchronous engine mechanism.
- `musce_macros` — the procedural authoring front end that lowers app
  `affordance!` declarations into the canonical action representation.
- `musce_agency` — the optional planner: the GOAP `Planner`, `Arbiter`, and
  execution `Driver` an app consumes to give NPCs goals.
- `musce_auth` — account authentication and identity: the `Account` record,
  capabilities, and the verdict/gate.
- `musce_net` — the transports (TCP line-mode and WebSocket) behind a
  transport-agnostic pipe, plus the commands-in/events-out router.
- `musce_host` — the runtime: the single sim thread, the tick loop, boot load and
  snapshot persistence, and the command dispatcher.
- `musce_ref` — the reference app and the runnable binary: it owns all app
  content (verbs, seed world, narration, the `@play` policy) and builds the
  `App` the engine runs.

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

MUSCE is licensed under the [Mozilla Public License 2.0](LICENSE).
