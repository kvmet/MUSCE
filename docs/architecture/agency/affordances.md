# Affordances

> Status: **partially built.** The `Affordance` struct, the case `Frame`, and
> `Guard { clause, reason }` with the `Affordance::veto` evaluator are built in the
> engine (`musce_action`, non-optional; see [../affordances.md](../affordances.md)).
> `musce_ref` carries the first affordances (`take` / `drop` / `put`),
> `RefWorldModel`, the `known_here` knowledge seed, and `perform` (the
> grounded-action dispatch); `take` is grounded as `do_take` so a player and a plan
> share one veto, and `put`'s handler now reads its container guard through the same
> affordance the planner would. The affordance *table* and its two indexes, the
> rest of the verb catalog as `musce_ref` instances, and the planner (regression)
> remain proposed. This doc covers what a grounded action carries and how the
> reference verbs collapse onto a handful of structural shapes; the symbolic
> vocabulary a precondition is written in lives in its sibling
> [preconditions.md](preconditions.md).

## The affordance

An **affordance** is a grounded action: the reusable unit both a player verb and
an NPC planner resolve to. It carries four things:

- **A case frame** binding the actors and objects: `(actor, object?, target?,
  kind?)`. This is the same frame the parser already produces (`verb / dobj /
  prep / iobj`) and the same frame the structural action already is
  (`Relate(source, target, kind)`, with `Move` its containment face). See
  [../actions.md](../actions.md). `kind` is the preposition/relation:
  `in`/`to`/`with`.
