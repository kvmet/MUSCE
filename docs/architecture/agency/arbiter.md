# The Arbiter

> Status: **selection and commitment built; condition-formula goal integration
> pending.** Goal selection
> lives in `musce_agency` (`arbiter.rs`) as `Arbiter` / `Goal` / `Urgency`. The
> reference magpie ([drives.md](drives.md)) feeds it two *competing* drives (hoard and
> admire) on the sim tick, so hysteresis is no longer dormant: the arbiter holds a
> commitment across ticks that a near-equal challenger cannot steal. Because agency
> types do not serialize, the arbiter is reconstructed each tick from an app-owned tag
> via `Arbiter::resume` (see "Cross-tick commitment" below). The same injection point
> still takes a hand-authored imperative order.

The arbiter answers "of everything this agent could want, which does it pursue
right now?" A [`Goal`] is a condition formula in the planner's closed state
algebra, ground or existential, paired with an `Urgency`. Drives emit goals; the
arbiter ranks them and *commits* to one, handing its formula to the planner.

## The real work is commitment, not ranking

Picking the highest-urgency goal is a one-line max. The substance is not thrashing:
two goals whose urgencies wobble around each other tick to tick must not flip the
agent's commitment back and forth, wasting every plan half-executed. So the arbiter
holds a committed goal and applies **hysteresis**: a challenger steals the
commitment only when its urgency exceeds the incumbent's by more than a margin
(`Arbiter::new(hysteresis)`). A zero margin degenerates to "always take the current
max" (no commitment); a large one makes a stubborn agent. This is the standard
answer to goal oscillation, and it is the arbiter's reason to exist as a stateful
object rather than a `max` call.

Goal **identity is the normalized formula, not the urgency.** The same want re-offered next
tick with a shifted urgency (a drive's curve moved) is the same commitment,
refreshed, not a new goal. So `select` finds the incumbent in this tick's offering
by formula, refreshes its urgency, and compares challengers against the live
value. An incumbent its drive stops offering at all is retired, and the field is
re-picked.

## The arbiter never reads the world

"Highest-urgency *unsatisfied* goal" is met without a satisfaction test in the
arbiter, deliberately. Satisfaction is handled in the two places that already know
it, so it is never re-derived here (and never gotten wrong for an existential goal,
which a naive `holds` pass could not answer without redoing the planner's binding):

- **Upstream:** a met need is expressed as *low urgency* by its drive. A fed NPC's
  `eat` urgency falls to the floor, so the goal simply loses the ranking. Drives own
  "is this need pressing," which is exactly a satisfaction reading of the NPC's own
  state.
- **Downstream:** an already-true goal produces an *empty plan*, which the
  [execution driver](execution.md) reports as `Progress::Achieved`. An imperative caller
  then calls `Arbiter::release`, dropping the commitment so the next `select` re-picks.
  A drive loop instead lets the commitment retire by fading urgency (see "Cross-tick
  commitment"), so it never calls `release`.

So the arbiter is pure priority-plus-commitment over whatever candidate set it is
handed. That set comes from drives (the magpie's hoard drive is the first, see
[drives.md](drives.md)) or, for imperative orders, from direct injection.

## The two bypass seams meet here

An **imperative goal** (a prescribed order like `greet(playerX)`) injects straight
into the arbiter's candidate set as a `Goal` with a high fixed urgency: no drive,
no component curve. That is the one-line injection the stack doc calls out, and it
is why the arbiter is fully testable before any drive exists. The *other* bypass, a
hand-authored sequence that skips planning entirely, injects one layer lower at the
execution sweep, not here (see [README](README.md)).

## Cross-tick commitment

Hysteresis only bites across ticks, and the sim's persisted state is the serializable
world, not a long-lived arbiter (agency types deliberately do not serialize). So an app
that wants a commitment to survive between ticks does not keep the arbiter alive: it
records *which* goal was committed to as ordinary world state and rebuilds the arbiter
each tick with `Arbiter::resume(hysteresis, incumbent)`. The incumbent it passes is
*this tick's* goal from the committed drive, looked up live, so a stale record never
revives a goal a drive has stopped offering. `select` matches that incumbent into the
candidate set by normalized formula exactly as an in-run commitment is matched, applies the band,
and the winner is recorded again. `resume` is the whole seam this needs, and `new(h)` is
now just `resume(h, None)`.

The reference magpie does exactly this with a `Committed(Drive)` component (see
[drives.md](drives.md)). Under this pattern a satisfied drive expresses itself as fading
urgency and, once below its threshold, stops offering, so the arbiter retires the
incumbent on its own and re-picks; the loop never calls `release`.

## The loop it closes

```rust
let mut arbiter = Arbiter::new(hysteresis);
loop {                                   // once per agent tick (wiring deferred)
    let Some(goal) = arbiter.select(&candidate_goals) else { continue };
    match driver.pursue(actor, &goal.condition, &known, world, run) {
        Progress::Achieved | Progress::Abandoned => arbiter.release(),
    }
}
```

`select` chooses and commits; the [driver](execution.md) plans and runs; `release`
frees the commitment when the pursuit ends either way. Composing these two over a
real agent on the sim tick is the deferred wiring step; the loop above runs today
off-thread, which the `musce_ref` composition oracle exercises end to end through
the same `perform` a player hits.

## Falsifiability

The arbiter is tested in `arbiter.rs` against hand-built goal sets: it picks the
max, holds a commitment a near-equal challenger cannot steal, resumes a prior tick's
  commitment on a fresh arbiter, yields when a challenger clears the band or when its own
urgency fades, drops a retired incumbent, and re-picks freely after `release`. One
end-to-end check is the `musce_ref` oracle
`the_arbiter_selects_a_goal_the_driver_carries_out`: two injected goals, the urgent one
committed and driven to completion through real `perform`, the world reflecting the
*selected* goal specifically (pursuing the loser would have stopped short). The *live*
check is the magpie oracle `commitment_stops_the_two_drives_thrashing_the_bead` (see
[drives.md](drives.md)): with hoard and admire both pressing, a committed bird moves the
bead far less than a no-commitment control that thrashes it nearly every tick, while
still serving both drives.

## Relation to the other docs

- [README](README.md): the agency stack and the build order; the arbiter is stack
  layer 2, build step 5, policy over a working planner.
- [planner.md](planner.md): the planner the committed goal's condition is handed to.
- [execution.md](execution.md): the driver that runs the plan and whose
  `Achieved`/`Abandoned` is the arbiter's cue to `release`.
