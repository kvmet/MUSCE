# Structural Facts

> Status: **built.** `Destroyed`, `Moved`, `LocusChanged`, and `ComponentChanged`
> are emitted at the `World` mutator layer and drained once per tick into
> `SystemCtx::facts`; the reference game's `death_cry` reaction consumes `Destroyed`.
> `ComponentChanged` is the bounded trigger a derived index rides; its first
> consumer (`musce_index`) is built separately. See
> [fact.rs](../../musce_core/src/fact.rs) and
> [world.rs](../../musce_core/src/world.rs). The executor and action vocabulary that
> sit above this channel are in [actions.md](actions.md).

Structural mutations emit typed **facts** for game logic to react to. A fact is an
*observation* of a mutation, not a mutation, so the rule that an action is the only
thing that mutates still holds (see [actions.md](actions.md)): a reaction reads facts
and may produce its own actions, but the fact stream changes nothing on its own.

## Which mutations get a fact, and why most do not

The set is deliberately small and is *not* one-fact-per-mutator. A mutation earns a
fact for one of two reasons, and only these:

1. **It carries unrecoverable state a reaction needs.** The post-mutation world can
   no longer answer the question: either the mutation *destroyed* the state (a
   destroyed entity's locus and name) or the change happened somewhere a system
   cannot otherwise observe (a cascade removal below `execute`). This is the
   **payload-carrier** role; the fact ships exactly the vanished data and no more.
   `Destroyed`, `Moved`, and `LocusChanged` fill it.
2. **It is a bounded trigger a per-tick maintainer cannot cheaply poll for.** A
   consumer that keeps a derived read-model current (a secondary index) would
   otherwise rescan the world every tick to discover what changed. A trigger lets it
   react to the handful of entities that moved instead of the whole table. This is
   the **trigger** role; `ComponentChanged` fills it. It stays bounded by an explicit
   opt-in (`World::track_component`): a component emits nothing until a consumer asks
   to track it, so the stream can never grow to narrate everything.

The two roles are not disjoint, and that is the point that keeps widening honest:
`Destroyed` already serves both. It carries an unrecoverable payload for `death_cry`
*and* an index consumes the very same fact purely as a lifecycle trigger (evict the
gone entity, reread nothing). `ComponentChanged` extends that existing
trigger-consumption rather than inverting the charter.

A mutation whose result is fully queryable afterward *and* that no maintainer needs a
trigger for earns no fact; a game that wants to fire on such an event uses a marker
or a system, not this channel. Facts recover the *unrecoverable* or feed a *bounded
maintainer*; they do not narrate. This is the test every candidate fact is measured
against, and the live proof it still has teeth is that `Created` is **declined** on
the trigger half (see below): a payload-free birth signal fails the "bounded" clause,
because `spawn` cannot enumerate which tracked components a built entity carries, so
it could not be gated by `track_component`.

## Where facts are emitted, and when they are read

Facts are emitted at the **`World` mutator layer, not `execute`**, and that placement
is load-bearing. A single `@destroy` cascades through the relation layer *below*
`execute` (a destroyed room takes its exits with it via `DespawnSources`); only the
mutator recursion observes those cascade removals, so emitting from `execute` would
catch the targeted entity and miss its collateral. `execute` and every verb call site
therefore stay untouched.

Facts buffer on a transient `World` field, drained **once per tick** by
`Dispatch::run_systems` at the top of the system loop into the read-only
`SystemCtx::facts` slice every system sees. That timing sets the latency: a
command-driven mutation (`@destroy`/`@purge`, drained before `run_systems`) is reacted
to the **same tick**, while a fact a system emits is seen the **next tick** (buffered
after the drain), so no system sees another's fact within a pass and system order is
cosmetic. A reaction is just a `System` iterating `ctx.facts`; the reference game's
`death_cry` narrates a destroyed thing's demise to its room.

## Destroyed

`Fact::Destroyed { entity, last_locus, name, cause }`. `last_locus` and `name` are a
**pre-removal snapshot** (captured while the entity is still live, between the
cascade-handler loop and the index removal, because a reaction reads them after it is
gone): `name` is the entity's `Name` handle, falling back to its `Description` (`None`
if it carries neither), and `last_locus` the `enclosing_locus` (`None` for a top-level
locus or a location-less entity). `cause` is `Direct` for the targeted entity and
`Cascade` for one swept up by a cascade; this discriminator lets one reaction catch
every removal in a recursive `@purge` (all `Direct`) yet skip the collateral of a
single `@destroy <room>` (room `Direct`, exits `Cascade`). A `Cascade { root }`
enrichment is deferred until a reaction needs to group a cascade by origin.

`Destroyed` is the exemplar of the test: destruction annihilates the dying entity's
locus and name (unrecoverable after the fact, hence the pre-removal snapshot) *and*
its cascade removals happen below `execute` (otherwise unobservable). Both halves of
the test at once, which is why it was the first fact.

## Movement: Moved and LocusChanged

Two facts, both about movement, meet the test and are emitted at the containment
mutator:

