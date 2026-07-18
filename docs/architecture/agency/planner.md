# The Planner

> Status: **built (agency build step 4).** Backward goal-regression over the
> affordance table, with ground goals (4a) and existential goal binding (4b),
> lives in `musce_agency` (`planner.rs`) as `Planner` / `plan` / `Plan` / `Step`.
> `musce_ref` carries the executable oracle: a regressed plan, run through
> `perform`, makes the goal hold. The replan-on-veto loop that consumes
> `plan_excluding` is now built as the [execution driver](execution.md) (step 5).
> Still deferred within step 4: movement (`go`, see below). The arbiter and drives
> (step 5) and per-actor cost learning (step 6) sit on top.

The planner is the GOAP core: given a goal, it finds a minimum-cost sequence of
affordances whose execution makes the goal true. It reads the same affordance
vocabulary, `WorldModel`, and `Guard` clauses a player verb reads (see
[../affordances.md](../affordances.md)), so a planned action and a typed one
cannot disagree on what a verb permits.

## Backward regression, not forward simulation

The search works *backward* from the goal. A search node is a subgoal `Clause`
(the literals that must become true). Expansion picks an unsatisfied literal,
finds an affordance whose `effect` produces it, and replaces that literal with the
affordance's bound guard preconditions, recording the step. A node **succeeds**
when every literal in it already holds in the **actual current world**, tested
through the game's `WorldModel`.

The alternative, forward state-space search, was set aside because it needs a
*hypothetical* world model: to apply an affordance's effect forward you must
toggle predicates true in a simulated state and keep asking `holds` against that
overlay. The engine has no such abstraction; `WorldModel::holds` reads the real
`World`. Backward regression never simulates a world: it only ever asks whether a
ground literal holds *right now* (at the success test) and otherwise manipulates
symbolic literal sets. That is a clean fit for the seam we already have, and it is
why regression, not forward search, is the core.

## Uniform-cost search

Nodes are settled cheapest-first from a min-heap keyed by cumulative cost, and the
goal is tested on pop, so the first satisfied node yields an optimal plan
(Dijkstra). Cost comes only through the game's `CostModel`, never a field on the
affordance; with the trivial `UnitCost` this degenerates to minimum-length, and a
real cost model sharpens it with no change here. There is no heuristic yet (`h =
0`); adding an admissible one is a later refinement, not a shape change.

`plan` returns a transient `Plan` (a `Vec<Step>` of `{ affordance, frame }`).
Nothing is persisted: the game lowers each step through its grounded action
(`perform`), where the veto and the structural `Action` commit live, so no agency
type embeds in a serialized script (see [README](README.md) crate section).

## Existential goal binding: ground-first, via `bind_var`

A goal may be existential: `∃x. related(x, actor, contained_by) ∧ tag(x, food)`
("hold some food"). `plan` grounds it before searching:

1. **The actor role** is substituted to the planning agent, since a goal is
   written in the same role vocabulary (`actor`) as affordance clauses.
2. **A fungible slot** (`x`) is bound against the actor's `known` set (the game's
   knowledge seam). The goal's literals split into a *static* part (predicates no
   affordance can produce, e.g. `tag(x, food)`, so they must already hold) and an
   *achievable* part (predicates some affordance's effect shares a shape with,
   e.g. `related(x, actor, contained_by)`). Only the static part filters the
   candidates, through the shared `bind_var` primitive; the achievable part is
   left for regression to plan. Each surviving grounding is regressed and the
   cheapest plan wins.