- **A guard set:** predicates that must hold for the action, each paired with the
  reason a player hears when it fails (`Guard { clause, reason }`). This is a
  symbolic approximation of the handler's real pre-commit rule, not the rule
  itself. It has two readers: the handler calls `Affordance::veto` to gate the
  verb (showing the first failing guard's reason), and the planner reads the same
  clauses to test plannability. The parts of the real rule the vocabulary cannot
  express (`can_traverse`, the containment cycle) stay truth and re-check at
  execution. See [../affordances.md](../affordances.md).
- **An effect set:** the predicates the action makes true or false, so the
  planner can chain backward toward a goal.
- **A cost:** the edge weight A\* minimizes. Where personality lives.

A **verb** is `affordance + text parser`. A **GOAP action** is `affordance +
(precondition, effect, cost)`. The grounded core is shared; the two front-ends
are independent. A player never touches the symbolic model; an NPC never parses
text.

## Primitives are the structural action set; verbs are instances

The affordance *primitives* are not `take`/`unlock`/`cook`. They are the
structural Action shapes the executor already has (see [../actions.md](../actions.md)):

- `Move(entity, into)`
- `Relate(a, b, K)` / `Unrelate(a, b, K)`
- `SetComponent(e, C, v)` / `RemoveComponent(e, C)`
- `Create(kind)` / `Destroy(e)`

That set is closed, small, and shape-free. A **verb** is one of those shapes with
bound parameters, a rule predicate, cost, and prose. This is the same collapse the
predicates went through one level up: the kinds are closed, the parameters are
open game vocabulary. `unlock` is not a primitive; it is `RemoveComponent(target,
Locked)` plus a has-key rule, and writing it down assumes `Locked` exists, that a
door is the thing, and that unlocking means removing that component. All of that is
world shape and lives in `musce_ref`, exactly like `Locked`, `container`, and
exits already do. `open`/`close`/`lock`/`unlock`/`cook`/`light` are not six
affordances; they are one primitive (`SetComponent`/`RemoveComponent`)
instantiated six ways. So the affordance layer invents no new primitive set: it
reuses the executor's vocabulary and adds per-instance metadata.

## Effects are the committed mutation, projected

An affordance's effect *is* the structural Action it commits, read as `related` /
`tag` predicates. Nothing is separately authored:

- `Move(item, into=actor)` → `holds(actor, item)` true, `holds(old, item)` false,
  read off `(source, target, kind)`.
- `Relate(self, x, Known)` → `knows(self, x)`. Knowledge is world state, so
  acquiring it (`search`) is an ordinary mutation, not a special channel; the
  "indirect objects must be *found* first" problem FEAR barely has falls out of
  this. (Which `x` a `search` discovers is settled at execution; see
  [preconditions.md](preconditions.md), "Argument binding.")
- `SetComponent(food, Cooked)` → `tag(food, Cooked)`. A state transition's effect
  is the tag it sets or clears, derived exactly as a `Move`'s is. There is no
  "authored effects" bucket.

The one case that is *not* a direct projection is a verb that changes a **value**
whose planner-relevant meaning is a threshold (`eat` lowers hunger). Do not plan
over the number: a game **system** maintains a derived tag at the threshold
(`tag(self, Hungry)` clears when hunger drops), and the verb's chainable effect is
that tag flip. Same escape hatch as "Testing component content" in
[preconditions.md](preconditions.md); the numeric work stays in a system, the
planner sees a tag.

## The reference-game verb catalog

Not the primitive set: a catalog of `musce_ref` *instances*, each a primitive
shape with bound params, a rule, and prose. The point of the table is that a large
verb surface maps onto a handful of shapes. The frame column shows how the verb
binds; the primitive column is the shape it instantiates.

| Verb | Case frame | Instantiates |
|---|---|---|
| `take` | (actor, object=item) | `Move(item, into=actor)` |
| `drop` | (actor, object=item) | `Move(item, into=locus)` |
| `give` | (actor, object=item, target=being) | `Move(item, into=being)` |
| `put` | (actor, object=item, target=container, kind=`in`) | `Move(item, into=container)` |
| `go` | (actor, target=exit) | `Move(actor, into=dest)` |
| `open` / `unlock` | (actor, object=door[, kind=`with`, target=key]) | `RemoveComponent(door, Locked)` |
| `close` / `lock` | (actor, object=door) | `SetComponent(door, Locked)` |
| `cook` / `light` | (actor, object=item[, target=appliance]) | `SetComponent(item, C)` |
| `eat` | (actor, object=food) | `Destroy(food)` (+ need via system) |
| `search` / `examine` / `look` | (actor, object?) | `Relate(self, x, Known)` |
| `greet` / `emote-at` | (actor, target=being) | none (precondition-only) |

The verbs differ from each other only in bound params (which component, which
destination), the rule (`unlock` needs a matching key, `open` does not), and prose;
the shape is shared. That is why the set feels large but is not: `open`, `unlock`,
`close`, `lock`, `cook`, and `light` are all one `SetComponent`/`RemoveComponent`
primitive, and what distinguishes them is game vocabulary the engine never sees.

Notes on the edges of the set:

- **Communicative verbs mutate nothing** (see command-dispatch.md) yet still want
  to be affordances, because a goal like `greeted(X)` is achieved by `emote-at`,
  whose precondition `near(actor, X)` is what makes the planner discover `go`
  first. So an affordance can have an empty effect on the *world* and still carry
  a precondition and a belief/goal effect. Open question below: do these need a
  full table entry or just a goal-satisfaction hook.
- **`unlock` is the first instrument verb** (`with key`). It fits the two-slot
  frame only because we treat the key as `target` under `kind=with`. A true third
  object (`put coin in slot on machine`) is out of the frame; deferred, and adding
  a slot later is an addition to the frame, not a reshape (the migration is the
  0-to-2 decision, not 2-to-3).

## The veto lives once, in the handler

The checks that decide whether a mutation may happen must not be duplicated into
the planner. They live in one place, the handler's pre-commit validate phase (the
only veto point; see [../actions.md](../actions.md)), keyed on the **actor and the
world**, never on the input channel. Because an agent's plan executes as a `Steps`
sequence through the same verb helpers a player command hits, every gameplay veto
(`can_traverse`, reachability, takeable) runs exactly once and vetoes a planned
NPC identically to a player. This is the existing `do_move` design, extended: it is
already why a scripted mover is stopped by a locked door.

