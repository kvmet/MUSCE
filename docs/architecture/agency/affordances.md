# Affordances and Preconditions

> Status: **partially built.** The core vocabulary, `Term`, `Predicate`
> (`Related` / `Tag`), `Clause`, the `Affordance` struct, the `WorldModel` seam,
> and the case `Frame`, are built in the engine (`musce_action`, non-optional; see
> [../affordances.md](../affordances.md)); the `CostModel` seam and `bind_var` stay
> on the optional planner side in `musce_agency`. `musce_ref`
> carries the first real affordance (`agency::take`), `RefWorldModel`, the
> `known_here` knowledge seed, and `perform` (the grounded-action dispatch); the
> `take` verb is grounded as `do_take` so a player and a plan share one veto. A
> hand-authored plan runs end to end in tests (enumerate a candidate, execute
> through the veto). The affordance *table* and its indexes, the rest of the verb
> catalog as `musce_ref` instances, and the planner (regression) remain proposed. This doc enumerates what a grounded action carries and what the
> symbolic vocabulary looks like; the concrete affordances become `musce_ref`
> content over the engine's action layer, and the engine owns none of the game
> predicates.

## The affordance

An **affordance** is a grounded action: the reusable unit both a player verb and
an NPC planner resolve to. It carries four things:

- **A case frame** binding the actors and objects: `(actor, object?, target?,
  kind?)`. This is the same frame the parser already produces (`verb / dobj /
  prep / iobj`) and the same frame the structural action already is
  (`Relate(source, target, kind)`, with `Move` its containment face). See
  [../actions.md](../actions.md). `kind` is the preposition/relation:
  `in`/`to`/`with`.
