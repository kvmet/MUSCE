# The Planner

> Status: **target design specified; implementation pending.** The planner is a
> bounded backward-regression search over typed affordance inputs/results and the
> closed functional state-slot algebra.

The built prototype already performs bounded backward regression over add-only
`Related`/`Tag` predicates and fixed frames, with one free variable at most. It
returns no plan at depth 8 or after 10,000 settled nodes and logs when either bound
causes pruning, so resource exhaustion is distinguishable from an ordinary
unreachable goal in operations. The representation and regression rules below are
its pending replacement.

The planner finds a minimum-cost sequence of affordance steps whose contracted
successful execution makes a goal condition hold. It manipulates the same canonical
representation used by player verbs, pointing clients, and scripts, while real
execution remains the correctness authority.

## Backward regression

A search node is a conjunction of conditions that must become true. Expansion:

1. selects an unsatisfied condition;
2. looks up affordances with a statically comparable effect;
3. unifies that effect with the selected condition;
4. applies the resulting substitution throughout the affordance;
5. solves or regresses its requirements;
6. records the resulting grounded step.

The search succeeds when every remaining ground condition holds in the current
world.

Backward regression fits the engine because it does not require a mutable
hypothetical ECS world. Search manipulates symbolic conditions and substitutions,
then consults the real world only to test grounded conditions and enumerate known
candidates.

## Effect indexes and static matching

The affordance table carries a reverse index by effect shape:

```text
SetRelation / ClearRelation   keyed by RelationId
SetComponent / RemoveComponent keyed by ComponentId
SetLocus / ClearLocus          keyed by derived locus slot
ShiftGauge                     keyed by GaugeId and direction
Create / Destroy               keyed by existence direction
```

Lookup narrows the candidate affordances; unification then compares their terms.
There are no opaque semantic predicate names. The `Create` index shape is reserved
but remains inactive until fresh-result regression is implemented. More generally,
an effect containing any `Result` term is excluded from the reverse index during
this phase. This prevents a fresh result from unifying with an entity that already
exists merely because another effect mentions the result.

The exclusion applies only to selecting achievers. Result-bearing effects still
participate in interference analysis when their affordance is selected through a
result-free effect. Freshness can prove, for example, that assigning an existing
source's relation slot to a result falsifies equality with every live constant even
though the result's identity is not known yet.

For example, the goal:

```text
¬Exists(Photo42)
```

matches:

```text
delete(target: Entity)
effects: Destroy(target)
```

and yields `target = Photo42`. The bound requirements are consequently:

```text
HasComponent(Photo42, Picture)
RelationTargetIs(Photo42, ControlledBy, Actor)
```

An unrelated controlled picture cannot satisfy the branch.

## Terms and substitutions

The planning agent grounds `Actor`. Affordance inputs begin unbound unless a goal,
command, script, or caller pins them. Results are step-local symbols produced only
by successful execution. Locals are existential variables scoped to a formula.

Unification is typed:

- constants must be equal;
- a free input or local may bind consistently at its permitted scope;
- a previously bound variable must agree with its existing value;
- `Actor` must agree with the planning agent;
- incompatible sorts fail.

Result symbols do not unify with goal constants while fresh-result regression is
inactive. The later extension may bind them only as fresh step results and
`ResultRef`s, never as identities already live before their producing step.

Parameter names are irrelevant to matching. They are local authoring labels;
`ParameterId` and the substitution carry identity.

Every recorded `Step` has ground inputs or references to results of earlier steps:

```rust
struct Step {
    affordance: AffordanceId,
    actor: EntityId,
    inputs: Box<[ValueOrResultRef]>,
}
```

The foundational planner emits only ground `Value`s because `Create` regression is
inactive; `ResultRef` reserves the decided extension without reshaping `Step`.
Plans are transient runtime output and are not persisted as scripted intents.

## Binding parameters not fixed by the goal

An effect often leaves some parameters free. A hunger goal may select an `eat`
affordance by its gauge effect without identifying food:

