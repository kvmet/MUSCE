# Execution: the driver and replan-on-veto

> Status: **built (agency build step 5).** The execution driver lives in
> `musce_agency` (`driver.rs`) as `Driver` / `Beat` / `Progress`, on top of the
> planner's `plan_excluding`. It runs a committed goal's plan to completion in one
> call. It is now wired onto the sim tick by the reference app's magpie (see
> [drives.md](drives.md)), which runs the whole plan per scheduled tick. Interleaving
> a plan a *beat* per tick (yielding between beats) is the remaining deferred
> refinement: a scheduling concern, not a change to this logic.

The driver is the bottom of the agency stack: given a committed goal, it plans,
runs the plan beat by beat through the app's grounded action, and **replans around
a beat that vetoes**. It is the executing half of "a scripted actor is vetoed
exactly as a player is": each step lowers through the same `perform` a typed verb
runs, so the same guard refuses it.

## Replan-on-veto is where soundness lives

The planner emits add-only plans and does not model interference (see
[planner.md](planner.md), "Add-only effects"). It is a proposer. The correctness
backstop is here: when a beat vetoes, the world has diverged from the plan's
assumption (another actor moved something, a contested action failed), so the driver
does not push on. It adds the failed `(affordance, frame)` to an **exclusion set**
and replans **from the now-current world**. The replan either routes around the
failed step (a different binding, a different chain) or, if that step was the only
route, returns no plan and the driver reports `Abandoned`.

Two properties make this the whole answer to "why doesn't it retry the same failed
action forever," together with the planner's internal search bounds:

- **The excluded step is never re-issued.** Exclusion keys on the affordance name
  and the exact bound frame, so `plan_excluding` will not re-propose the step that
  just vetoed. A *different* binding of the same affordance stays available (put into
  a different chest), which is what "route around" means.
- **Pursuit terminates.** Each replan adds at least one distinct step to the
  exclusion set, and the step space over a finite `known` set is finite, so
  exclusion eventually exhausts every route and the planner returns `None`. A
  `MAX_REPLANS` constant backstops the pathological case, mirroring the planner's own
  bounds; it does not bind in practice.

Replanning reads **live** world state, not the state the plan was built against. A
plan whose early beats committed is not redone: those effects are already true, so
the new plan starts from them. The `musce_ref` oracle proves exactly this: a
two-step take-then-put plan whose `put` is made to veto (a chest smashed mid-plan)
recovers by re-planning onto the surviving container *without redoing the take*,
because the coin is genuinely held after the first beat.

## The generic beat boundary

`pursue` takes a closure `FnMut(&mut World, &Step) -> Beat`. `Beat` is
`Committed | Refused`: the only thing the loop needs to know about a beat is whether
it landed. The app maps its own richer result onto it (`musce_ref` collapses
`Outcome::Committed` / `Outcome::Refused(_)` to `Beat`), so the generic driver never
names an app's outcome type, and the refusal *reason* is not carried up (the loop
excludes the step regardless of why it failed). Keeping the lowering in the caller's
closure is what lets the driver stay in `musce_agency` while `perform` and the veto
stay in the app crate.

`Progress` is `Achieved | Abandoned`. `Achieved` covers the empty plan (an
already-true goal ran zero beats), so it doubles as the arbiter's "satisfied,
release it" signal; `Abandoned` is "no route survived," the other release cue.

## The API

```rust
Driver::new(&planner).pursue(actor, goal, known, &mut world, run) -> Progress
```

The static context (the planner) lives in the `Driver`, mirroring
`Planner::new(...).plan(...)`; the per-pursuit inputs are `pursue` arguments.
`world` is `&mut` because execution mutates it; the planner borrows it immutably
for each replan within the call.

## What is deferred

- **The one-beat-per-tick sim wiring.** `pursue` runs a whole plan in one call, and
  the magpie ([drives.md](drives.md)) now calls it once per scheduled tick, so the
  driver is live on a real NPC. What stays deferred is interleaving a plan a *beat*
  per tick (one beat per agent per tick, yielding between beats), a scheduling concern
  for the sim thread. The replan logic here does not change; it is re-entered per beat
  instead of looped internally.
- **A natural in-app veto trigger.** With deterministic, precondition-gated verbs
  and a single actor, a correctly-planned plan never vetoes at execution; the
  divergence the replan path handles arrives with concurrent agents or a
  variable-outcome action (a skill roll, combat), the same gate the per-actor
  learner of build step 6 waits on. Until then the path is exercised by a constructed
  mid-plan mutation standing in for another actor, which drives the real `perform`
  veto, not a stub.

## Falsifiability

The driver is tested in `driver.rs` against the planner's ground-fact stub: a plan
runs to completion; a permanently vetoed step is issued exactly once and then the
goal is abandoned (the "not forever" proof); a vetoed step is routed around onto
another binding; an unreachable goal abandons without running anything. The ground
truth is the `musce_ref` oracle
`the_driver_replans_around_a_vetoed_beat_and_finishes_the_goal`: a real veto through
`perform`, recovery against live mutated state, and the goal predicate actually true
in the `World` at the end.

## Relation to the other docs

- [README](README.md): the agency stack; execution is layer 4, and the "existing
  sequence sweep" it will eventually be wired into.
- [planner.md](planner.md): the proposer whose `plan_excluding` this consumes, and
  the add-only effect model whose soundness this backstops.
- [arbiter.md](arbiter.md): the layer above, whose committed goal flows into
  `pursue` and whose `release` this drives via `Achieved` / `Abandoned`.
- [../actions.md](../actions.md): the structural executor each beat ultimately
  commits through, the commit-time backstop below even this.
