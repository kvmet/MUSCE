# Preconditions and the Predicate Vocabulary

> Status: **partially built.** The predicate vocabulary (`Term`, `Predicate`,
> `Clause`) and the `WorldModel` evaluation seam are built in the engine
> (`musce_action`, non-optional; see [../affordances.md](../affordances.md));
> `bind_var` (candidate enumeration) is built on the planner side in
> `musce_agency`. The regression that drives binding, content filters (the
> `PredicateRegistry` bucket), and the cost policy remain proposed. This is the
> symbolic half of [affordances.md](affordances.md): what a precondition or goal
> is written in, why objects are identified by constraint rather than by id, and
> how a variable binds against what an agent knows.

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
plus no closed container between them; `open(c)` = `¬ tag(c, Locked)`. The `¬`
there is real and built: a clause is a conjunction of `Literal { negated,
predicate }`, and the engine evaluates the negation (`go`'s `¬ tag(exit, Locked)`
is the first negated guard), so `WorldModel` still answers only atomic questions.
So the relation-graph economy holds fully: the whole model is `related`/`tag`
queries against state that already exists. There is **no separate belief store**:
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
  (`related` / `tag`) over terms (`Const | Var`); a clause is a conjunction of
  `Literal { negated, predicate }`, so negation is engine-owned and the game's
  reading stays atomic; all built in the engine (`musce_action`). Parameters
  (relation kinds, component tags) and `Var` names are
  `String`, matching the executor's `Action`. This is no longer migration-class:
  plans lower to structural `Action`s rather than serialize into `Intent` (see the
  README build order), so the predicate types are never persisted, and interning
  them to `Copy` symbol ids is a pure internal optimization, deferred and worth
  doing engine-wide (the same `String` kinds ride `Action`). Content filters (the
  non-chainable `PredicateRegistry` bucket) stay unbuilt until a verb first tests
  component *content* rather than presence.
- **Predicate evaluation.** *Seam decided.* Whether a predicate holds against the
  world is a game-supplied `WorldModel::holds(predicate, world)` (built; no generic
  default, since only the game reads its own relation/component names), the
  read-side twin of `CostModel`. It evaluates *ground* predicates; binding a free
  `Var` by enumerating candidates is the separate planner primitive of build step 3
  ("bind lazily against beliefs" above), not this seam.
