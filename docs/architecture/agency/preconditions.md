# Conditions, Terms, and Binding

> Status: **target design specified; implementation pending.** Planning
> conditions constrain functional relation and locus slots, component presence,
> existence, and registered qualitative gauge regions. Affordances distinguish
> typed inputs from results.

The planner must compare goals, preconditions, and effects statically. A predicate
is therefore not an arbitrary app callback. It is a condition in a closed state
algebra with a known matching effect.

## Terms

```text
Term = Actor
     | Input(ParameterId)
     | Result(ParameterId)
     | Local(LocalId)
     | Constant(Value)
```

- `Actor` is fixed to the planning agent.
- An `Input` is supplied before execution and may occur in conditions and effects.
- A `Result` is produced by execution and may occur only in effects and outcome
  narration.
- A `Local` is existentially quantified within a formula.
- A `Constant` pins a specific typed value.

Parameter ids are action-local. Their authoring names improve readability but do
not participate in unification; renaming `item` to `picture` changes no semantics.

## Conditions

The planner-visible state slots and their condition forms are:

```text
RelationTarget(source, relation): Option<Entity>
    == target | == None | != target
ComponentPresent(entity, component): bool
LocusOf(entity): Option<Entity>
    == locus | == None | != locus
GaugeRegion(entity, gauge): ordered registered region
Exists(entity): bool
```

Surface `source.relation_is(relation, target)` is equality on `RelationTarget`;
its negation is inequality. Relations are source-functional, so setting one
target implicitly displaces the previous target. Surface `AtLocus` is locus
equality; locus absence and inequality are equally available. All three are
evaluated by the engine's transitive `enclosing_locus` query rather than a stored
`LocatedIn` relation.

A formula is initially a conjunction with explicit local existential
quantification. Disjunction is not part of the foundational representation. A
shared domain disjunction is reified as categorical state rather than duplicated
across many affordances.

Relation, component, gauge, and qualitative-region ids are app vocabulary
registered with the engine. The slot and constraint variants themselves are
engine vocabulary. Adding `Food`, `Locked`, `MountedOn`, or `Affinity` adds an id,
not a new condition kind.

## Comparable effects

Effects update those same slots:

```text
RelationTarget ↔ SetRelation / ClearRelation
ComponentPresent ↔ SetComponent / RemoveComponent
LocusOf        ↔ SetLocus / ClearLocus
GaugeRegion    ↔ ShiftGauge
Exists         ↔ Create / Destroy
```

The planner first compares slot identity, then unifies corresponding terms and
checks whether the assignment satisfies the constraint. Assignments to the same
relation source and kind interfere when their targets differ; clearing the slot
interferes with every target equality. It never matches prose names or invokes an
opaque predicate implementation.

`SetLocus(root, locus)` and `ClearLocus(root)` assign the derived locus slots of
the root and all of its post-transition containment descendants. Regression
expands that closure through finite chains of ordinary `ContainedBy` slot
constraints. A live chain may satisfy those constraints, and earlier plan steps
may establish one; the current containment tree is not assumed to remain fixed.

For gauges, only registered qualitative one-sided targets participate in
regression. A directional effect promises strict progress in the required
orientation, not a magnitude or exact landing.

## Identity through substitution

Repeated use of a term enforces identity:

```text
requires:
    HasComponent(target, Picture)
    RelationTargetIs(target, ControlledBy, Actor)

effects:
    Destroy(target)
```

Regressing `¬Exists(Photo42)` through `Destroy(target)` yields the substitution
`target = Photo42`. Applying it to the requirements asks about `Photo42`
everywhere. A fact about another picture cannot satisfy the branch.

Substitutions are sort-checked. An `Entity` parameter cannot unify with `Text`,
and a relation position accepts only entity-valued terms.

