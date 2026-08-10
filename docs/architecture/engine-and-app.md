# Engine and App

> Status: **built, including generic canonical pointing.** The
> runtime (`musce_host`) is a library parameterized by an
> injected `App`; the engine crates carry no app content; the reference app
> `musce_ref` owns the verbs, the seed world, name resolution, the `@play` actor
> policy, `main`, and the end-to-end test. This records the boundary between the
> engine substrate and an app built on it, the `App` the runtime is
> parameterized over, and the role of `musce_ref`.

## The substrate is not an app

MUSCE is an engine, not an app. The crates built so far (`musce_core`,
`musce_proto`, `musce_action`, `musce_net`, `musce_host`, `musce_persistence`) are
substrate: they own world state, the mutation path, the transport, and the
runtime, and they stay free of any particular app's content. An app supplies the
content: its verbs and how they parse, what exists at boot, how things read in
prose, and what the rules are.

The first action slice put a handful of verbs, a seed world, and name resolution
inside `musce_action` to prove the plumbing end to end. That was scaffolding: app
content living in an engine crate. It now lives in `musce_ref`.

## musce_ref: the reference app

`musce_ref` is the minimal reference app that ships in this repo. It exists for
three reasons:

- **The end-to-end fixture.** Integration tests drive a real app through the real
  engine; that app is `musce_ref`. The engine crates stay content-free while
  still being exercised whole.
- **The worked example.** It is the canonical demonstration of standing an app up
  on the engine: build a command table, seed a world, choose an actor, call the
  runtime.
- **The fork point.** A real app forks `musce_ref` and replaces its content. Real
  games do not live in this repo.

It is deliberately small and opinionated: English-first parsing, plain prose, a
few rooms. Where it has to choose a convention it picks one rather than
generalizing; if you need a different choice, you fork this piece, not the engine.

## Topology: the runtime is a library, the app is the binary

`musce_host` is a runtime *library*. Its `run` owns the sim thread, the tick loop,
boot load, and persistence, and it holds no app knowledge. It takes the app as
an injected value:

```
musce_host::run(store, config, shutdown, app) -> RunReport
```

`musce_ref` is the binary. Its `main` builds the reference `App` and calls `run`.
An external app does the same from its own repo: depend on the `musce` facade,
build its own `App`, call `run`. The runtime is reused; only the content differs.
The single in-repo consequence is that `main` moves from `musce_host` into
`musce_ref`.

