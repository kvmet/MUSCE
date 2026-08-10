# Gauges

> Status: **value vocabulary, evaluator registration, and qualitative conditions
> built.** `GaugeId`, `GaugeLevel`, `GaugeDirection`, and `GaugeTarget` are built;
> `StateRegistry` registers point evaluators and total ordered `GaugeRegion`s, maps
> readings into regions, and evaluates canonical one-sided thresholds. Directional
> effect contracts, executable oracles, and planner integration remain pending.

Component presence and relations describe categorical facts. They do not describe
an ordered quantity such as health, hunger, temperature, or affinity without
exposing app-specific numeric representations. A gauge is a named, read-only,
normalized projection of such state.

## A reading, not stored truth

Stored components and relations are authoritative. A gauge evaluator reads that
truth and returns a qualitative position:

```text
read(entity, GaugeId) -> Option<GaugeLevel>
```

A gauge has no independently settable storage. Changing it means executing an
action that mutates the backing world state. Most evaluators will read one
component, but they may combine several. `None` means the gauge does not apply to
that entity.

The point read is the execution and evaluation primitive. Batch evaluation or
indexing may optimize it later without changing its meaning.

## Common ordered space

`GaugeLevel` is a newtype over the complete `u8` range:

```rust
GaugeLevel::MIN
GaugeLevel::new(127)
GaugeLevel::MAX
```

The value is normalized and ordinal:

- both endpoints are exact saturation;
- every bit pattern is valid;
- higher and lower are meaningful;
- generic numeric distance is not meaningful.

An app maps its backing representation monotonically into this space. The engine
does not infer units, linear effort, or valence.

The raw level is not planner-authored domain language. Its finite order provides
evaluation and a termination measure; it does not make 256 universally meaningful
states or permit the planner to treat numeric distance as cost.

## Registered qualitative regions

Each gauge registers an ordered domain vocabulary over the raw space. For example:

```text
Hunger: Sated < Peckish < Hungry < Starving
Health: Critical < Hurt < Sound
```

Each region id maps to a non-overlapping raw interval, in total order, covering
the gauge's applicable range. Region names are app vocabulary; ordering and
threshold comparison are engine mechanics. Goals and planner guards use these
registered ids rather than arbitrary `u8` constants.

The same name on two gauges implies no shared scale. Comparing the hunger of two
entities, or comparing Hunger with Health, is not a planner operation.

An ordered amount belongs on the entity whose state owns it. Hunger is a gauge on
the actor; a fungible balance may be a gauge on an account or purse. Proxy entities
such as one coin per unit are used only when individual identity and transfer are
gameplay, not merely to make a numeric amount visible to predicates.

## Runtime targets and planner thresholds

The raw `GaugeTarget` serves handler calculations and imperative queries:

```rust
GaugeTarget::at(level)
GaugeTarget::at_least(low)
GaugeTarget::at_most(high)
GaugeTarget::between(low, high)
GaugeTarget::MIN
GaugeTarget::MAX
```

Comparing a reading with a target yields the direction required to approach it:

```text
current < target.min  -> Up
current in target     -> satisfied
current > target.max  -> Down
```

`GaugeDirection::{Up, Down}` is raw orientation. It does not mean improve or
worsen. A healer may drive health up; a poisoner may drive it down. App goals
choose desirable targets.

Raw targets are not canonical planner conditions. They cannot appear in
affordance `requires` or `effects`, planner goals, or the reverse index. A
deterministic handler may query one while calculating its guaranteed transition,
but cannot use it as an undeclared refusal rule; a commitment decision that cannot
be expressed through registered qualitative guards makes the affordance opaque.

Planner-regressible targets are one-sided qualitative thresholds:

```text
GaugeAtLeast(entity, gauge, region)
GaugeAtMost(entity, gauge, region)
```

`GaugeAtLeast(entity, gauge, region)` is true when the reading projects into that
region itself or any higher registered region. `GaugeAtMost` is symmetrically
inclusive of its named region. An exact raw interior point or bounded interior
band remains queryable by handler code but is not a planner condition: one
strictly monotone action may skip over it. A saturation endpoint is planner-visible
only through a registered one-sided region containing that endpoint.

There is no generic `Stable` effect. No change is the absence of an immediate
effect; active maintenance requires duration and counter-effect semantics.

## The condition/effect dual

Gauges enter the planner's closed state algebra as two different types:

```text
condition:
    GaugeAtLeast(entity, gauge, region)
    GaugeAtMost(entity, gauge, region)

effect:
    ShiftGauge(entity, gauge, direction)
```

Threshold conditions are truth-valued in one world snapshot. `ShiftGauge`
describes a transition and is not itself a predicate.

Static comparison requires:

1. unifiable entity terms;
2. the same `GaugeId`;
3. an effect direction equal to the threshold's required change.

An effect promises a strict qualitative-region change in its direction, not a raw
magnitude or exact landing. A successful `Up` commit must end in a higher registered
region; a successful `Down` commit must end in a lower one. A raw change that stays
inside one region is not a planner-visible `ShiftGauge` effect.

The finite registered order provides the bound: each successful shift crosses at
least one region boundary, so no more than the number of intervening regions is
needed to reach a one-sided target. The planner may conservatively regress one
region per application; an action that jumps farther only reaches the target
earlier. Raw numeric distance is never interpreted as effort, utility, or plan
length.

This is qualitative simulation: it reasons about ordered regions and directional
change without pretending to know an app-specific transition function.

## Why not named qualitative predicates

A predicate such as `VeryHungry(actor)` or
`FeelsReallyFondlyAbout(a, b)` is queryable but not general planning structure.
Every such predicate would require an effect with the same bespoke name.

The gauge form separates domain vocabulary from planning mechanics:

```text
GaugeAtMost(actor, Hunger, Sated)
ShiftGauge(actor, Hunger, Down)
```

or:

```text
GaugeAtLeast(attitude, Affinity, Fond)
ShiftGauge(attitude, Affinity, Up)
```

`Hunger` and `Affinity` are registered gauge ids; `Sated` and `Fond` are regions
registered within their respective gauges. Threshold conditions and `ShiftGauge`
remain the only planning forms.

## Quantities about relationships

Gauges are addressed by one entity. Quantitative state about a pair is represented
by reifying the relationship:

```text
RelationTargetIs(attitude, Owner, Alice)
RelationTargetIs(attitude, Toward, Bob)
GaugeAtLeast(attitude, Affinity, Fond)
```

The attitude entity may carry the backing component for affinity and may expose
other gauges such as trust or fear. This follows the relation layer's general
approach to richer many-to-many state: the relationship becomes an entity with
ordinary relations and components.

An action that praises Bob grounds the same `attitude` parameter and advertises:

```text
ShiftGauge(attitude, Affinity, Up)
```

Term identity ensures the action changes the attitude entity named by the goal.

## Component values and thresholds

The planner does not expose arbitrary component fields or raw comparisons:

- categorical state is a component-presence or relation condition;
- ordered planner state is a registered qualitative gauge threshold;
- a hard rule outside this algebra makes an affordance opaque and non-plannable;
- a soft preference is a cost input.

Named regions are registered data over `GaugeLevel`, not new engine condition
variants. Raw singleton and band targets remain outside the canonical `Formula`
and `Effect` types and may be queried only by handler code.

## Evaluation and registration

The app registers each `GaugeId` with:

- its name;
- the entity kinds for which it applies, if useful for validation;
- a point evaluator;
- its ordered, non-overlapping qualitative regions and raw boundaries;
- tests proving monotonic projection from backing state.

Registration gives conditions and effects a statically comparable id while the
evaluator remains app code. An unknown gauge id is a schema error, not a false
reading.

The built `StateRegistry::register_gauge` rejects empty, overlapping, gapped, or
duplicate region declarations and requires a total cover from `GaugeLevel::MIN` to
`GaugeLevel::MAX`. A registered evaluator returning `None` means the gauge does not
apply to that entity; an unknown gauge or region is an evaluation error.

## Planner and execution obligations

- Regression compares only matching gauge ids and terms.
- The current region and threshold select the required direction.
- A committed directional effect moves to a strictly different qualitative region
  in that direction.
- Search bounds repetition by the finite registered region order and its own lower
  budgets.
- A grounded step rechecks any gauge guard before committing.
- After execution, an oracle verifies that the registered region moved strictly in
  the advertised direction.
- Staying in the same region or moving oppositely is metadata drift and fails the
  oracle.

## Relation to the other docs

- [affordances.md](affordances.md): the complete condition/effect algebra and
  typed term representation.
- [agency/preconditions.md](agency/preconditions.md): static matching and
  substitution.
- [agency/planner.md](agency/planner.md): bounded QSIM regression.
- [ecs-and-relations.md](ecs-and-relations.md): stored truth and reified
  relationships.
