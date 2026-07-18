# Agency

> Status: **proposed; vocabulary, guards, binding, planner, arbiter, and driver
> built.** The
> affordance vocabulary (`Term`, `Predicate`, `Literal` with engine negation,
> `Clause`, `Affordance`, the case `Frame`), the `WorldModel` seam, and `Guard {
> clause, reason }` with `Affordance::veto` are **promoted into the engine**,
> non-optional, in `musce_action` (see [../affordances.md](../affordances.md));
> `musce_agency` re-exports them and keeps the optional planner side, the
> `CostModel` seam and the `bind_var` enumeration primitive (build steps 1 and 3).
> `musce_ref` grounds the `take`/`drop`/`put`/`go` verbs and carries their
> affordances, `RefWorldModel`, the `known_here` knowledge seed, and `perform` (the
> grounded-action dispatch a plan step lowers to). `put`'s container check and
> `go`'s locked-exit check are guards the handler and planner share. The **planner
> (backward regression) is built** (build step 4, see [planner.md](planner.md)):
> `musce_agency` carries `Planner`/`plan`, and `musce_ref`'s executable oracle runs
> a regressed plan through the same `perform` a player hits to satisfy a goal. The
> **arbiter and the execution driver are built** (build step 5, see
> [arbiter.md](arbiter.md) and [execution.md](execution.md)): `musce_agency` carries
> `Arbiter` (goal selection with commitment/hysteresis) and `Driver` (running a plan
> with replan-on-veto over `plan_excluding`), exercised with hand-injected goals.
> Two competing drives, the live wiring, and cross-tick commitment now exist: the
> reference magpie ([drives.md](drives.md)) hoards and admires on the tick, the arbiter
> holding a persisted commitment so the two do not thrash. Deferred: movement (`go`), a
> *consume* drive, and per-beat interleaving. Each `> Status:` marker says how settled.

"Agent" here is any entity that acts on its own: an NPC is the obvious case, but
the same machinery drives a possessed puppet running on autopilot, a summoned
familiar, or a piece of the world that pursues goals. Nothing below assumes the
actor is a named NPC.

The goal is one mechanism by which a thing gets done, reachable two ways: a
player types a verb, or an agent's planner selects an action. Both resolve to the
same rule-checked, world-mutating unit, so a scripted actor is vetoed exactly as
a player is. The engine already has the load-bearing half of this: `do_move` is a
grounded action shared by the `go` verb, the `wander` system, and sequences (see
[../actions.md](../actions.md) and [../sequences.md](../sequences.md)). This
folder is about generalizing that seam and building the autonomy on top of it.

## The stack

Four subsystems, top to bottom. All four now exist.

1. **Drives** turn the NPC's internal need-state into goals with an urgency.
   `eat-when-hungry` is a standing drive whose urgency is a function of the
   `Hunger` component; a prescribed order like `greet(playerX)` is an imperative
   goal injected from outside. Both emit `Goal { predicate, urgency }`. Drives
   read the NPC's *own* components, never the world or its beliefs.
2. **The arbiter** picks the highest-urgency unsatisfied goal and hands its
   predicate to the planner. Its real work is not selection but *commitment*:
   not thrashing between two near-equal goals every tick.
3. **The planner** regresses backward from the goal over the world graph filtered
   to what the agent **knows** (`Known` relation edges), chaining affordances by
   precondition until it reaches the current state, and emits a bound action
   sequence. This is the GOAP core.
4. **Execution** is the existing sequence sweep: a synthesized plan is a `Steps`
   list, run beat by beat through the same rule helpers a player hits, replanned
   when a beat is vetoed or beliefs change.

Under the planner sits the **affordance table** (the set of grounded actions,
shared with player verbs). There is no separate belief store: what an agent knows
is `Known` relation edges in the world graph, added by epistemic actions like
`search`, so knowledge is ordinary persisted world state the planner reads
filtered through. True stale / false belief (a cached view that diverges from
truth) is a deliberately deferred richer layer.

Two deliberate bypass seams: an imperative goal injects straight at the arbiter,
and a hand-authored sequence injects straight at the sweep, skipping planning
entirely. Not every behavior should pay for a planner run; a fixed greeting is
cheaper and more predictable as a script.

## Build order

The stack above is the runtime layering, top to bottom. The *build* order is not
that numbering: it follows falsifiability and reversibility, which put the
conceptual top layer (drives) last and the bottom of the planner first.

