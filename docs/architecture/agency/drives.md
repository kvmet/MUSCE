# Drives and the Agency Loop

> Status: **categorical reference drives built.** Drives remain app policy over
> need state and emit canonical condition formulas. The reference hunger and
> hoard drives use components and relations; qualitative gauge goals remain
> specified for the deferred QSIM extension.

A drive turns an actor's internal need into a goal and an urgency. It does not
choose actions. The arbiter chooses among goals, the planner finds grounded
affordances, and the driver executes them.

## Responsibilities

A drive reads the actor's own persisted state:

```text
drive(actor) -> Option<Goal { condition, urgency }>
```

It may use exact app components to compute urgency. The goal it emits must use the
planner's comparable condition algebra. This keeps need policy app-specific while
goal achievement remains generic.

Drives do not query the world for candidate objects. A hunger drive knows that the
actor wants a lower hunger reading; the planner decides which known food and which
affordances can move it there.

## Gauge goals

Ordered needs are registered qualitative gauge thresholds:

```text
hunger drive:
    goal = GaugeAtMost(Actor, Hunger, Sated)
    urgency = app_curve(current_hunger)
```

An `eat(food)` affordance advertises:

```text
ShiftGauge(Actor, Hunger, Down)
Destroy(food)
```

The goal identifies no food. During regression, `eat` is selected by the matching
hunger direction, then its `food` parameter is bound from its conditions:

```text
HasComponent(food, Edible)
RelationTargetIs(food, ContainedBy, Actor)
```

This is the ordinary free-parameter binding path, not a special consume mechanism.

Categorical drives remain ordinary formulas. A hoarding goal can be:

```text
exists item:
    HasComponent(item, Shiny)
    RelationTargetIs(item, ContainedBy, nest)
```

## Urgency and satisfaction

Urgency is drive policy, not generic gauge distance. `GaugeLevel` is ordinal, so
the engine may not assume that a larger numeric gap means proportionally more
urgency or effort. The app maps its exact need state to `Urgency`.

A goal already true yields an empty plan and `Achieved`. A standing drive may also
fade its urgency or stop offering once its need is sufficiently relieved. The
arbiter does not independently reinterpret the goal.

## Competing drives and commitment

Two near-equal drives must not switch the actor's goal every tick. The arbiter
holds a commitment and applies hysteresis: a challenger replaces the incumbent
only when it clears the configured margin.

Cross-tick commitment is app-owned persisted state identifying the selected drive
or goal family. Each tick:

1. drives compute fresh goals and urgency;
2. the app maps the persisted commitment to the corresponding live goal;
3. a reconstructed arbiter resumes that commitment;
4. selection applies hysteresis;
5. the selected drive id is persisted again.

The committed goal formula is always taken from current drive output. A stale
record cannot revive a goal the drive no longer offers.

## Knowledge

The drive does not locate objects. The planner binds entity parameters from the
actor's known candidates. The app's perception policy decides that set: room
contents, inventory, a personal cache, sensed entities, and remembered entities
may differ by actor.

Ignorance remains gameplay. A drive that wants food does not consult a global food
index.

## The tick loop

```text
metabolism: update persisted need state
drives:     produce current goals and urgency
arbiter:    resume and select one commitment
planner:    create a plan of affordance steps
driver:     execute one or more beats, replanning on contested failure
narration:  emit the app's account of committed acts
```

Metabolism changes needs according to app rules. Drives only interpret them.
Planning and execution never reset a drive directly; relief is observable through
the world state left by successful acts.

Controlled actors may suspend autonomous drive processing while keeping their
persisted need state. That is app policy over embodiment, not planner behavior.

## Falsifiability

A reference drive is complete only when tests prove:

- its need changes over time;
- its urgency crosses an actionable threshold;
- the emitted goal selects affordances through static effects;
- free action parameters bind from known candidates;
- real execution moves the target gauge or categorical state;
- satisfaction reduces or retires the drive;
- competing drives demonstrate commitment rather than thrashing;
- impossible or contested-failure pursuits do not falsely relieve the need.

## Relation to the other docs

- [README](README.md): the full agency stack.
- [arbiter.md](arbiter.md): commitment and hysteresis.
- [planner.md](planner.md): effect-goal matching and free-parameter binding.
- [execution.md](execution.md): grounded execution and replanning.
- [../gauges.md](../gauges.md): normalized readings and QSIM direction.
