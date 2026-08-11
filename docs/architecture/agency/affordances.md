# Affordance Instances and Grounded Execution

> Status: **built for the reference categorical actions.** Concrete app
> affordances use the engine-owned input/result and functional state-slot
> representation described in [../affordances.md](../affordances.md).

An affordance is the shared gameplay act beneath every front end. A text command,
a clicked control, a script, and an autonomous plan may bind it differently, but
all produce the same grounded action and run the same app implementation.

## Signature, model, and implementation

A concrete affordance carries:

- a name and typed action-local input/result signature;
- ordered guards over the closed condition algebra;
- declared effects over the matching effect algebra;
- a deterministic, contested, or opaque resolution mode;
- an authority gate;
- a typed narrator;
- an app implementation that commits the act and returns results.

Cost is not intrinsic to the affordance. `CostModel` reads
`(actor, affordance, grounding, world)` so costs may vary by actor and selected
inputs.

The declared effects are unconditional promises of every successful act. Execution
does not interpret them as structural commands. The shared performer checks the
gate and guards, then calls the app implementation to resolve the declared mode and
commit through the structural `Action` vocabulary. A deterministic implementation
cannot add another applicability veto.

## Reference signatures

The signatures describe action occurrences without imposing global participant
roles:

| Affordance | Parameters | Successful state change |
|---|---|---|
| `take` | `item: Entity` | item becomes contained by actor |
| `drop` | `item: Entity`, `destination: Entity` | item becomes contained by destination |
| `give` | `item: Entity`, `recipient: Entity` | item becomes contained by recipient |
| `put` | `item: Entity`, `container: Entity` | item becomes contained by container |
| `go` | `exit: Entity`, `destination: Entity` | actor becomes located at destination |
| `open` | `door: Entity` | `Locked` is removed from door |
| `unlock` | `door: Entity`, `key: Entity` | `Locked` is removed from door |
| `eat` | `food: Entity` | food loses `Edible`; actor gains `Fed` |
| `hang` | `item: Entity`, `support: Entity`, `fastener: Entity` | item's mounted relation slot is assigned to support |
| `say` | `text: Text` | no persistent effect; emits speech |
| `craft` | input `material: Entity`; result `product: Entity` | product is created and categorized |

Names such as `item`, `recipient`, and `fastener` are local documentation. The
engine sees typed parameter ids. Alternate grammar can bind the same signature:

```text
hang {item} on {support}
decorate {support} with {item}
```

The preposition is parser syntax, not a generic action field.

## Worked examples

### Delete the entity actually controlled

```text
delete(target: Entity)

requires:
    HasComponent(target, Picture)
    RelationTargetIs(target, ControlledBy, Actor)

effects:
    Destroy(target)
```

Effect-goal unification binds `target` before the requirements are considered.
The actor must therefore control the exact entity being destroyed, not merely
some entity bearing `Picture`.

### Hang with an inferred fastener

```text
hang(item: Entity, support: Entity, fastener: Entity)

requires:
    RelationTargetIs(item, ControlledBy, Actor)
    RelationTargetIs(fastener, ControlledBy, Actor)
    HasComponent(support, HangingSurface)
    HasComponent(fastener, Fastener)

effects:
    SetRelation(item, MountedOn, support)
```

A parser may bind `item` and `support` while candidate solving supplies
`fastener`. The grounded action contains all three inputs before execution. If
the fastener is consumed, the same parameter appears in `Destroy(fastener)`.

### Speech as an executable, non-planning act

```text
say(text: Text)

requires:
    HasComponent(Actor, Voice)
    ¬HasComponent(Actor, Muted)

effects:
    []
```

The handler emits `Speech { speaker: Actor, text }`. Because there is no persistent
effect, backward planning does not select `say` to satisfy a world-state goal.

## Grounding paths

Each front end produces a partial input substitution:

- A text parser binds the parameters named by its grammar.
- A pointing client binds the selected entity to the parameter declared by the
  offer and supplies further picks as needed.
- A planner binds parameters by effect-goal unification and condition solving.
- A script may provide every input directly.