- **A precondition set:** predicates that must hold for the action to be
  *plannable*. This is a symbolic approximation of the handler's real pre-commit
  rule, not the rule itself. The real rule (`can_traverse`, reachability) stays
  truth and re-checks at execution; the precondition set only steers the planner.
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
  this. (Which `x` a `search` discovers is settled at execution; see "Argument
  binding.")
- `SetComponent(food, Cooked)` → `tag(food, Cooked)`. A state transition's effect
  is the tag it sets or clears, derived exactly as a `Move`'s is. There is no
  "authored effects" bucket.

The one case that is *not* a direct projection is a verb that changes a **value**
whose planner-relevant meaning is a threshold (`eat` lowers hunger). Do not plan
over the number: a game **system** maintains a derived tag at the threshold
(`tag(self, Hungry)` clears when hunger drops), and the verb's chainable effect is
that tag flip. Same escape hatch as "Testing component content" below; the numeric
work stays in a system, the planner sees a tag.

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

## The precondition / predicate set

The vocabulary the planner regresses over should mirror the world's *own* generic
structure, not name game states one at a time. The engine already treats relation
kinds and component types as opaque game vocabulary (`Contains`, `item`,
`container` are markers the engine never interprets); the predicate set is those
same structures asked as questions. So the predicate *kinds* are a tiny closed
set and their *parameters* are open game vocabulary.

Two primitive predicate kinds:

| Predicate | True when | Backed by |
|---|---|---|
| `related(a, b, K)` | a relation of kind `K` links `a` to `b` | the relation graph |
| `tag(e, C)` | `e` bears component/marker `C` | component presence |

Everything specific is one of these parameterized by a game kind. Do **not** add
`cooked`, `edible`, `armor`, `worn`, `knows` as predicates; they are
`tag(x, Cooked)`, `tag(x, Edible)`, `tag(x, Armor)`, `related(self, x, Worn)`,
`related(a, e, Known)`. A new game state is a new *parameter*, never a new
predicate kind, so it stays an addition and never a migration (the same
closed-kinds / open-parameter split as `EventKind` vs the `channel` tag; see
[../command-dispatch.md](../command-dispatch.md)).

Familiar predicates are **macros** over the two, named for readability but not new
primitives: `holds(a, i)` = `related(a, i, Contains)`; `knows(a, e)` =
`related(a, e, Known)`; `at(e, L)` = the enclosing-locus walk over `Contains`;
`near(a, t)` = `a` and `t` share an enclosing locus; `reachable(a, i)` = `near`
plus no closed container between them; `open(c)` = `¬ tag(c, Locked)`. So the
relation-graph economy holds fully: the whole model is `related`/`tag` queries
against state that already exists. There is **no separate belief store**:
knowledge is `Known` edges in the world graph, which persist like any relation
(so an agent's memory survives a restart for free), and the planner reads truth
filtered to entities the agent has a `Known` edge to.

This makes the MVP knowledge semantics honest and narrow: an agent can act only
on entities it has encountered, but it is never *wrong* about their current state,
because state is re-read from truth rather than cached. A `Known` edge is
acquaintance, not a remembered snapshot. True stale / false belief (believing a
key is where it no longer is) needs a cached, divergent-from-truth belief and is a
deliberately deferred richer layer; it does not change the predicate
(`related(self, x, Known)` stays), only what backs it, so deferring it is an
addition, not a migration. Whether `Known` edges persist or stay transient, and
whether they ever decay, are later calls, not MVP shape.

### Testing component content, not just presence

`tag(e, C)` tests that a component is *present*. Real preconditions routinely
constrain its *content*: not any pencil, a pencil with at least half its uses
left; not any container, one that still has room. The vocabulary needs this, but
the way it enters matters, because content splits by a role the presence tags do
not have: **chainable versus filter.**

`related` and `tag` are **chainable**: the planner regresses *through* them, and
an action can make them true (they are effects). A content test almost never is.
"A pencil with 50% uses" is not something to *plan to achieve*; it constrains
*which* pencil binds. So content enters as a **filter**, not a chainable
primitive:

- **As a binding filter (and a cost input): supported, fully general.** When
  binding a variable, enumerate the candidates satisfying the structural
  constraints (`related`/`tag`) and filter by the content test. The planner treats
  it as an opaque boolean at bind time, so it can be arbitrarily computed (a
  `remaining / max ≥ 0.5` ratio, cross-component logic). It is cheap *because* the
  planner only tests it and never reasons about how to satisfy it.
- **As a planning goal ("raise this pencil to 50%"): out of scope.** Chaining
  actions toward a numeric threshold is metric planning, categorically harder than
  propositional GOAP, and not something an agent planner should attempt. If a
  threshold genuinely must be *reached* by planning (sharpen a sword until it can
  cut), do not make the planner do arithmetic: have a game **system** maintain a
  derived **tag** at the threshold (`tag(sword, SharpEnough)`) and let the planner
  chain over the tag. Numeric accumulation is the system's concern; the planner
  sees a tag flip. Same shape as forward-value-is-truth, the tag a derived index.

**Representation.** A raw field/op/value comparison cannot express the ratio, so a
content test is a **game-supplied boolean predicate over a candidate**, referenced
by an interned id plus args so it serializes into goals and plans while the
function stays game code (a small `PredicateRegistry`, the same interning move as
`CapRegistry`). A raw field comparison is just the common built-in case; ratios
and compound tests are game fns.

**Gate or grade.** Separate a hard filter (`uses > 0`, must pass, prunes the
candidate) from a soft preference (prefer the pencil with the most uses, a cost
bias). The hard form is a bind-time filter; the soft form feeds the cost model
(still open below). Both are bind-time, neither is chainable. And correctness is
unchanged: filters re-evaluate at execution, so a candidate consumed between plan
and act fails its beat and replans.

### Objects are identified by constraint, not by ID

The thing the specific predicates hid. A predicate argument is a **term**: a
bound entity, *or* a variable constrained by other predicates in the same clause.

```
Term = Const(EntityId) | Var(name)
```

A goal or precondition is then a small conjunction over terms, so a want is a
*pattern to satisfy*, not a named entity:

| Intent | Clause |
|---|---|
| have food | `∃x. holds(self, x) ∧ tag(x, Food)` |
| have armor | `∃x. holds(self, x) ∧ tag(x, Armor)` |
| be wearing armor | `∃x. related(self, x, Worn) ∧ tag(x, Armor)` |
| ensure P has armor | `∃x. holds(P, x) ∧ tag(x, Armor)` |

`self` and `P` are constants (`P` bound from the imperative goal that set it); `x`
is a variable the planner binds lazily against perceived entities matching the
constraint (see "Argument binding" below). Identifying by ID is just the case
where the term is already `Const`. This is what lets one `fetch` affordance serve
food, armor, keys, and quest items with no per-thing variant: the affordance is
written over terms, and the goal supplies the constraint.

**`Const` vs `Var` is the fungibility axis.** A fungible want ("any food") is a
`Var` plus constraints; a non-fungible one ("greet *this* king") is a `Const`, one
binding, no predicate. There is no separate identity primitive: pinning an entity
*is* using its `Const` term, and identity is never chainable (no action's effect
makes `x` *be* a given entity), so it could never be a `related`/`tag`-style
predicate anyway. A `Const`-pinned slot still chains its structural preconditions;
it only fixes which candidate binds.

`Const(id)` is the singleton degenerate of a `Var` filtered to one candidate
(`tag(x, Player) ∧ eq(x, id)` binds the same entity), which is *why* identity is
not a primitive. `Const` stays the canonical form regardless, because it puts the
identity inside the term where regression can unify an effect (`greeted(Const(k))`)
against a goal directly, rather than carrying it as a separate `eq` conjunct, and
because it binds in O(1) instead of enumerate-then-filter. The filter spelling
earns its place only for **dynamic identity**: when the target id is *computed from
world state at bind time* (the being whose id sits in a quest component), there is
no literal to put in a `Const`, so it becomes a content filter. Static/literal
identity is `Const`; computed identity is a filter; a literal set is a
`member`-filter (one `Const` cannot hold a set).

Sets follow from the same pieces, two ways:

- **Any-of** (one slot, any member of a set): a shared `tag` when the members
  share a property (`tag(x, GuildMaster)`), or a `member(x, [ids])` **content
  filter** (the non-chainable bucket above) for an ad hoc set with no shared
  property. Not a new primitive.
- **All-of** (a goal spanning several specific entities, "gather these three"): a
  conjunction of `Const`-pinned clauses, `holds(self, a) ∧ holds(self, b) ∧
  holds(self, c)`. Already the clause machinery, each term non-fungible.

This lifts the planner from propositional to variable-carrying (unification on
regression, constrained enumeration on binding), which is real machinery, heavier
than FEAR's ground matching. That cost lands in the planner, deferred. The
*shape* it forces, terms and conjunctions instead of named predicates, is what to
fix now.

Two things are *not* planner preconditions and belong to other layers:

- **Need-state** (`hunger(a) > k`, `hurt(a)`, `afraid(a)`) reads the NPC's own
  components and feeds *drives*, not the planner. It is why the drive layer never
  needs an agent's knowledge (`Known` edges): how-I-feel is always known,
  where-the-food-is is not.
- **The real rule** (`can_traverse`, the reachability check in the `take`
  handler) is truth and re-checks at execution. `reachable(a, i)` in the table
  above is the planner's *approximation* of it. When the real rule grows a veto
  the approximation lags, and that is safe: a plan built on a stale precondition
  just gets vetoed at the beat and replans. World-model is a derived planning
  hint; the rule helper is truth. Same shape as "reactions respond, they do not
  veto" and "forward link is truth, index is derived."

## The veto lives once, in the handler

The checks that decide whether a mutation may happen must not be duplicated into
the planner. They live in one place, the handler's pre-commit validate phase (the
only veto point; see [../actions.md](../actions.md)), keyed on the **actor and the
world**, never on the input channel. Because an agent's plan executes as a `Steps`
sequence through the same verb helpers a player command hits, every gameplay veto
(`can_traverse`, reachability, takeable) runs exactly once and vetoes a planned
NPC identically to a player. This is the existing `do_move` design, extended: it is
already why a scripted mover is stopped by a locked door.

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

## Argument binding: the thin-frame payoff and its limit

FEAR gets away with propositional (argument-free) actions because its object set
is tiny and in view. A MUD has indirect objects, so a clause like `holds(self, x)
∧ tag(x, Food)` carries a *variable* `x` constrained by a tag, not an entity.
Fully lifted backward planning is HTN-hard. The pragmatic line, enabled by the
thin frame and the constraint form:

- **Bind lazily against beliefs.** When regressing that clause, enumerate the
  perceived entities that satisfy the constraint (`tag(x, Food)`) and branch one
  grounded plan per candidate. The arity stays `object + target + kind`; only
  *when* binding happens moves, from eager-against-text to lazy-against-beliefs.
  The enumeration primitive itself (`bind_var`, filtering candidates through
  `WorldModel`) is built; the regression that drives it is step 4.
- **An empty candidate set is the signal, not a failure.** If the agent has no
  `Known` edge to an entity satisfying `tag(x, Food)`, the clause regresses to a
  `search(Food)` action whose effect is `related(self, x, Known) ∧ tag(x, Food)`.
  "I do not have one yet" is exactly what generates the find step. This is why
  knowledge-as-a-relation is load-bearing rather than nice-to-have: acquiring it
  is a first-class action the planner can chain toward.

## Open questions

Deliberately unresolved; listed so they are not mistaken for decided.

- **Predicate representation.** *Decided.* Two chainable primitive kinds
  (`related` / `tag`) over terms (`Const | Var`), clauses as conjunctions, built in
  the engine (`musce_action`). Parameters (relation kinds, component tags) and `Var` names are
  `String`, matching the executor's `Action`. This is no longer migration-class:
  plans lower to structural `Action`s rather than serialize into `Intent` (see the
  README build order), so the predicate types are never persisted, and interning
  them to `Copy` symbol ids is a pure internal optimization, deferred and worth
  doing engine-wide (the same `String` kinds ride `Action`). Content filters (the
  non-chainable `PredicateRegistry` bucket) stay unbuilt until a verb first tests
  component *content* rather than presence.
- **Communicative affordances.** Full table entries with empty world-effects, or a
  lighter "goal-satisfaction hook" that does not participate in regression the way
  world-changing actions do? Still open.
- **Cost model.** *Seam decided; policy open.* Cost is not a field on the
  affordance but a game-supplied `CostModel::cost(actor, affordance, world)`
  (built; the generic crate ships only `UnitCost`). Whether a game's model is flat
  per-affordance or scaled by distance/effort at bind time is still open, as is
  where soft content preferences land (grade the pencil by uses remaining rather
  than filter on it; see "Gate or grade"). Per-actor *learned* cost is build step 6
  over this same seam.
- **Predicate evaluation.** *Seam decided.* Whether a predicate holds against the
  world is a game-supplied `WorldModel::holds(predicate, world)` (built; no generic
  default, since only the game reads its own relation/component names), the
  read-side twin of `CostModel`. It evaluates *ground* predicates; binding a free
  `Var` by enumerating candidates is the separate planner primitive of build step 3
  ("bind lazily against beliefs" above), not this seam.
- **Derived-effect extraction.** *Decided: declared.* The `Affordance` carries an
  explicit `effect: Clause`, and the by-effect index reads that, keeping the
  executor's internals out of the mechanism. Auto-projecting the effect off the
  `Action` a verb commits is a possible later refinement; it is elegant but couples
  the index to executor internals, and may never be worth it.