- **`Moved { entity, from, to }`** on every containment change. `from`/`to` are
  containers; querying after a move yields only `to`, so the prior container `from` is
  the vanished state this recovers (`None` at either end for a root). It serves
  containment-scoped reactions (encumbrance, "the idol left the pedestal fires the
  trap").
- **`LocusChanged { entity, from, to }`**, emitted *additionally* and *only* when the
  move crosses the enclosing `Locus`. `from`/`to` are loci; `from` is the vanished
  prior locus. It serves perception-scoped reactions (presence, "X left" / "X
  arrived", region triggers, and the future shard handoff, which happens at the locus
  boundary).

They are two facts, not one `Moved` carrying four fields, because their audiences
differ: a containment reaction never wants to think about loci, and a perception
reaction never wants to recompute `from_locus != to_locus`. The engine computes the
distinction once, at the mutator, where the vanishing `from_locus` is still
resolvable. A same-locus reparent emits only `Moved`; a room-to-room walk emits both.

### The carried subtree does not move (and why that is not a gap)

A move fires `Moved`/`LocusChanged` for the entity whose **own** containment link
changed, and for nothing it carries. A character walking from the hall to the garden
reparents only itself; the sword in its hand and the coin in its bag keep their links
(`contained_by` the character), so the engine emits no fact for them, even though
their *enclosing locus* changed along with the character's.

This is not an oversight; it follows from the selection test rather than
contradicting it. The sword's locus change is **derivable**: after the move the
sword's enclosing locus is queryable (the garden), and the sword is under the
character, so "the sword went where the character went" is reconstructable from the
character's `LocusChanged` plus the containment tree. A fact exists only for what a
reaction *cannot* reconstruct, so emitting one per carried item would violate the
principle, and would spam besides (a character with fifty items would emit fifty-one
`LocusChanged`). The engine emits the one fact that is not derivable, the mover's,
and stops.

The **right way for a consumer** to react to the carried subtree is to start from the
mover's fact and walk `descendants(entity)` itself, once, only when it needs to (a
region trigger usually cares about the character stepping into the lava, not each coin
in the bag). Pushing this to the game is deliberate: only the game knows *whether* the
subtree matters for a given reaction, and it computes it cheaply on demand, whereas
the engine emitting it eagerly would pay that cost on every move for reactions that
mostly do not want it. The rule (fire for the entity whose own link changed) is also
exactly the rule the reparent cascade already obeys: a destroyed container's surviving
children each have their *own* link rewritten, so they emit; a carried item never
does. One rule, no special cases, derived data left where it can be computed lazily.
**This is settled, not a limitation to lift later:** revisiting it means re-deriving
in the engine what the consumer can already derive itself, at a cost the consumer does
not always want to pay.

## ComponentChanged

`Fact::ComponentChanged { entity, tag }`. Unlike the others this is a **trigger, not
a payload-carrier**: it names only the entity and the changed component's `tag` and
carries no value. A maintainer reconciles by **rereading** the current world: the
value is present after a set (upsert it) and absent after a remove (drop it,
recovering the old key from the maintainer's own `entity -> key` reverse map). Nothing
here is unrecoverable, so on principle nothing is carried; and on cost, `Fact` is a
non-generic enum, so any payload would be type-erased (`Value` or `Box<dyn Any>`) at a
serialization or heap cost per write that a free typed reread beats.

**Bounded by opt-in.** A component emits nothing until a consumer calls
`World::track_component::<C>()`. The tracked set is what stops this from becoming a
narrate-every-write channel: fact volume is proportional to the components someone
actually indexes, not to all mutation.

**Emitted from the mutator layer, at every write it mediates:** `set_component` /
`remove_component` (tag-driven), the typed `insert<C>` / `remove<C>`, `create` (once
per tracked tag in the blob, post-spawn so a rereading maintainer sees the new
values), and `modify<C>`. It is **not** emitted from `spawn`, which takes an
already-built `EntityBuilder` and cannot enumerate the tracked components inside it;
boot content built through `spawn` (and `seed`, which runs through it) is invisible to
the trigger stream and is instead covered by a maintainer's one-time baseline scan.

**Idempotent and order-free.** Duplicate triggers for one entity in a tick are safe
because reread converges regardless; a maintainer may coalesce its dirty set but is
not required to. Order against a same-entity `Destroyed` in the same tick is likewise
irrelevant: whether the index rereads-then-evicts or evicts-then-rereads-nothing, it
ends empty for a gone entity. A maintainer must still consume `Destroyed` (both
`Direct` and `Cascade`): `despawn` tears an entity down wholesale and emits only
`Destroyed`, never a per-component remove, so without it a destroyed tracked entity
leaves a stale index entry.

**The raw `&mut` bypass.** `ComponentChanged` fires only for writes that pass through
the `World` mutator. A write through the raw handle, `world.ecs().get::<&mut C>()`,
mutates in place *below* that layer and emits nothing, so a tracked index over such a
component silently desyncs. Two guardrails: `World::modify<C>(id, f)` is the supported
in-place path (mutate then emit), and `World::forbid_tracking::<C>()` lets a game
declare a raw-mutated type, after which `track_component` of it is a hard panic rather
than a silent desync. Neither is a debug-only reconciliation (the "`except -> pass`"
pattern the repo forbids on the critical path); they fail loud at startup.

## Created, Related, and Unrelated get no fact

By the test above, `Related` and `Unrelated` earn **no** fact: their result is fully
queryable afterward (a new link is readable) and no maintainer needs a trigger for
them yet, so a game hooks them with a marker or a system. They become facts only if a
concrete reaction ever needs pre-mutation state they destroy (e.g. the old target a
re-`relate` overwrote), and then carrying exactly that and no more.

`Created` is the instructive one: it fails **both** halves of the test. It carries
nothing unrecoverable (a spawned entity is right there to query), and as a trigger it
cannot be bounded, because `spawn` takes a built `EntityBuilder` and cannot enumerate
which tracked components it holds, so a `Created` signal could not be gated by
`track_component` and would fire on *every* spawn while an index cares about a
fraction of them. That is the narrate-everything bloat the opt-in exists to prevent.
Creation reaches a maintainer by two other paths instead: the one-time baseline scan
at boot, and `ComponentChanged` from a post-boot `create`/`insert`/`set`. A real
`Created` trigger is added only if a genuine post-boot bare-`spawn` path (a builder
already carrying a tracked component) ever falsifies that coverage.