Entity parameters left open may be solved against entities known to the actor.
Non-enumerable inputs such as `Text` must already be ground. A `GroundAction` is created
only when every input has a value of the declared sort. Results are absent until a
successful `ActionOutcome`; offers never request them.

No front end receives special permission from having resolved a value. The
grounded action runs the gate, guards, and declared resolution contract through
the shared performer.

## Effects and structural actions

The condition/effect algebra mirrors the planner-relevant dimensions of the
structural action set:

- functional relation targets;
- component presence;
- derived enclosing locus;
- entity existence;
- qualitative gauge thresholds and strict direction.

The mapping need not be one structural action per affordance. `eat` may destroy a
food entity and alter a backing hunger component; its planning effects are
`Destroy(food)` and `ShiftGauge(Actor, Hunger, Down)`. Conversely, speech emits an
event and performs no structural mutation.

Executable-oracle tests ground the affordance and run its real implementation. A
deterministic applicable act must commit, every result must have its declared sort,
every advertised slot assignment must hold afterward, and every gauge shift must
cross into a different registered region in its advertised direction. Contested
acts are checked across successful outcomes; opaque acts have no planner contract.
This prevents metadata from drifting from gameplay truth.

## Guards and resolution

Possession, control, locus, categorical component presence, and qualitative gauge
thresholds are planner conditions. A condition whose state slot no action changes
is still declared as a guard and filters candidates.

For a deterministic affordance, ground inputs whose entity values are live plus
an admitting gate plus true guards require commitment. A normal refusal or
structural failure is contract drift. A contested affordance may fail a valid
attempt and trigger replanning. An opaque affordance may apply richer unmodeled
rules, but it is deliberately absent from the planner's effect index. A
containment-cycle check belongs to the declared acyclic relation transition
semantics, which both planner and executor enforce.

Effects describe only what every successful commit guarantees. A consequence that
fires only at a threshold or on some outcomes is a reaction to a guaranteed state
change, not a conditional affordance effect.

## Indexes over affordances

The registered set is indexed in two ways:

- **By name/id** for commands, pointing, scripts, and grounded dispatch.
- **By effect shape** for regression. The key is the closed state-slot variant plus
  its relation/component/gauge id; term unification happens after lookup. Opaque
  affordances are excluded.

The planner generates groundings lazily. It does not materialize every
affordance/entity combination.

## Authoring surface

App content normally declares an affordance with the Rust-embedded `affordance!`
description language. It resembles a typed function signature with a logical
contract:

```rust
affordance! {
    encourage(attitude: Entity) {
        requires {
            attitude.has_component(InterpersonalAttitude)
                => "That attitude cannot be influenced.";
        }

        effects {
            attitude.shift_gauge("affinity", Up);
        }

        gate Open;
        resolution Deterministic;
        execute musce_ref::act::encourage;
        narrate musce_ref::act::narrate_encourage;
    }
}
```

The procedural macro generates typed input/result structs, execution and narration
adapters, and registration metadata, then lowers the logical declarations into the
canonical AST. Each `requires` entry pairs a condition with its ordered execution
refusal reason; the planner consumes the condition and ignores the prose.

```text
             Rust affordance! declaration
                         ↓
typed handler adapter + canonical affordance representation
                         ↓
          validation, grounding, and execution
```

Convenience forms are syntax-level expansions, not new predicates. For example,
`same_locus(Actor, item)` lowers to two canonical `AtLocus` constraints sharing an
existential local. No authoring surface may introduce opaque planner callbacks or
bypass schema validation.

The canonical representation remains independent of Rust macro syntax. A future
data file or scripting language may lower to the same AST, but it does not define
a parallel condition/effect model.

## Relation to the other docs

- [../affordances.md](../affordances.md): canonical engine representation,
  guards, gates, and registration validation.
- [../affordance-authoring.md](../affordance-authoring.md): complete Rust macro
  syntax and generated handler interface.
- [../affordance-contracts.md](../affordance-contracts.md): applicability,
  resolution modes, and executable oracles.
- [preconditions.md](preconditions.md): the logical algebra and binding rules.
- [planner.md](planner.md): effect-goal unification and regression.
- [../actions.md](../actions.md): structural commit path and atomicity.