1. **The affordance and predicate/term types.** The affordance struct,
   `Term = Const | Var`, the `related`/`tag` clause form, the `PredicateRegistry`,
   and the `cost` *representation* (the planner obtains cost by calling a
   game-supplied function through the `Game` seam, never by reading a bare
   `affordance.cost` field, so a flat scalar, a bind-time computation, and the
   per-actor learned bias of step 6 are all the same seam and richer cost stays an
   addition rather than a signature change; see the affordances open question) are
   the wide-signature shapes the rest of the stack is written against, so their
   cardinality and encoding go first. What this step *is not*, on reflection: a
   serialized-shape decision. The earlier worry that a bound affordance would
   embed in a serialized `Step`/`Intent` (migration-class) is dissolved by the
   lowering resolution in the crate section: a synthesized plan lowers to the
   structural `Action` set the executor already runs, so no agency type is
   persisted and none embeds in `Intent`. As built, nothing in `musce_agency`
   derives `Serialize`, and the `String` encoding of kinds is a pure internal
   swap, not a migration (see affordances "Predicate representation"). So step 1
   goes first for the wide-signature reason, not an irreversible-serialization
   one; everything else in the stack is a cheap additive retrofit.
2. **Express an existing verb as a real affordance, oracle-validated.** A verb
   cannot yet *dispatch through* an affordance: there is no affordance executor
   until step 3's lowering, so "resolve through" would overclaim. What step 2
   honestly delivers is the affordance as **real game content** (`musce_ref`'s
   `agency::take`), not a test-only artifact, plus the `WorldModel` seam
   (`RefWorldModel`) that reads a ground predicate against this game's world, the
   read-side twin of `CostModel`. Ground truth is an executable oracle: run the
   real verb, bind the affordance's effect against the frame the parser would
   have built, and assert every predicate `holds` in the world afterward. This
   validates the **grounded (`Const`) path and effect projection** against real
   rules and makes divergence a test failure (wrong kind, flipped direction,
   empty effect), closing the falsifiability gap a hand-authored effect would
   leave. It does not exercise the `Var` / candidate-enumeration path the planner
   needs, nor free-variable evaluation; those get their own rung at step 3. Only
   `take` is carried for now: `go`'s effect names the exit's *destination*, which
   is not a frame role, so binding it is a step-3 modeling decision, not a
   freebie.
3. **Manual plan execution, before the planner.** *Built.* A hand-authored plan
   runs end to end without the parser, exercising the two step-3 primitives
   together: `bind_var` (candidate enumeration, the **shared primitive** the
   planner reuses) fills a plan step's fungible slot from what the actor knows,
   and the bound affordance executes through the game's grounded action.
   Correcting the earlier framing: a plan is **not** a persisted `Steps` list and
   there is **no affordance-carrying `Intent` variant**. The lowering resolution
   (crate section) is why: a synthesized plan is transient runtime output, and a
   plan step lowers by dispatching to the game's grounded action (`perform` →
   `do_take` / `do_drop` / `do_put` / `do_move`, returning a committed/refused
   `Outcome`), where the veto already lives, so a planned action is filtered and
   refused exactly as its typed verb is (the tests prove the veto rejects the
   planned path, not just the typed one, for every verb). Nothing agency-owned
   serializes; the persisted `Sequences` / `Intent` scripts stay a *separate*
   bypass seam (a hand-authored script injects at the sweep, per the two bypass
   seams above). `Known` is seeded the trivial way ("co-located ⇒ known",
   `known_here`); perception / sense-propagation is a deferred layer, deliberately
   not coupled in. What is absent is **regression** (chaining affordances by
   precondition): that is the planner proper, step 4.
4. **The planner. (Built.)** Backward regression over the affordance table on top
   of the step-3 binding primitive, falsifiable by the same executable oracle step
   3 uses: run the planner's *own* output through the sweep and assert the goal
   predicate became true. That is an independent check against world state, not a
   comparison to a hand-authored plan (many chains satisfy a goal; the planner may
   pick a different correct one), and it covers goals no hand-plan was written for.
   Cost *value* and heuristic start trivial (unit cost, no heuristic) and sharpen
   later; both are additions (the cost *representation*, by contrast, was pinned in
   step 1). Two cuts are deferred within the step and documented in
   [planner.md](planner.md): movement (`go`), because co-located-only `Known` makes
   no cross-room goal formable (so nothing exercises `go`'s derived-location
   effect), and the replan-on-veto loop, which belongs with the arbiter (step 5).
   The planner therefore plans within-room manipulation, add-only effects, uniform
   cost, ground and existential goals.
