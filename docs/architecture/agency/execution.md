# Execution and Replanning

> Status: **target grounded-step integration specified; implementation pending.**
> The driver executes canonical actions, binds results, and replans around
> contested outcomes or changed live state.

The driver is the executing half of agency. Given a committed goal, it asks the
planner for a transient plan, runs its steps through the app's shared affordance
implementation, and replans when a contested step fails or live state invalidates
the remaining plan.

## Grounded beats

Every `Step` carries supplied values or references to earlier results:

```rust
struct Step {
    affordance: AffordanceId,
    actor: EntityId,
    inputs: Box<[ValueOrResultRef]>,
}
```

Each input is validated against the affordance signature, and every entity input
must still be live. A result reference is resolved before its dependent step
executes. A missing or destroyed entity invalidates the proposed beat before gate
or guard evaluation; execution performs no grammar interpretation or entity name
resolution.

The app maps its outcome onto:

```text
Beat = Committed(results) | ContestedFailure
```

The generic driver records returned result bindings and whether a contested
attempt committed. Failed input-liveness validation triggers replanning. A
deterministic refusal or structural executor failure after live inputs, an
admitting gate, and true guards is a contract violation, not a normal `Beat`.

## Replan on invalidation or contested failure

The planner is a proposer. A symbolic plan may be invalidated by:

- another actor changing the world;
- a grounded entity input being destroyed before its beat;
- a stochastic or contested outcome;
- stale or incomplete beliefs.

The planner and structural executor share relation invariants such as acyclicity.
An opaque execution-only rule is permitted only on an affordance excluded from the
planner's effect index.

On a contested failure or failed live-state revalidation, the driver:

1. records the exact grounded step in an exclusion set;
2. reads the current live world;
3. asks the planner for another route to the same goal;
4. executes the replacement plan or abandons the pursuit.

An exclusion key is:

```text
(affordance id, actor, resolved typed input values)
```

The failed grounding is not proposed again during that pursuit. Another grounding
of the same affordance remains available.

Committed earlier steps are not rolled back. Their effects are already part of
world truth, so the replacement plan begins from the partial result.

## Termination

Each contested failure adds a distinct resolved input grounding to the exclusion
set. Candidate domains are finite and knowledge-scoped, and the planner has its
own node/depth bounds. `MAX_REPLANS` provides an outer bound.

The driver therefore reports:

```text
Progress = Achieved | Abandoned
```

An already-satisfied goal produces an empty plan and `Achieved`. Exhausting every
route produces `Abandoned`.

## Tick integration

The logical driver is independent of scheduling. An app may:

- run a short plan to completion in one scheduled turn;
- execute one beat per tick;
- budget several beats per actor;
- interleave actors round-robin.

One-beat-per-tick execution stores the committed goal, remaining plan, and
exclusions as app-owned runtime or persisted state. Before each beat it revalidates
the action inputs and guards against the live world.

## Gauge steps

After a committed `ShiftGauge` affordance, execution re-reads the gauge:

- movement to a different registered region in the advertised direction validates
  the effect;
- reaching the goal may finish the pursuit;
- the same region or opposite movement is a contract violation;
- partial movement permits another planned application within bounds.

The region movement must be strict. The driver never assumes a raw numeric
magnitude from `GaugeDirection`; the finite registered region order bounds
progress.

## Duration

Duration is represented by state and repeated beats, not by an effect scheduled to
become true later. A long activity establishes a categorical marker such as
`Sleeping` or `Cooking`; a system or repeated affordance performs one guaranteed
per-beat change; only a beat that guarantees crossing a qualitative region may
advertise `ShiftGauge`. A completion reaction removes the marker or establishes the
final categorical state at its modeled threshold. Every planner-visible effect
therefore describes the immediate successful commit.

## API shape

```rust
Driver::new(&planner).pursue(
    actor,
    goal,
    known,
    &mut world,
    run_grounded_action,
) -> Progress
```

`run_grounded_action` is app-supplied, preserving the crate boundary: the generic
driver knows the canonical affordance representation but not concrete app handlers
or outcome prose.

## Falsifiability

Tests must prove:

- a generated plan commits through real affordance implementations;
- an already-true goal executes zero beats;
- a stale or destroyed entity input invalidates a beat and replans without being
  reported as deterministic contract drift;
- a contested failed grounding is not retried during the pursuit;
- another grounding of the same affordance may recover;
- replanning observes effects committed before the contested failure;
- an unreachable goal terminates as `Abandoned`;
- deterministic applicable steps commit and bind well-typed results;
- gauge effects cross a qualitative region strictly in their advertised direction.

## Relation to the other docs

- [planner.md](planner.md): the proposer and exclusion-aware search.
- [affordances.md](affordances.md): concrete grounded implementations.
- [../affordance-contracts.md](../affordance-contracts.md): deterministic and
  contested execution guarantees.
- [arbiter.md](arbiter.md): committed goal selection above the driver.
- [../actions.md](../actions.md): structural commit-time authority.