Different variables may denote the same value unless the formula explicitly
requires distinctness. A distinctness constraint, when needed, is a logical
built-in rather than a semantic participant role. A result bound by `Create` is
intrinsically fresh and therefore distinct from every live input, constant, and
previously produced result.

## Inputs, results, and local witnesses

Inputs define a grounded action's supplied values. Locals support internal joins:

```text
exists locus:
    AtLocus(Actor, locus)
    AtLocus(item, locus)
```

The common locus is evidence for the condition but need not appear in the
grounded action. If the handler needs its identity, it is declared as an input.

A positive `Exists(input)` requirement is rejected as a tautology because entity
input grounding already requires a live entity. Existence remains available for
goals, result freshness, negative conditions, and plan revalidation.

Effects may refer to the actor, inputs, results, and constants. A local needed by
an effect is promoted to an input unless successful execution produces a fresh
value, in which case it is a result. `Create(result)` introduces a fresh entity
and binds that result. Result-aware regression is deferred, so no effect
containing a result term is currently eligible as a reverse-index achiever. Those
effects remain visible to interference analysis, and results remain distinct from
grounded inputs in schemas, outcomes, and wire projections.

## Candidate binding

Effect-goal unification usually grounds the parameters that matter to the desired
state. Remaining entity parameters are solved from the affordance's positive
conditions against the actor's known candidate set.

For:

```text
eat(food: Entity)

requires:
    RelationTargetIs(food, ContainedBy, Actor)
    HasComponent(food, Edible)

effects:
    ShiftGauge(Actor, Hunger, Down)
    Destroy(food)
```

a hunger goal binds only `Actor` and the gauge. Candidate solving enumerates known
entities for `food`, filters them through `Edible`, and leaves the containment
condition available for regression through `take`.

Candidate domains are sort-specific:

- entities come from the actor's knowledge/perception model;
- finite symbols come from a declared finite domain;
- text and other non-enumerable values must be supplied as constants.

An empty candidate set means no known grounding exists. The planner does not scan
the whole world and does not invent non-enumerable values.

## Structural facts, not semantic callbacks

The planning projection distinguishes facts precisely:

```text
AtLocus(thing, locus)
RelationTargetIs(thing, ContainedBy, Actor)
RelationTargetIs(thing, ControlledBy, Actor)
```

Co-location, possession, and control are not interchangeable. An affordance names
the relation it actually requires.

A convenience such as `available(actor, thing)` or
`feels_really_fondly_about(a, b)` is not a chainable atom. It must either:

- lower to a formula over registered relations, components, existence, and
  gauges or the engine-owned locus projection;
- become categorical or ordered world state in that algebra; or
- make the act opaque and non-plannable, or remain a cost input when it is soft.

This boundary keeps every planner condition regressible. Queryability alone is
not enough.

## Component values

Component presence expresses categorical state. Arbitrary component content is
not exposed as a generic field/operator/value predicate:

- A category that actions establish or remove becomes a component-presence or
  relation fact.
- An ordered quantity becomes a gauge.
- A hard calculation outside the algebra makes the affordance `Opaque` and
  therefore unavailable as a planner edge.
- A soft choice between valid groundings belongs in `CostModel`.

This prevents the planner from accumulating bespoke comparisons that no effect can
produce.

## Knowledge

Binding is constrained by what the actor knows. Knowledge is a boundary on
candidate enumeration rather than permission to query arbitrary world entities.
If observations are world entities, their owner and subject are separate
source-functional relation slots; a single generic many-to-many `Known` edge is
not assumed.

The planner reads current truth about known candidates. A richer belief model may
later store observations that diverge from truth, without changing the
condition/effect algebra.

## Relation to the other docs

- [../affordances.md](../affordances.md): typed signatures, grounding, guards,
  and the complete engine representation.
- [affordances.md](affordances.md): concrete action schemas and execution.
- [planner.md](planner.md): regression and search over these forms.
- [../gauges.md](../gauges.md): the quantitative state member of the algebra.