This is ground-first, not lifted. Binding lazily inside the search would let the
planner invent objects it does not know, which is exactly the omniscience the
`Known` filter exists to prevent (see [README](README.md), "the planner needs no
world index"). The static/achievable split is principled, not a heuristic: a
literal is static precisely when *no* affordance can make it true, so filtering by
it can never hide a plan. An empty candidate set is a meaningful "nothing known
fits", not an error; enriching it with a find/search step is the deferred
perception layer.

The current verb set introduces no *mid-search* existential: every precondition of
`take`/`drop`/`put` names only frame roles that effect-unification already binds.
So `bind_var` is used only for the goal slot today. A future affordance whose
precondition carries a fresh variable (`cook` needing `∃y. tag(y, fuel)`) reuses
the same primitive during regression; multiple free vars in one goal is a
combinatorial product no current goal needs and is deferred.

## Unification is a small positional match

Grounding an affordance during regression is not general unification. The frame
has three fixed roles (`actor`, `object`, `target`), so matching an effect
predicate against a ground subgoal predicate is a positional check: same variant
and same relation kind / component, then each effect term is either a `Const` that
must equal the subgoal's entity or a role-var that binds (or must agree with) its
frame slot, with `actor` fixed to the planning agent. The result is the `Frame`
that grounds the step.

## Add-only effects, and where soundness actually lives

Effects are add-only: `take` declares that the object becomes held, not that it
leaves the room. Two consequences, both deliberate:

- **Only positive subgoal literals are regressed.** A negated literal (`take`'s
  `¬ fixture`) is checked by `holds` at the success test; if it is unsatisfied,
  the branch dead-ends, because no add-only effect makes a fact false. So negation
  needs no special regression handling, and the planner correctly cannot plan
  "through" a locked door or an un-take-able fixture.
- **No delete lists, so a plan can be interfered with in principle.** Backward
  regression without delete effects can emit a plan where one step falsifies a
  later step's precondition. The reference verb set has no such interference (a
  `take` *establishes* `put`'s held-precondition), but the general backstop is not
  the planner: it is execution's **replan-on-veto**. A vetoed beat means the world
  diverged from the plan's assumption; the executor replans from the new state.
  The planner is a proposer, not the correctness authority, exactly as the
  structural executor, not the guards, is the commit-time backstop (see
  [../actions.md](../actions.md)).

## Termination

The search is total: it returns a plan or `None`, never spins. A visited set keys
subgoals by an order-independent normal form and settles each once; a depth bound
caps plan length and a settled-node budget backstops a pathological table. For the
reference verbs (plans of one or two steps over a handful of affordances) neither
bound binds; they exist so the function is total by construction.

## Movement (`go`) is deferred, and why it is *forced*, not lazy

`go`'s effect is "the actor is now at the exit's destination", where the
destination is *derived* from the exit (`world.exit_destination`), not a frame
role. Modeling that needs a derived/functional term the vocabulary does not have.
It is deferred, and the reason is a hard constraint:

`Known` is co-located-only. An agent knows only the entities sharing its locus, so
`bind_var` can never surface an entity in another room as a candidate, so **no
cross-room goal is formable**, so nothing can require a `go` step. Building
`go`'s derived-location handling now would be a capability no test can drive,
which the falsifiability rule forbids; and a derived `Term` is a pure additive
change to unserialized vocabulary (cheap to retrofit), which the reversibility
gate says to defer regardless of likelihood. The MVP planner therefore plans
**within-room manipulation** (`take`/`drop`/`put` over co-located known entities),
which is exactly the world `known_here` describes. `go`'s derived-location
handling lands when perception / multi-room `Known` makes a cross-room goal
formable, because that is what would exercise it.

## The API, and the replan-loop seam

```rust
Planner::new(affordances, model, cost).plan(actor, goal, known, world) -> Option<Plan>
Planner::new(affordances, model, cost).plan_excluding(.., known, world, excluded) -> Option<Plan>
```

The static planning context (the affordance table and the game's read/cost
policies) lives in the `Planner`; the per-query inputs (actor, goal, known set,
world) are `plan` arguments. `world` is borrowed only for the call, so the caller
can take `&mut World` to execute the returned plan immediately after.

The **replan loop** is built (step 5) as the [execution driver](execution.md), and
it consumes `plan_excluding`: on a vetoed beat the driver adds the failed
`(affordance, frame)` to an exclusion set and replans, so the planner routes around
that step (a different binding stays available) or, if it was the only route,
returns `None`. `plan` is `plan_excluding` with an empty set, so the exclusion set
was the additive `plan` argument the context/query split kept cheap. Regression
skips a candidate step that matches an excluded entry (same affordance name, same
bound frame). Together with the internal termination bounds, that is the two-level
answer to "why doesn't it retry the same failed action forever": the search is
bounded, and the executor never re-issues an excluded step (see
[execution.md](execution.md)).

## Falsifiability

The generic planner is tested in `planner.rs` against a stub `WorldModel`
(regression chains, cost selection, existential binding, negation, no-plan). The
ground truth is the executable oracle in `musce_ref`: the planner's *own* output,
run through the same `perform` a player hits, makes the goal predicate true. That
is an independent check against world state, not a comparison to a hand-authored
plan (many chains satisfy a goal; the planner may pick a different correct one),
and it covers goals no hand-plan was written for.

## Relation to the other docs

- [README](README.md): the agency stack and build order this doc's step 4 sits in.
- [../affordances.md](../affordances.md): the vocabulary, guards, and the `veto`
  the planner reads for applicability and `perform` runs at execution.
- [preconditions.md](preconditions.md): the predicate vocabulary a goal and a
  precondition are written in, and how a variable binds against what an agent knows.
- [../actions.md](../actions.md): the structural executor that stays the
  commit-time backstop a plan's add-only effects deliberately do not replace.