```text
eat(food: Entity)

requires:
    RelationTargetIs(food, ContainedBy, Actor)
    HasComponent(food, Edible)

effects:
    ShiftGauge(Actor, Hunger, Down)
    Destroy(food)
```

The planner binds `food` from the actor's known entity set. Conditions whose state
dimension no affordance can establish act as candidate filters; achievable
conditions remain in the regressed subgoal. Thus `HasComponent(food, Edible)`
filters candidates while `RelationTargetIs(food, ContainedBy, Actor)` can regress through
`take`.

Binding several free parameters forms the constrained product of their candidate
sets. Search remains lazy: it produces one compatible substitution per successor
instead of materializing every action/entity combination.

Non-enumerable input values are never invented. An unbound `Text` input makes that
grounding unavailable unless a caller supplied it. Finite symbol domains must be
declared explicitly.

## Existential goals

A goal may contain locals:

```text
exists food:
    RelationTargetIs(food, ContainedBy, Actor)
    HasComponent(food, Edible)
```

Candidate binding grounds the local against known entities, retaining the cheapest
successful plan across groundings. Static conditions filter candidates; achievable
conditions are left for regression.

Identity-specific goals use constants:

```text
RelationTargetIs(Photo42, ContainedBy, Actor)
```

The two cases share the same substitution machinery. Fungibility is a free
variable; identity is a constant.

## Regression semantics

Functional assignment, interference, QSIM, and descendant-aware locus regression
are specified in [regression.md](regression.md). Those rules apply after
effect-goal unification and use only the closed state algebra.

## Cost and search order

Search is uniform-cost initially. `CostModel` receives the actor, affordance,
partial or complete grounding, and world:

```text
cost(actor, affordance, grounding, world)
```

This supports actor-specific preferences and input-sensitive choices without
placing cost in the affordance schema. A heuristic may be added only if it is
admissible for the mixed boolean/QSIM state model.

## Termination

The search is total:

- a visited set keys nodes by a normalized formula plus substitution;
- a depth bound caps plan length;
- a settled-node budget caps pathological branching;
- gauge regression has a repetition bound;
- candidate enumeration is finite and knowledge-scoped.

It returns a plan or `None`, never an unbounded search.

## Execution and replanning

The planner proposes steps whose inputs are ground when their turn arrives. The
driver executes each through the app's shared affordance implementation and binds
its results for dependent steps. A stale or destroyed input fails live-state
revalidation and triggers replanning before applicability is established. A
deterministic refusal after live inputs, an admitting gate, and true guards is a
contract violation. A contested failure excludes that exact `(affordance, actor,
inputs)` grounding for the current search and replans from live world state.
Opaque affordances never enter the effect index.

Committed earlier effects remain in the world, so replanning begins from the
actual partial result. A different grounding of the same affordance remains
available.

## Falsifiability

Each concrete affordance needs an executable oracle:

1. ground live inputs across representative applicable states;
2. prove refusal when a declared guard is false;
3. for a deterministic affordance, prove that admitting gate plus true guards
   commits rather than refusing;
4. execute the real handler and validate all returned results;
5. verify every advertised slot assignment, including the derived locus of the
   moved root and every post-commit descendant for `SetLocus` or `ClearLocus`;
6. verify movement to a strictly different registered region in each advertised
   gauge direction;
7. verify that effects advertised as unconditional hold for every successful
   outcome.

Structural executor failures in an applicable deterministic case fail the oracle.
Registration can validate schema contradictions, but it cannot prove arbitrary
Rust handler behavior; the oracle is the behavioral half of the contract.

Planner tests then run generated plans through those same handlers and assert the
goal condition in the resulting world. Comparing against a hand-written plan is
insufficient because several correct plans may exist.

## Relation to the other docs

- [preconditions.md](preconditions.md): the condition algebra, term scopes, and
  candidate binding.
- [regression.md](regression.md): assignment, gauge, and derived-locus regression.
- [affordances.md](affordances.md): concrete schemas and grounded execution.
- [execution.md](execution.md): result-aware execution and contested replanning.
- [../gauges.md](../gauges.md): QSIM readings, targets, and directions.
- [../actions.md](../actions.md): the structural executor below successful acts.
