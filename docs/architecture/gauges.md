# Gauges

> Status: **foundational value types built; evaluation and agency integration
> proposed.** `musce_action` provides the bounded `GaugeLevel`, raw
> `GaugeDirection`, symbolic `GaugeId`, and inclusive `GaugeTarget` algebra. No
> gauge evaluator, registry, predicate/effect form, or planner behavior is built
> yet.

`tag(e, C)` asks whether an entity bears a component. That is enough for categorical
facts, but it cannot describe a quantity such as health without exposing the backing
component's representation to every consumer. Gauges provide the missing value
shape: a named, read-only measurement projected onto one common bounded ordering.

## A gauge is a reading, not a component

Stored state is truth. It lives in an ordinary component, participates in archetype
membership, persists, and may be changed through the world's validated mutation
paths. A gauge is a derived view over that truth. It has no per-entity storage or
presence of its own and cannot be set; changing a gauge means changing the components
its evaluator reads.

Most gauges will read one component, but that is not an invariant. A gauge evaluator
may read whatever app state defines the measurement. Reading a gauge for an entity to
which it does not apply will eventually return `None`. The point read is the semantic
primitive; a batched evaluator or derived index can be added later without changing
what the gauge means.

## The common value space

`GaugeLevel` is a newtype over the complete `u8` range:

```rust
GaugeLevel::MIN       // 0: lower saturation
GaugeLevel::new(127)  // an ordered interior reading
GaugeLevel::MAX       // 255: upper saturation
```

The byte is **normalized and ordinal**. Its endpoints are exact, every bit pattern is
valid, and interior values say only which reading is higher. The generic engine does
not assign them units, names, or a meaningful distance. In particular, it must not
assume that moving from 20 to 40 takes twice the effort of moving from 20 to 30.

The app keeps the exact value in its backing components and maps it monotonically into
the byte. A health component may map `0 .. 100` to `0 .. 255`; another gauge may
combine several inputs. A concept with an unbounded backing representation must choose
meaningful operational limits before exposing a gauge, because saturation and a
generic target direction require endpoints.

A byte is preferred to a normalized float: it has no NaN or infinity, endpoint and
threshold comparisons are exact, and its deliberately finite resolution avoids
claiming generic precision the planner cannot use.

## Bounds, targets, and direction

`GaugeLevel::MIN` and `GaugeLevel::MAX` are the two saturated readings. They can
eventually be point-readable predicates because an evaluator can answer them from one
world snapshot. They need no separate bound type: a bound is simply a distinguished
level, and the endpoint target constants provide the common goal form.

`GaugeTarget` is an inclusive interval `[min, max]`. Its constructors cover the useful
goal shapes without assigning universal names such as `LoLo` or `HiHi` to interior
levels:

```rust
GaugeTarget::at(level)
GaugeTarget::at_least(low)
GaugeTarget::at_most(high)
GaugeTarget::between(low, high)
GaugeTarget::MIN
GaugeTarget::MAX
```

A reading inside the interval satisfies the target. A reading below it requires
`GaugeDirection::Up`; one above it requires `GaugeDirection::Down`. This comparison is
implemented by `GaugeTarget::required_change` and is the whole target algebra:

```text
current < target.min  -> Up
current in target     -> satisfied (`None`)
current > target.max  -> Down
```

`Satisfied` is not a gauge level. It is the result of comparing a reading with a
particular target. Likewise, `Up` and `Down` are not predicates that hold at a point;
they describe requested or produced change. The types keep those categories separate:

- `GaugeLevel`: measured position;
- `GaugeTarget`: the acceptable interval for one goal;
- `GaugeDirection`: orientation needed to approach that interval.

There is deliberately no generic `Stable` direction. Producing no immediate change is
normally the absence of an effect; actively maintaining a quantity over time requires
duration and counter-effect semantics that the basic gauge vocabulary does not claim.

## Valence and app policy

Higher is not universally better. `GaugeDirection` is raw orientation, never
`Improve` or `Worsen`: a healer may drive a creature's health toward `Max`, while a
poisoner drives the same gauge toward `Min`. Named thresholds such as `LOW_HEALTH`,
alarm bands such as `LoLo`, and decisions about which target to pursue belong to app
goal logic. They may be constants over `GaugeLevel`; they are not engine-wide states.

This lets exact app measurements and qualitative action descriptions meet without
forcing either into the other's representation. Goal logic can read a concrete
component or a future gauge evaluator, construct a target, and obtain the required
direction. An affordance can eventually declare only that it moves the named gauge
`Up` or `Down`, without claiming a magnitude or number of applications.

## What is built

The non-optional `musce_action` crate currently owns only the inert value vocabulary:

```rust
GaugeId
GaugeLevel(u8)
GaugeDirection::{Down, Up}
GaugeTarget
```

`GaugeTarget` validates interval ordering, tests satisfaction with `contains`, and
derives an optional required direction. These operations read no world and mutate
nothing.

## What is not built

- **Evaluator and registry.** There is no `GaugeId -> read(entity)` routing yet.
- **Predicates and effects.** Bounds are not part of `Predicate`, and directions are
  not part of an affordance's effect vocabulary.
- **Planner integration.** The planner does not regress a target through directional
  effects.
- **App gauges.** No concrete health, hunger, or other gauge is registered.
- **Batching and indexing.** A point read will be the semantic primitive; bulk reads
  and derived indexes wait for a measured need.
- **Wire representation.** Gauge values and targets do not cross the protocol.

## Relation to the other docs

- [affordances.md](affordances.md): owns the current predicate/effect vocabulary that
  future gauge forms may extend.
- [agency/preconditions.md](agency/preconditions.md): owns the chainable-versus-filter
  split and the motivation for hiding exact app values from propositional planning.
- [ecs-and-relations.md](ecs-and-relations.md): owns components and the stored truth a
  gauge evaluator will read.
- [indexes.md](indexes.md): owns any future materialized acceleration structure;
  indexing would not turn the gauge itself into stored truth.