The dependency arrows stay acyclic and the runtime never depends on the app. An
app binds only to the facade, which re-exports the engine layers below it (see
[The `musce` facade](#the-musce-facade)):

```
musce_ref -> musce -> musce_host -> musce_action -> musce_core
                                                 -> musce_proto   (a dependency-free leaf)
```

`musce_proto` is the wire vocabulary and references no world identity, so it sits
below the action layer as a leaf rather than in a line above `musce_core`.

## The App injection

`App` is the whole of what the runtime needs from an app, and it is small:

- **`commands: CommandTable`** the in-app verb registry the embodiment frame
  dispatches against.
- **`admin: CommandTable`** the `@`-namespace builder verbs, capability-gated and
  rule-bypassing, dispatched through the admin frame. Same `CommandTable`
  mechanism as `commands`; the gate (`Gate::Cap`) carries the difference. Empty
  for an app with no builder surface.
- **`seed: fn(&mut World)`** builds the starting world when the database loads
  empty; a loaded world is left untouched.
- **`choose_actor`** the `@play` policy: which actor a connection comes to drive.
  It receives the authenticated `AccountId`, or `None` for a guest, so an app can
  select only that principal's characters. The reference stub still returns its
  single seeded avatar. Persisted `Controls`/`Focus` then resolves which body that
  selected character drives (see
  [networking-and-sessions.md](networking-and-sessions.md)).
- **`systems: Vec<fn(&mut SystemCtx)>`** the tick-loop systems the runtime carries
  on the phase pipeline, run in order every tick (see
  [concurrency.md](concurrency.md)). A `Vec`, so the runtime runs N by
  construction; empty for an app with no simulation.
- **`register: fn(&mut World)`** registers the app's own component (and relation)
  types on a fresh world, run before load and seed, since registration must
  precede deserialization. Engine types register themselves in `World::new`; this
  is where an app adds its own, so they round-trip through persistence like any
  built-in. The reference app registers its kind markers (`item`, `creature`,
  `container`, a player avatar, a gift recipient, an exit), its exit-connectivity relations
  (`LeadsFrom`/`LeadsTo`), and its behavior components (`Wander`, `Locked`,
  `Aliases`, the sequence types) here. An app-defined relation must be registered
  before any world that uses it is built or loaded, since registration is what
  wires its serialization and cascade; `register` runs before load and seed.
- **`affordances: fn(&World, &CapRegistry) -> Result<AffordanceRegistry, _>`**
  builds and activates the app's immutable canonical action vocabulary after
  world-type registration. Boot fails if its schemas or typed state readers do
  not match the registered world. The host owns the result and injects the same
  registry into every `Ctx` and `SystemCtx`, so verbs, clicks, scripts, and
  autonomous systems cannot accidentally execute against different vocabularies.
- **`caps: CapRegistry`** the app's capability vocabulary, interned to `CapId`s as
  it wires its `Gate::Cap` gates. The runtime resolves account grant strings against
  this same registry (see [authorization.md](authorization.md)); opaque ids carry
  registry provenance so accidentally mixing registries fails closed. Empty for an
  app with no capability-gated verbs.
- **`snapshot: fn(&World, EntityId) -> web::SnapshotData`** is the pointing web
  client's world projection: the perceivable containment tree for an actor, rooted at its locus
  and including that locus's relation-backed exits as clickable nodes (a click has
  no `go` box to type into), each node carrying the passive detail it perceives by
  presence, such as a `Description`. This is app policy because names, kinds, and
  detail projection are app vocabulary; the engine only routes the read and
  serializes the result (see
  [networking-and-sessions.md](networking-and-sessions.md) and
  [offers.md](offers.md)). An app with no pointing client returns empty projections.
- **`interactions: InteractionPolicy`** owns the pointing client's app-specific
  exposure boundary. Its read-only `offers` function maps a clicked entity to
  partial canonical groundings and optional candidates. Its `validate` function
  may narrow each complete, untrusted client grounding before execution. The host
  performs schema, sort, liveness, authority, and guard checks itself and invokes
  the same canonical performer used by commands and systems; the policy has no
  alternate mutation path. Actor identity comes from the authenticated session's
  live embodiment and is never supplied by the request. An app with no pointing
  surface uses `InteractionPolicy::none()`.

### The component boundary

The engine defines a component only when engine code *reads* it. Everything else
is app vocabulary and lives in the app, registered through `register` above. So
`Item`/`Creature`/`Container`/`Player`/`Exit` are not engine types: the engine
stores them but never interprets them (containment holds any entity in any entity;
what counts as a takeable "item" or a fillable "container" is an app rule). The
whole room graph is app vocabulary too, and it holds together: the reference app
owns both the room-as-place kind and the connectivity between rooms. Exit
*connectivity* (the `LeadsFrom`/`LeadsTo` relations) is defined in `musce_ref` over
the engine's public relation layer and cascades like any other app relation; no
engine code reads it. An app built on the engine defines its own kinds and
relations the same way, without modifying it.

One kind stays in core, and not by accident: it *is* the engine's model.

- **`Locus`** is the perception boundary: a scope in the containment tree that the
  engine finds (`enclosing_locus`) and snapshots at destruction (the `Fact`
  channel's `last_locus`). It is neutral. The engine assigns it no further meaning;
  the reference app tags its rooms with it, so co-located things in a room share a
  scope, but a non-MUD application (a data store with logic) could make its loci
  anything. This is the one world-model commitment the engine makes, and it is
  load-bearing: `enclosing_locus` and the audience resolver read it. "Room" is not
  an engine word.

Permissions are *not* such a kind: authorization is account-scoped, resolved to a
verdict at dispatch, not a marker the engine reads off the actor (see
[authorization.md](authorization.md)). An app whose perception is not
containment-hierarchical (a coordinate grid, a radius) would need the engine to
grow a parameterization seam (an app-supplied scope function); it is not built,
because it is not needed yet. The containment-scoped `Locus` is the one deliberate
model assumption the "an app never modifies the engine" rule is scoped within.

A plain struct of values plus fn pointers, matching the style the command and
component registries already use. A `trait App` is the alternative if an app ever
needs to carry its own state into these hooks; nothing needs that yet, so we do
not add it.

The account floor (`@quit`/`@who`/`@help`) stays in the runtime: it is session
management, not app content. Only `@play`'s choice of actor is app policy, which
is why it is the one floor concern the app injects.

## The engine's app-facing API

For an app to live in its own crate the engine must expose the surface an app
programs against. This is the real design work the split forces; the rest is
moving files.

- **`CommandTable` registration.** A public way to register a verb: a name, a
  permission `Gate` (`Open` for in-app verbs, `Cap` for capability-gated verbs), a
  handler. The lookup (exact name, then first registered prefix), the gate check,
  and `dispatch_command` (which both the embodiment and admin frames run through)
  stay engine mechanism; the verbs and their parsing are the app's. Registration
  asserts that each startup declaration is a nonempty lowercase parser word and is
  not an exact duplicate; ambiguous prefixes remain intentional and use order.
- **`Ctx` and a public emit API.** The handler context takes one `Caller` bundle
  (actor, connection, account-scoped `Verdict`) beside `&mut World` and the
  host-owned affordance registry, preventing
  those inputs from drifting. It exposes read-only `verdict`/`permits`/`is_su`
  authority queries plus a small public emit surface: a first-person line to the
  actor, a third-person line to the room excluding a set of parties (the actor, or
  the actor and a directed target both), and a directed line to a specific entity
  (resolved to its driving connections). Handlers are
  `fn(&mut Ctx, &str)`. The exact method names are an open detail; the shape is
  fixed. `Ctx::perform` always uses that injected registry. `SystemCtx::perform`
  does the same for autonomous callers, with an explicit verdict.
- **`execute` / `Action` / `ExecError`.** Already public: the structural mutation
  path an app's rule-checked handlers commit through.
- **The canonical affordance types and proposed `affordance!` surface.** The
  engine-owned schema, state registry, immutable affordance registry, and shared
  performer are built. The proposed app-facing description language declares
  typed inputs/results, logical requirements and effects, a resolution contract,
  and the Rust handler and narrator that implement the act. It will generate typed
  values, adapters, and registration metadata, then lower to the canonical AST; see
  [affordance-authoring.md](affordance-authoring.md).
- **The audience resolver, `Outbound`, and `Actors`.** Engine mechanism the app
  does not touch directly. `dispatch_bare` already takes the command table as a
  parameter, so it drives the app's table unchanged.

Name resolution leaves the engine entirely. Matching a typed noun against
descriptions is opinionated, English-leaning policy, so it lives in `musce_ref`
over the world queries the engine already exposes (`contents`, `container_of`,
`enclosing_locus`, component access). The engine owns no naming.

## The `musce` facade

An app depends on one crate: `musce`. It re-exports the engine's app-facing
surface, grouped by concept rather than by originating crate: `musce::world`
(identity, components, relations, queries), `musce::action` (verbs, dispatch, the
mutation path, the emit channel), `musce::store`, `musce::wire`, `musce::auth`,
the composition root (`App`, `run`, `Config`) at the crate root, and a curated
`musce::prelude`. An app never names `musce_core`, `musce_host`, or the rest
directly.

Grouping by concept decouples a public path from the crate that currently holds
the type: moving `Ctx` between crates, or merging two, does not move
`musce::action::Ctx`. The facade is the only stability contract an app binds to;
the internal split churns freely behind it.

`musce_ref` depends on `musce` alone, so the surface is self-testing: a gap is a
compile error in this repo, not a downstream app's discovery.

Optional subsystems attach as cargo features on the facade, not as dependencies an
app wires itself. The first is `musce_index` (a generic component index, reached
at `musce::index`; see indexes.md), taken with `features = ["musce_index"]`; a
plugin thus costs one feature flag on the crate an app already depends on, and stays
invisible to games that do not enable it.

## What moves where

| Concern | Lands in |
|---------|----------|
| `Action`, `execute`, `ExecError` | `musce_action` (engine) |
| `CommandTable` lookup + `register`, `Gate` | `musce_action` (engine) |
| `Ctx` + public emit API, the handler type | `musce_action` (engine) |
| audience resolver, `Outbound`, `Actors` | `musce_action` (engine) |
| `Locus` (perception boundary), `enclosing_locus`, containment, the relation layer | `musce_core` (engine) |
| the runtime, `run`, the `App` type, the floor | `musce_host` (engine) |
| exit connectivity (`LeadsFrom`/`LeadsTo`, `exits_of`) | `musce_ref` (app) |
| verbs (`look`/`go`/`take`/`drop`/`say`) + parsing | `musce_ref` (app) |
| admin/builder verbs (`@tel`/`@goto`/`@summon`/`@create`/`@dig`/`@set`) | `musce_ref` (app) |
| `Gate` tiers, `dispatch_command`, the admin frame | `musce_action`/`musce_host` (engine) |
| name resolution | `musce_ref` (app) |
| the seed world | `musce_ref` (app) |
| narration prose, the takeable rule | `musce_ref` (app) |
| `@play` actor-choice policy | `musce_ref` (app) |
| `main` and the end-to-end test | `musce_ref` (app) |

## Build order

1. Make the engine surface public: `CommandTable::register` and the `Ctx` emit
   API, so a verb can be defined outside `musce_action`.
2. Add the `App` type to `musce_host` and parameterize `run` over it: seed via the
   injected `seed`, and route `@play` through the injected actor policy.
3. Create `musce_ref`: move the verbs, name resolution, seed, narration, and the
   `@play` policy into it; give it `main` and the end-to-end test.
4. `musce_action` and `musce_host` now carry zero app content. Update the docs
   that described those verbs as living there to point here.

The crate and binary-target wiring is settled: `musce_ref` is a workspace member
with both a library and a binary (its `main`); `musce_host` is library-only (its
`main` moved to `musce_ref`), and the end-to-end test lives in `musce_ref` too.