5. **The arbiter and the execution driver. (Built.)** Policy over a working
   planner, the textbook deferral: they land once it exists. The **arbiter**
   ([arbiter.md](arbiter.md)) selects the highest-urgency goal and commits with
   hysteresis so it does not thrash; commitment is only observable once something
   selects goals, which is why it follows a real planner. The **execution driver**
   ([execution.md](execution.md)) runs a committed goal's plan beat by beat and
   replans around a vetoed beat via the planner's `plan_excluding`, the exclusion-set
   seam step 4 left. A **first drive** and the **live sim wiring** followed: the
   reference magpie ([drives.md](drives.md)) reads competing `Hoarder` and `Curiosity`
   needs, emits hoard and admire goals, and runs the arbiter/driver loop on the tick
   through the same `perform` a player hits, the arbiter holding a persisted commitment
   (`Arbiter::resume` plus a game-owned tag) so the two do not thrash the bead. What
   stays deferred is a *consume* drive (which needs the planner's mid-search binding)
   and per-beat interleaving of the driver on the tick.
6. **Per-actor cost learning.** An agent keeps a running success statistic per
   affordance (an exponential moving average or a win / loss tally, not a trained
   model), and the game's cost function returns `base + learned_bias(actor,
   affordance)`, so an actor's costs drift toward what it actually succeeds at with
   no manual tuning. The mechanism is small but has hard entry gates it cannot
   precede. Its signal is the **beat outcome** the execution sweep already produces
   (a vetoed beat is a clean per-affordance failure, a committed beat a success;
   goal-level outcome is the wrong, badly-attributed signal), so it needs the
   planner and execution of steps 3 and 4. It has *nothing to learn* until at least
   one action carries a **variable outcome** (a skill roll, combat, a contested
   action), because a deterministic precondition-gated action always succeeds once
   selected. And by the falsifiability rule it must not ship until the static-cost
   planner of step 4 stands as a baseline and a metric exists for "is this actor
   succeeding more over time," or a drifting weight silently degrades the agent with
   no way to separate learner from planner. The learned component (a
   `map<affordance, stat>` on the actor, persisted per-actor like `Hunger`) and its
   update system are `musce_ref` content; `musce_agency` only exposes cost as a
   game-supplied function and never learns the weights exist. It is independent of
   step 5 (hand-injected goals drive enough actions to learn from), so it may land
   before or after drives, but only after the planner.

## Documents

- [affordances.md](affordances.md): the grounded action itself. Primitives are
  the structural action shapes and verbs are instances, effects as the committed
  mutation projected, the reference verb catalog, and how the veto lives once in
  the handler rather than duplicated into the planner (with the declarative slice
  now a shared `Guard`).
- [preconditions.md](preconditions.md): the symbolic vocabulary a precondition or
  goal is written in. The predicate set (`related`/`tag` plus content filters) as
  a view over the relation graph, why objects are identified by constraint rather
  than by id, and how a variable binds lazily against what an agent knows.
- [planner.md](planner.md): the built GOAP core. Backward regression over the
  affordance table, uniform-cost search, ground and existential goal binding, the
  add-only effect model with replan-on-veto as the soundness backstop, why `go`
  is deferred, and the executable oracle.
- [arbiter.md](arbiter.md): the built goal selection. Urgency ranking, the
  commitment/hysteresis that stops thrashing, why the arbiter never reads the world,
  and the imperative-goal injection seam.
- [execution.md](execution.md): the built execution driver. Running a plan beat by
  beat, replan-on-veto over the exclusion set as the soundness backstop, the generic
  `Beat` boundary, and the deferred one-beat-per-tick sim wiring.
- [drives.md](drives.md): the built drives and the live loop. The magpie's competing
  `Hoarder` and `Curiosity` needs, symmetric state-based relief, why *stow* and *hold*
  not *consume*, and the cross-tick commitment that keeps the two from thrashing.

## Deferred / not yet written

Perception and the `Known` relation (how edges are acquired and whether they persist
or decay, and the deferred false-belief layer), the planner's remaining pieces (a
cost heuristic and movement/`go`'s derived-location effect), and the per-actor
learning rule of build step 6 (the stat shape, the update rate, and whether weights
decay) still want their own doc. Drives now have one ([drives.md](drives.md)) and a
first instances, two of them competing under cross-tick commitment; what is left there
is richer drives (more urgency curves, imperative-goal injection and retirement), a
*consume* drive (needing the planner's mid-search binding), and per-beat interleaving of
the driver on the tick. The built core is [planner.md](planner.md),
[arbiter.md](arbiter.md), [execution.md](execution.md), and [drives.md](drives.md).

One cross-cutting dependency is noted but not owned here, and it is a standing
invariant rather than a planned migration: **plannable ∩ gated = ∅.** Every action
a planner reaches is ungated (gameplay verbs); every capability-gated action is
admin-only and parser-only. So the `Gate`/`Verdict` check stays connection-scoped
at `dispatch_command`, above the planner's reach, and agents never encounter it.
This holds by design; if it ever breaks, the capability requirement becomes an
affordance property checked in the executor's validate (where the gameplay vetoes
already live), which folds it into the single veto point without moving authority
onto the body. See [affordances.md](affordances.md) and
[../authorization.md](../authorization.md).

## Crate boundary and the world-index question

> Superseded in part by [../affordances.md](../affordances.md): the affordance /
> predicate vocabulary and `WorldModel` are now promoted into the engine
> (`musce_action`, non-optional; phase A built), with a guard-based dispatch veto
> to follow, leaving `musce_agency` as the optional planner/arbiter/drives layer.
> The generic / game split argued below still holds; the split moved from
> *crate-optional vocabulary* to *engine-non-optional vocabulary plus an optional
> planner*.

**The generic mechanism is its own crate, `musce_agency`.** It is carved up front,
with `musce_ref` as its first consumer, the way `musce_index` was: the generic
crate and its reference consumer landed together in one change, not prototyped in
the game and extracted later. The reason to draw the boundary early is that in Rust
the boundary *is* the enforcement of the one property this design most needs. The
**generic mechanism** (the planner's regression and unification, the arbiter's
commitment logic, the term/clause machinery, the affordance table with its by-name
and by-effect indexes, the `PredicateRegistry`) must stay separable from the **game
content** in `musce_ref` (the concrete affordances, the predicate parameters like
`Known`/`Locked`/`Food`, drives, goals, costs, rules). A crate boundary makes a
weld between them a *compile error*: `musce_agency` cannot name `Locked` or `Food`
because that would be an upward dependency on `musce_ref`. Module privacy inside a
single crate gives none of that, and deferring the crate would defer the guardrail
to exactly the moment welding has already set in. So the split resembles
`musce_index` (generic game-side mechanism a game consumes), not `sequences`
(welded to a serialized `Intent`), *because the boundary is what keeps it there.*

The one shape that could still weld agency to `sequences` is the plan step, and it
is resolved by keeping a synthesized plan **transient** and lowering it through
game code, not by a new serialized variant. A plan step lowers by dispatching to
the game's **grounded action** for that affordance (`perform` → `do_take` /
`do_drop` / `do_put` / `do_move`), the same handler-level unit a player's verb
runs, which validates its veto and *then* commits the structural `Action`. The
veto is why lowering is not a generic effect→`Action` translation: an affordance's
declared effect is the *symbolic* mutation the planner chains on and that the
step-2 oracle checks against the world, but the committing path must run the game's
rule (the affordance guard each `do_*` reads through `RefWorldModel`, and
`can_traverse` for `go`), so the game supplies the dispatch (see
[affordances.md](affordances.md) and [../actions.md](../actions.md)). Two things
keep the crate acyclic. The grounded actions live in `musce_ref`, which already
depends on `musce_agency`, so affordance types flow *downward* into `perform` with
no upward edge; and a plan is runtime output that never persists, so no
agency-owned type embeds in the serialized `Intent`. Routing plans through a new
`Intent` variant is the path that would force `Intent` (lower) to name an
affordance instance (higher) and create the cycle; transient lowering avoids it.
The executor's `Action` set still sits *below* agency in `musce_action`, reached
through the grounded action, so the structural mutation path is shared without
agency owning it.

**The planner needs no world index, by design.** A GOAP search looks like it would
query the world heavily to find candidates, but knowledge-gating bounds it: binding
a variable (`holds(self, x) ∧ tag(x, Food)`) enumerates the agent's `Known` edges,
a small per-agent set, and filters by tag, never scanning the world. So there is
nothing for `musce_index` (which indexes *world* components for *world-wide*
retrieval; see [../indexes.md](../indexes.md)) to accelerate in the planner, and a
planner that *did* consult a world index ("where is any food") would reintroduce
the omniscience the `Known` filter exists to prevent. The planner's own indexes
(by-name, by-effect) are over the small static affordance table, not over entities,
so they are not `musce_index` either.

Where `musce_index` legitimately meets agency is one layer over, in **perception**,
the thing that *creates* `Known` edges. Co-located perception is a cheap
locus-contents read with no index; sensing at range (deferred sense-propagation) is
the only place the spatial index could serve, and that is perception logic, outside
the planner. The governing principle: **ignorance is gameplay, not a query.** An
agent that does not know where food is should behave like it, wander, head where
food is usually found, ask someone, all app logic, rather than consult a global
lookup. A game may keep authored search priors (a kind-to-locations table) if it
wants data-driven wandering, and that is the one spot a *game* might index; the
planner never does.
