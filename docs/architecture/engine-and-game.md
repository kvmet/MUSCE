# Engine and Game

> Status: **built.** The runtime (`musce_host`) is a library parameterized by an
> injected `Game`; the engine crates carry no game content; the reference game
> `musce_ref` owns the verbs, the seed world, name resolution, the `@play` actor
> policy, `main`, and the end-to-end test. This records the boundary between the
> engine substrate and a game built on it, the `Game` the runtime is
> parameterized over, and the role of `musce_ref`.

## The substrate is not a game

MUSCE is an engine, not a game. The crates built so far (`musce_core`,
`musce_proto`, `musce_action`, `musce_net`, `musce_host`, `musce_persistence`) are
substrate: they own world state, the mutation path, the transport, and the
runtime, and they stay free of any particular game's content. A game supplies the
content: its verbs and how they parse, what exists at boot, how things read in
prose, and what the rules are.

The first action slice put a handful of verbs, a seed world, and name resolution
inside `musce_action` to prove the plumbing end to end. That was scaffolding: game
content living in an engine crate. It now lives in `musce_ref`.

## musce_ref: the reference game

`musce_ref` is the minimal reference game that ships in this repo. It exists for
three reasons:

- **The end-to-end fixture.** Integration tests drive a real game through the real
  engine; that game is `musce_ref`. The engine crates stay content-free while
  still being exercised whole.
- **The worked example.** It is the canonical demonstration of standing a game up
  on the engine: build a command table, seed a world, choose an actor, call the
  runtime.
- **The fork point.** A real game forks `musce_ref` and replaces its content. Real
  games do not live in this repo.

It is deliberately small and opinionated: English-first parsing, plain prose, a
few rooms. Where it has to choose a convention it picks one rather than
generalizing; if you need a different choice, you fork this piece, not the engine.

## Topology: the runtime is a library, the game is the binary

`musce_host` is a runtime *library*. Its `run` owns the sim thread, the tick loop,
boot load, and persistence, and it holds no game knowledge. It takes the game as
an injected value:

```
musce_host::run(store, config, shutdown, game) -> RunReport
```

`musce_ref` is the binary. Its `main` builds the reference `Game` and calls `run`.
An external game does the same from its own repo: depend on the engine libraries,
build its own `Game`, call `run`. The runtime is reused; only the content differs.
The single in-repo consequence is that `main` moves from `musce_host` into
`musce_ref`.

The dependency arrows stay acyclic and the runtime never depends on the game:

```
musce_ref -> musce_host -> musce_action -> musce_proto -> musce_core
```

## The Game injection

`Game` is the whole of what the runtime needs from a game, and it is small:

- **`commands: CommandTable`** the in-game verb registry the embodiment frame
  dispatches against.
- **`seed: fn(&mut World)`** builds the starting world when the database loads
  empty; a loaded world is left untouched.
- **`bind_actor`** the `@play` policy: which actor a connection comes to drive. The
  stub finds the seeded avatar; the persisted `Controls`/`Focus` embodiment (see
  [networking-and-sessions.md](networking-and-sessions.md)) replaces the body of
  this hook later without changing the interface.

A plain struct of values plus fn pointers, matching the style the command and
component registries already use. A `trait Game` is the alternative if a game ever
needs to carry its own state into these hooks; nothing needs that yet, so we do
not add it.

The account floor (`@quit`/`@who`/`@help`) stays in the runtime: it is session
management, not game content. Only `@play`'s choice of actor is game policy, which
is why it is the one floor concern the game injects.

## The engine's game-facing API

For a game to live in its own crate the engine must expose the surface a game
programs against. This is the real design work the split forces; the rest is
moving files.

- **`CommandTable` registration.** A public way to register a verb: a name, a
  permission `Gate`, a handler. The lookup (exact name, then first registered
  prefix) and the gate check stay engine mechanism; the verbs and their parsing
  are the game's.
- **`Ctx` and a public emit API.** The handler context (`&mut World`, the actor,
  the connection) plus a small public emit surface: a first-person line to the
  actor and a third-person line to the room with the actor excluded. Handlers are
  `fn(&mut Ctx, &str)`. The exact method names are an open detail; the shape is
  fixed.
- **`execute` / `Action` / `ExecError`.** Already public: the structural mutation
  path a game's rule-checked handlers commit through.
- **The audience resolver, `Outbound`, and `Actors`.** Engine mechanism the game
  does not touch directly. `dispatch_bare` already takes the command table as a
  parameter, so it drives the game's table unchanged.

Name resolution leaves the engine entirely. Matching a typed noun against
descriptions is opinionated, English-leaning policy, so it lives in `musce_ref`
over the world queries the engine already exposes (`contents`, `container_of`,
`enclosing_room`, component access). The engine owns no naming.

## What moves where

| Concern | Lands in |
|---------|----------|
| `Action`, `execute`, `ExecError` | `musce_action` (engine) |
| `CommandTable` lookup + `register`, `Gate` | `musce_action` (engine) |
| `Ctx` + public emit API, the handler type | `musce_action` (engine) |
| audience resolver, `Outbound`, `Actors` | `musce_action` (engine) |
| the runtime, `run`, the `Game` type, the floor | `musce_host` (engine) |
| verbs (`look`/`go`/`take`/`drop`/`say`) + parsing | `musce_ref` (game) |
| name resolution | `musce_ref` (game) |
| the seed world | `musce_ref` (game) |
| narration prose, the takeable rule | `musce_ref` (game) |
| `@play` actor-choice policy | `musce_ref` (game) |
| `main` and the end-to-end test | `musce_ref` (game) |

## Build order

1. Make the engine surface public: `CommandTable::register` and the `Ctx` emit
   API, so a verb can be defined outside `musce_action`.
2. Add the `Game` type to `musce_host` and parameterize `run` over it: seed via the
   injected `seed`, and route `@play` through the injected actor policy.
3. Create `musce_ref`: move the verbs, name resolution, seed, narration, and the
   `@play` policy into it; give it `main` and the end-to-end test.
4. `musce_action` and `musce_host` now carry zero game content. Update the docs
   that described those verbs as living there to point here.

The crate and binary-target wiring is settled: `musce_ref` is a workspace member
with both a library and a binary (its `main`); `musce_host` is library-only (its
`main` moved to `musce_ref`), and the end-to-end test lives in `musce_ref` too.