The declarative slice of that veto is now a shared `Guard`: `put`'s container check
is one guard clause, evaluated by the handler through `Affordance::veto` and read
by the planner for plannability, so the two cannot drift (see
[../affordances.md](../affordances.md)). The imperative remainder (`can_traverse`,
the containment cycle) still lives only in the handler, which is the rest of this
section.

**Only restate a veto the planner can act on.** You cannot mechanically derive a
precondition from arbitrary rule code (`can_traverse` is imperative; there is no
extracting "what must be true" from it), so *some* symbolic restatement is
inherent to planning, not eliminable duplication. Bound it: give the planner a
symbolic precondition *only* for a condition it can chain an action to satisfy
(door `Locked` → `¬ tag(door, Locked)`, because it can `unlock`). A veto it cannot
decompose into another action (an arbitrary rule, a capability it lacks) gets
**no** symbolic precondition; the planner stays optimistic, execution vetoes, it
replans. This is the chainable/filter split again: the chainable preconditions are
the plannable slice worth restating; every other veto lives once in the handler
and is discovered by trying.

**One deliberate exception: the capability gate stays above the planner.** Unlike
the gameplay vetoes, the `Gate`/`Verdict` check is not resolved in the handler. It
sits one layer up, at `dispatch_command`, from the connection's session-cached
account (see [../authorization.md](../authorization.md)). It keys on the
*connection*, and an agent has none, so it does not fall through to a planned actor
the way the handler vetoes do. This is correct, not a gap, and it rests on one
standing invariant: **plannable ∩ gated = ∅.** Every action a planner can reach is
ungated (the gameplay verb set); every capability-gated action is admin-only and
parser-only (the `@`-admin set). The one veto not unified into the handler is
precisely the one nothing plannable can hit, so the planner never needs it and the
connection scoping never bites.

If that invariant ever breaks (a planner-reachable action grows a capability
requirement), the fix is bounded and known: make the requirement a property of the
**affordance**, checked in the executor's validate phase against `ctx.verdict`,
where every gameplay veto already lives. That folds the capability check into the
single veto point rather than adding a parallel one, and it does *not* move
authority onto the body: the verdict is still resolved per-account and threaded
through `Ctx` (which already carries it), an agent just supplies one from its
authority source instead of a connection (an authorization.md concern when it
lands). Until the affordance table and its by-effect index exist there is nothing
to hang enforcement on, so this stays a documented invariant; when they exist it
graduates to a cheap registration-seam assertion, `plannable ⇒ ungated`, that fires
the moment a gated affordance is registered into the planner's reach.

## Two indexes over the set

The affordance table is queried two ways, so it needs two indexes, and this is a
shape decision worth naming now rather than an additive afterthought:

- **By name** for the parser: a player types `take`, look up the affordance,
  bind the frame from the rest of the line. This is today's `CommandTable`.
- **By effect** for the planner: regression asks "what affordance makes
  `holds(self, food)` true?" and needs a reverse map from effect-predicate
  template to the affordances that produce it. The planner never materializes a
  graph of options; it generates successors lazily through this index, because a
  materialized graph is every-object times every-verb times every-state.

## Open questions

Deliberately unresolved; listed so they are not mistaken for decided.

- **Communicative affordances.** Full table entries with empty world-effects, or a
  lighter "goal-satisfaction hook" that does not participate in regression the way
  world-changing actions do? Still open.
- **Cost model.** *Seam decided; policy open.* Cost is not a field on the
  affordance but a game-supplied `CostModel::cost(actor, affordance, world)`
  (built; the generic crate ships only `UnitCost`). Whether a game's model is flat
  per-affordance or scaled by distance/effort at bind time is still open, as is
  where soft content preferences land (grade the pencil by uses remaining rather
  than filter on it; see [preconditions.md](preconditions.md), "Gate or grade").
  Per-actor *learned* cost is build step 6 over this same seam.
- **Derived-effect extraction.** *Decided: declared.* The `Affordance` carries an
  explicit `effect: Clause`, and the by-effect index reads that, keeping the
  executor's internals out of the mechanism. Auto-projecting the effect off the
  `Action` a verb commits is a possible later refinement; it is elegant but couples
  the index to executor internals, and may never be worth it.
