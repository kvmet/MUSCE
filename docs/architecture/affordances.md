# Affordances, Conditions, and Effects

> Status: **target representation specified; implementation pending.** The
> engine-owned affordance vocabulary will use typed input/result parameters,
> functional state slots, grounded substitutions, explicit resolution contracts,
> and a Rust `affordance!` authoring macro. The optional `musce_agency` planner
> consumes this representation but does not own it.

An **affordance** describes an attempt an actor can make: the values that identify
the attempt, the world conditions under which it is applicable, the state changes
it advertises to planners, its authority requirement, and the app implementation
that validates and commits it.

The representation belongs in `musce_action`, not in the optional planner. Player
commands, pointing clients, scripts, and autonomous agents all need to ground and
validate the same act. GOAP is one consumer of that common representation.

## Actor and typed inputs/results

Every affordance has one privileged participant, `Actor`, and a fixed signature of
typed, action-local parameters:

```text
delete(target: Entity)
hang(item: Entity, support: Entity, fastener: Entity)
say(text: Text)
give(item: Entity, recipient: Entity)
go(exit: Entity, destination: Entity)
craft(material: Entity) -> (product: Entity)
```

`Actor` is supplied by the execution context. It is privileged because authority,
cost, agency, and first-person narration are all actor-relative. Parameters define
the rest of one action occurrence.

Input and result names are local identifiers, like Rust function parameter names.
Inputs are bound before execution. Results are produced by successful execution
and may be referenced by effects and later plan steps. Names
carry no engine-wide semantics: `target`, `item`, `recipient`, and `fastener` are
not members of a universal role taxonomy. Parameter order provides a compact
runtime index and a stable display order; the engine assigns no meaning such as
"argument zero is the patient."

The canonical representation keeps a name and sort:

```rust
struct Parameter {
    id: ParameterId,
    name: String,
    sort: ValueSort,
    mode: ParameterMode, // Input | Result
    slot: u16,           // compact within its mode
}
```

Conditions and effects refer to `Actor` or a parameter by id. A grounded action
stores only its bound inputs. Successful execution returns result bindings:

```rust
struct GroundAction {
    affordance: AffordanceId,
    actor: EntityId,
    inputs: Box<[Value]>,
}

struct ActionOutcome {
    results: Box<[Value]>,
}
```

`ParameterId` is the stable schema identity used by terms and the wire. `slot`
indexes the dense input or result array for its mode, so result declarations do not
leave holes in `GroundAction.inputs`.

An input may also be affected; no separate `InOut` mode is needed. A result is
absent from a perform request and from `Needs`, cannot appear in a requirement,
and must be produced on every successful execution. A plan may carry a symbolic
reference to a prior step's result until that step executes. Regression through
effects containing a fresh result, including `Create`, is deferred, but these
shapes avoid an input-only wire migration when it is implemented.

The normal authoring surface is the Rust-embedded `affordance!` description
language specified below. It assigns the compact ids and lowers its typed names
into this canonical representation. The canonical types remain directly
constructible by engine internals and tests, but app content should not manually
coordinate parameter ids or type-erased values.

## Rust-embedded action description language

App content normally uses a procedural `affordance!` macro rather than manually
assembling ids and type-erased values. It presents the canonical representation as
a typed function signature with `requires`, `effects`, and an `execute` handler.
The macro generates typed inputs/results, execution and narration adapters, and
registration metadata.

Every DSL construct lowers into the canonical term, formula, guard, and effect
nodes described here. Convenience forms expand before registration; they never
become opaque predicates. The full syntax, generated interface, validation split,
and future external-language boundary are specified in
[affordance-authoring.md](affordance-authoring.md).

## Terms and variable scope

A condition or effect refers to values through terms:

```text
Term = Actor
     | Input(ParameterId)
     | Result(ParameterId)
     | Local(LocalId)
     | Constant(Value)
```

Parameters are shared across the whole affordance. Reusing an input in a
condition and an effect requires both occurrences to denote the same value:

```text
delete(target: Entity)

requires:
    HasComponent(target, Picture)
    RelationTargetIs(target, ControlledBy, Actor)

effects:
    Destroy(target)
```

When `Destroy(target)` unifies with the goal `Destroy(Photo42)`, the requirements
become `HasComponent(Photo42, Picture)` and
`RelationTargetIs(Photo42, ControlledBy, Actor)`. Control of another picture does not
satisfy them.

Locals are existential witnesses scoped to a formula. They express joins whose
identity does not define the action occurrence:

```text
exists locus:
    AtLocus(Actor, locus)
    AtLocus(item, locus)
```

`AtLocus` is evaluated through the engine's transitive `enclosing_locus` query, so
nesting depth does not change its meaning. It is not a stored relation. If
execution needs an existential witness, it becomes an input. A freshly produced
identity becomes a result. Effects may refer only to `Actor`, inputs, results, and
constants.

## A closed state algebra

Planner-visible conditions are constraints over a small set of state slots.
Effects assign or move those same slots:

| State slot | Conditions | Effects |
|---|---|---|
| `RelationTarget(source, K): Option<Entity>` | equals target, unset, or differs from target | `SetRelation(source, K, target)`, `ClearRelation(source, K)` |
| `ComponentPresent(entity, C): bool` | present or absent | `SetComponent(entity, C)`, `RemoveComponent(entity, C)` |
| `LocusOf(entity): Option<Entity>` | equals locus, unset, or differs from locus | `SetLocus(entity, locus)`, `ClearLocus(entity)` |
| `GaugeRegion(entity, G)` | registered qualitative threshold or region | `ShiftGauge(entity, G, Up/Down)` |
| `Exists(entity): bool` | exists or absent | `Create(result)`, `Destroy(entity)` |

