# Agency

> Status: **target affordance/planner representation specified; implementation
> pending.** The arbiter, execution driver, and reference drives retain their
> documented responsibilities but will consume steps built from typed
> affordance inputs/results, functional state slots, and explicit resolution
> contracts.

Agency lets an entity pursue a world-state goal through the same gameplay acts a
player uses. It adds selection and planning, not a second mutation or rule path.

## The stack

1. **Drives** read an actor's internal state and propose goals with urgency.
2. **The arbiter** chooses among goals and maintains commitment with hysteresis.
3. **The planner** regresses a chosen goal through registered affordance effects.
4. **The execution driver** runs steps and replans around contested outcomes.
5. **Affordance implementations** honor their resolution contract and commit through the
   structural action executor.

An imperative goal may enter at the arbiter, and a hand-authored sequence may enter
at execution. These are intentional bypass seams for behavior that does not need
planning.

## The shared affordance representation

The non-optional engine layer owns:

- `Actor` and typed action-local inputs/results;
- terms, substitutions, grounded actions, and result bindings;
- the closed functional state-slot algebra and derived locus slot;
- guards with refusal reasons;
- deterministic, contested, and opaque resolution modes;
- affordance authority gates;
- schema validation and condition evaluation.

The optional `musce_agency` crate owns:

- effect-goal unification and backward regression;
- candidate binding against actor knowledge;
- the cost model;
- plan search and exclusion;
- the arbiter and execution driver.

App content owns concrete relation/component/gauge ids, affordance schemas,
handlers, costs, drives, goals, and knowledge/perception policy.

## Why the vocabulary is closed

Planning requires static comparison between a desired state and an action's
advertised transition. The state algebra is:

```text
RelationTarget    ↔ SetRelation / ClearRelation
ComponentPresent ↔ SetComponent / RemoveComponent
LocusOf           ↔ SetLocus
GaugeRegion       ↔ ShiftGauge
Exists            ↔ Create / Destroy
```

Relation kinds, component ids, gauge ids, and qualitative-region ids are open app
vocabulary. Arbitrary semantic predicate callbacks are excluded because
queryability does not make a condition regressible. Relation and locus slots are
functional, so assignment and interference are explicit.

Creation writes a fresh result parameter. The schema and runtime reserve result
bindings now; planner regression through them is a deferred extension.

Categorical facts use functional relations or component presence. Spatial scope
uses the derived `LocusOf` slot. Ordered values use registered gauge regions and
QSIM direction. A rule outside that algebra makes an act opaque and non-plannable;
soft preferences belong in cost.

## Action identity

An affordance has one privileged actor plus a fixed typed signature:

```text
take(item: Entity)
put(item: Entity, container: Entity)
go(exit: Entity, destination: Entity)
say(text: Text)
craft(material: Entity) -> (product: Entity)
```

Input and result names are local authoring labels, not universal semantic roles. The
canonical runtime representation assigns them compact ids. Conditions and effects
refer to those ids, so repeating a parameter preserves entity identity across the
whole transition.

A grounded action and outcome are:

```text
(affordance id, actor, typed input values) -> typed result values
```

Text syntax, clicked controls, scripts, and planning each produce partial
substitutions and converge on this one ground form.

## Knowledge and binding

The planner binds open entity parameters only from candidates the actor knows.
Knowledge is gameplay: the planner never scans a global index to discover an
unknown object.

The current truth about a known candidate is read when planning and again when
executing. A later belief model may allow remembered state to diverge from truth,
but that changes the knowledge source rather than the condition/effect algebra.

Non-entity parameter domains are explicit. Finite symbols may be enumerable;
non-enumerable text must be supplied and is never invented by the planner.

## Planning and execution

Backward regression unifies a goal condition with an affordance effect. The
substitution is applied to the affordance's requirements, preserving identity:
deleting a specific picture requires control of that picture.

Remaining entity inputs are solved from their conditions. Every input is ground
before its step executes; it may refer symbolically to a result of an earlier step
until that result is produced.

Execution calls the same app implementation as a player command or pointing
action. It rechecks the gate and declarative guards, then follows the declared
resolution mode. True guards require a deterministic act to commit; refusal is a
contract violation. A contested failure triggers replanning from live state.
Opaque acts do not enter planning.

Planning is single-actor. A step always names the planning `Actor`; it never
silently schedules another entity's action. Cooperation is modeled as an act the
actor can attempt, such as `ask_to_give(item, holder)`, with structural conditions
such as an `Obeys` relation and `Contested` resolution for the other entity's
choice. This keeps authority and causal responsibility explicit.

## Gauges

Gauges expose ordered derived readings without leaking backing component
representations. Apps register qualitative regions over the raw finite reading.
Planner goals use one-sided named thresholds; effects promise strict `Up` or
`Down` progress. Exact raw points and interior bands remain readable but are not
regressible from direction alone.

State about a relationship is represented on a reified relationship entity. For
example, an attitude entity relates an owner to a subject and carries an
`Affinity` gauge. This keeps gauges unary while supporting dyadic social state.

## Plans are transient

Synthesized plans, affordance steps, and result bindings are runtime values. They do not
embed in persisted `Intent` or `Steps` components. The app lowers each step
directly through its affordance implementation.

This keeps the dependency direction clean:

- `musce_action` owns the shared representation and structural executor;
- `musce_agency` depends on it for generic planning;
- the app depends on both and supplies concrete content and execution.

## Authoring

The canonical AST defines the semantics, and the normal app-facing authoring
surface is a Rust-embedded `affordance!` description language. Its typed signature
declares inputs and results; `requires` and `effects` construct the closed logical
algebra; `resolution` states the execution guarantee; `execute` names an ordinary
Rust handler; and `narrate` names the typed shared narrator. Each ordered
requirement carries the refusal reason used when it is the first failed guard. The
procedural macro generates typed values, adapters, and registration metadata
before lowering the declaration into the canonical AST.

DSL conveniences must expand into canonical formulas during schema construction.
They cannot introduce opaque planning predicates or bypass registration
validation. A future data format, Lua surface, or Lisp-like syntax may target the
same AST if content needs justify it; that would be another authoring front end,
not a second semantic model.

## Documents

- [affordances.md](affordances.md): concrete signatures, effects, and grounded
  execution.
- [preconditions.md](preconditions.md): terms, condition algebra, substitution,
  and candidate binding.
- [planner.md](planner.md): backward regression, QSIM handling, and termination.
- [arbiter.md](arbiter.md): goal selection and commitment.
- [execution.md](execution.md): result-aware execution and contested replanning.
- [drives.md](drives.md): urgency-producing app policy.
- [../affordances.md](../affordances.md): engine-owned canonical representation.
- [../affordance-authoring.md](../affordance-authoring.md): Rust macro syntax and
  generated typed handler interface.
- [../affordance-contracts.md](../affordance-contracts.md): applicability,
  resolution modes, and behavioral guarantees.
- [../gauges.md](../gauges.md): quantitative state model.

## Deferred

- the canonical AST and Rust `affordance!` authoring macro;
- implementation of concrete affordances and planner search;
- fresh-result regression through `Create` (the schema, outcome, plan-reference,
  and wire shapes are decided now);
- multi-room knowledge and richer belief;
- one-beat-per-tick plan interleaving;
- per-actor cost learning;
- optional non-Rust authoring syntax for hot reload or non-Rust content authors.
