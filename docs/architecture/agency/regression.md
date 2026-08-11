# Regression Semantics

> Status: **foundational categorical regression built; advanced rules pending.**
> Direct relation/component/locus/existence assignment achievement is built.
> Full side-effect interference, inequality achievement, qualitative gauges,
> descendant-aware locus validation, and fresh-result regression remain pending.

The planner first selects candidate affordances through the reverse effect index
described in [planner.md](planner.md). It then applies the assignment and derived
state rules here. These rules determine both whether an effect can establish a
condition and whether a side effect interferes with another required condition.

## State-slot assignments and interference

Conditions regress through assignments to the same state slot:

```text
RelationTarget == target  through SetRelation
RelationTarget == None    through ClearRelation
RelationTarget != target  through ClearRelation or a provably different SetRelation
ComponentPresent          through SetComponent / RemoveComponent
LocusOf == locus           through SetLocus
LocusOf == None            through ClearLocus
LocusOf != locus           through ClearLocus or a provably different SetLocus
Exists / ¬Exists          through Create / Destroy
```

The slot identity for a relation is `(source, RelationId)`, not a three-term set
member. Setting a target implicitly removes the old target. Two effects that assign
different targets to a unifiable relation slot are mutually exclusive; clearing a
slot interferes with every positive target constraint. The same assignment logic
rejects contradictory component, locus, existence, and gauge effects.

`SetLocus(root, locus)` and `ClearLocus(root)` are bulk assignments over the
derived locus slots of `root` and its post-transition containment descendants.
Achievement and interference use that same closure; movement cannot silently
invalidate a descendant's locus condition.

Execution still rechecks every step because another actor or a contested outcome
can invalidate a sound symbolic plan. A deterministic handler may not add an
undeclared applicability veto.

Entity inputs are live by grounding, so a positive `Exists(input)` requirement is
rejected as a tautology. `Create(result)` binds a fresh step result that later
effects and steps may reference. The schema, `Step`, outcome, offers, and wire
reserve that shape now; every result-bearing effect stays out of the reverse index
until fresh-result unification is implemented.

## Gauge regression

A planner gauge goal is a registered qualitative threshold:

```text
GaugeAtMost(Actor, Hunger, Sated)
```

The current reading determines the required direction:

```text
on satisfying side -> satisfied
on other side      -> required Up or Down
```

Only effects on the same entity term and `GaugeId` in the required direction are
candidate predecessors. A directional effect guarantees crossing at least one
registered region boundary in the required direction, so the number of
intervening regions bounds repeated applications. Regression conservatively
advances one region per application. Raw singleton levels and bounded interior
bands are handler-query facilities, not canonical planner conditions, guards,
goals, or effects: a direction-only effect may skip over them. The QSIM model
reasons about registered qualitative regions and orientation, never generic
numeric distance.

## Movement and derived location

Movement must expose planner-comparable location facts. A callback such as
`same_locus(actor, item)` is not sufficient because regression cannot identify the
effect that establishes it.

The planning projection therefore exposes the engine-owned derived functional
slot `LocusOf(entity)`, evaluated by `enclosing_locus`. A shared-locus requirement
is a join:

```text
exists locus:
    AtLocus(Actor, locus)
    AtLocus(item, locus)
```

`go(exit, destination)` advertises `SetLocus(Actor, destination)`. The app derives
`destination` from the exit while grounding the action; it is an ordinary input by
the time a step is recorded. No `LocatedIn` edge is stored or maintained, and
nesting depth does not affect the query.

`SetLocus(root, destination)` guarantees the destination for `root` and every
entity transitively contained by it after the affordance's relation assignments.
`ClearLocus(root)` likewise guarantees an unset locus for that subtree. Regression
of a descendant locus goal cannot rely only on the containment tree currently in
the world. It constructs one successor branch per finite containment witness from
the goal entity to `root`; each witness is a conjunction of ordinary
`RelationTarget == parent` constraints. The zero-length witness covers
`goal_entity == root`, while longer witnesses can themselves regress through
`take`, `put`, and other containment assignments. Same-step relation assignments
are applied before the remaining witness links are regressed. The actor's finite,
knowledge-scoped entity domain bounds witness enumeration.

The live containment tree can satisfy a witness, but is not treated as immutable.
This lets `take coin; go hall` establish `AtLocus(coin, hall)` and lets interference
analysis see that walking away falsifies a carried coin's old locus.

## Relation to the other docs

- [planner.md](planner.md): search structure, reverse indexing, binding, and
  termination.
- [preconditions.md](preconditions.md): terms, conditions, and candidate binding.
- [../affordances.md](../affordances.md): canonical slot and effect vocabulary.
- [../gauges.md](../gauges.md): qualitative regions and directional guarantees.
- [execution.md](execution.md): live-state revalidation and replanning.