Relations are source-functional in the world: a source has at most one target for
one relation kind. `SetRelation` overwrites that slot and therefore falsifies an
equality to every other target. `ClearRelation` has no target argument. Two
incompatible assignments to the same `(source, K)` slot interfere even when no
delete effect was separately declared. Surface
`source.relation_is(K, target)` syntax lowers to
`RelationTarget(source, K) == target`; its negation lowers to inequality, not set
removal.

`LocusOf` is a derived functional slot. Its evaluator calls `enclosing_locus` over
containment. Surface `entity.at_locus(locus)`, `entity.has_no_locus()`, and
`entity.not_at_locus(locus)` constrain equality, absence, and inequality.
`SetLocus` and `ClearLocus` are planner-visible guarantees made by movement; they
do not materialize or maintain `LocatedIn` edges.

Locus assignments have subtree semantics. `SetLocus(root, locus)` assigns that
derived locus to `root` and every entity transitively contained by it in the
resulting structural state. `ClearLocus(root)` assigns `None` to the same closure.
The planner expands this closure through finite containment witnesses composed of
ordinary source-functional `ContainedBy` constraints, including constraints that
earlier plan steps can establish. The current live subtree is evidence, not the
only possible closure.

An affordance that changes containment across locus boundaries advertises both the
`SetRelation` assignment and the resulting `SetLocus` projection when both matter.
Neither effect is inferred from the other: the oracle verifies each declared slot,
and same-locus reparenting need not claim a locus change.

`Create` binds a fresh entity result and establishes its existence. Result-aware
regression and cross-step result substitution are deferred, but the canonical
schema distinguishes results now. Until that regression exists, every effect
containing a result term is excluded as a reverse-index achiever. Such an effect
still participates in interference analysis when its affordance is selected by a
result-free effect. A positive `Exists(input)` requirement is rejected because an
entity input must already be live; positive existence remains meaningful for
goals, results, and plan validity.

The slot kinds are closed; relation, component, gauge, and qualitative-region ids
are open app vocabulary. Static comparison first identifies a state slot, then
unifies its terms and checks assignment compatibility.

This constraint is what makes backward planning possible. A callback named
`feels_really_fondly_about(a, b)` could answer a query but could not be regressed
unless some effect repeated that exact opaque name. Instead, ordered sentiment is
a registered qualitative gauge threshold and actions advertise a direction on the
same gauge. See [gauges.md](gauges.md).

The logical model is a planning projection of world state, not a vocabulary of
English conveniences. "Same locus" is a join over `AtLocus`; meaningful control is
a relation-target equality. They remain distinct formulas. A convenience is
planner-visible only when it lowers to this algebra and actions expose comparable
effects on its backing slot.

Categorical state uses relations or component presence. Ordered quantitative
state uses gauges. An arbitrary calculation that cannot lower into these forms is
permitted only on an opaque, non-plannable affordance. Soft preferences belong in
the cost model.

## Conditions, guards, and resolution contracts

Ordered guards pair applicability formulas with refusal prose. Every affordance
also declares `Deterministic`, `Contested`, or `Opaque` resolution. For a
deterministic act, ground inputs whose entity values are live plus an admitting
gate plus true guards require a successful commit; contested failure is explicit,
and opaque acts do not enter planning. Advertised effects are unconditional
promises of every successful commit.

The complete applicability contract, structural-invariant boundary, conditional
reaction rule, and executable-oracle obligations are specified in
[affordance-contracts.md](affordance-contracts.md).

## Authority is separate

An affordance carries an authority `Gate` alongside its world-state guards.
Authority is resolved from a principal's `Verdict`; it is not a world condition
and the planner does not regress through it.

| | Guard | Gate |
|---|---|---|
| concerns | actor and input state | principal authority |
| source | world | `Verdict` |
| planner reads | yes | no |
| failure | gameplay refusal | authorization refusal |

The act carries its gate so every entry point enforces the same authority
requirement. Automation without an authority source receives the app's default
verdict.

## Non-entity inputs

Inputs may be values other than entities. Their sorts determine how they may
be grounded: entities use knowledge-scoped candidates, finite symbols require an
explicit domain, and non-enumerable text must be supplied rather than invented by
the planner. An executable act with no persistent effect contributes no backward-
planning edge. The complete `say(text: Text)` example is in
[affordance-authoring.md](affordance-authoring.md).

## Registration validation

Registration rejects malformed schemas before grounding or indexing. The complete
validation and reverse-index construction rules live with the behavioral contract
in [affordance-contracts.md](affordance-contracts.md#schema-registration).

## Relation to the other docs

- [affordance-authoring.md](affordance-authoring.md): the Rust macro syntax and
  generated typed handler interface.
- [affordance-contracts.md](affordance-contracts.md): applicability, resolution
  modes, unconditional effects, and behavioral oracles.
- [agency/affordances.md](agency/affordances.md): how app verbs instantiate this
  representation and lower grounded actions to handlers.
- [agency/preconditions.md](agency/preconditions.md): formula evaluation,
  substitution, unification, and candidate binding.
- [agency/planner.md](agency/planner.md): backward regression over the comparable
  condition/effect pairs.
- [actions.md](actions.md): the structural mutations a successful grounded action
  ultimately commits.
- [gauges.md](gauges.md): normalized readings, targets, and directional effects.
- [offers.md](offers.md): partial grounding for pointing clients.
