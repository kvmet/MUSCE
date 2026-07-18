# Drives, and the live agency loop

> Status: **built (first drive, first live wiring).** The reference game's magpie
> (`musce_ref`, `hoard.rs`) is the first autonomous agent: a `Hoarder` need
> component, a metabolism that raises it, a drive that turns it into a goal, and the
> arbiter/driver loop run on the sim tick through the same `perform` a player hits.
> This is the content slice that makes a drive falsifiable and the sim wiring the
> thing that makes it live. Richer and *competing* drives, cross-tick arbiter
> commitment, and per-beat interleaving are deferred, below.

A **drive** turns an NPC's internal need-state into a goal with an urgency. It reads
the NPC's *own* components, never the world or its beliefs: "am I hungry / restless /
hurt" is a reading of the self. The arbiter ranks the goals drives emit and commits
to one; the planner and driver carry it out. Until now the whole stack ran only on
hand-injected goals off any tick; the magpie is the first thing to close the loop on
a real creature.

## The magpie: the first falsifiable drive

A drive is unfalsifiable until a need-component actually *changes* and some action
*relieves* it: with a frozen need and no consumer, nothing distinguishes a working
drive from a constant. The magpie supplies both halves:

- **The need** is `Hoarder { urge }`, a component on the bird. Its presence opts the
  creature into the drive, exactly as `Wander` opts one into wandering.
- **The metabolism** raises `urge` each scheduled tick (saturating), so an idle
  magpie visibly grows restless.
- **The drive** (`hoard_drive`) reads `urge`; past a threshold it emits a goal to get
  *some* shiny thing into its nest, with urgency equal to the urge. Below the
  threshold it emits nothing, so a content bird simply loses the ranking without the
  arbiter ever testing satisfaction against the world.
- **The relief** is stowing: when the goal becomes true the urge is reset. The bird
  is content again until the metabolism climbs it back up.

The goal is `∃x. related(x, nest, contained_by) ∧ tag(x, shiny)`, where the nest is a
constant the bird reads from its own `Nest` edge and `x` is the fungible slot the
planner binds against what the bird knows. That is exactly the single-existential
goal the planner already binds (planner step 4b): `tag(x, shiny)` filters the known
candidates, and regression plans `take(x) → put(x, nest)` for the rest.

## Why *stow*, not *consume*

The reference verb set (`take`/`drop`/`put`, no `go`) supports two drive shapes. A
**consume** drive (hunger → eat) acquires a thing and a consumer destroys it to
relieve the need; the consumer is a *new* verb, and routing it through the planner
would need mid-search existential binding (its effect is about the actor being fed,
so the food role stays free after effect-unification), a deferred planner capability.
A **stow** drive acquires a thing and *places* it, and the placing relieves the need.

Stow is the better first drive, and not only for flavor:

- **The relieving action is `put`, already in the affordance table.** So the whole
  loop routes through `perform` with no new verb and no mid-search binding: the drive
  is a pure content slice over the built planner.
- It is a genuine **two-step plan** (`take → put`), so replan-on-veto is reachable in
  principle (smash the nest mid-plan and the container guard vetoes for real), where a
  consume drive's take-only plan never exercises the driver's interesting path.
- It is **non-destructive**: the hoard accumulates as visible world state, a cleaner
  assertion than "the food vanished."

So `eat` and mid-search binding are *not* built here; the magpie needs neither. When
a future drive genuinely wants a consumed resource, that is the step that motivates
mid-search binding, on its own falsifiable footing.

## The loop, on the sim tick

One system (`hoard`) runs the loop, per uncontrolled `Hoarder`, on ticks that are a
multiple of `HOARD_EVERY`, mirroring `wander`'s cadence and its controller check (a
piloted bird halts):

```text
metabolism:   urge = min(urge + 1, MAX)
drive:        goals = hoard_drive(bird)          // 0 or 1 goal
arbiter:      goal  = Arbiter::new(H).select(goals)?
driver:       pursue(bird, goal, known, world, perform-as-beat)
consummation: if Achieved { urge = 0 }           // Abandoned leaves it restless
```

The driver runs the **whole committed plan in this one call**. Interleaving it a beat
per tick (the bird visibly crossing the room over several ticks) is the deferred sim
refinement the [execution](execution.md) doc describes; the replan logic is unchanged,
it is just re-entered per beat instead of looped internally. Whole-plan-per-tick is
the honest first wiring, the same way `wander` runs its whole step on a cadence.

`Abandoned` (nothing within reach to stow) deliberately does *not* relieve the urge:
the bird stays restless and tries again as the world changes. `Achieved` covers both
a real stow and an already-satisfied goal (an empty plan), so both relieve it, which
is the driver's `Progress` doubling as the release cue the arbiter doc describes.

## Two decisions this surfaced

Wiring the loop onto a live creature forced two choices worth recording.

- **Consummation lives in the game, not the planner.** The urge is the bird's own
  component; the planner and driver are world-only and never touch it. So the game
  resets the urge off the driver's `Progress`. This keeps `musce_agency` free of any
  need vocabulary, the same boundary the crate split exists to hold.
- **Cross-tick arbiter commitment is not built (2a).** The `Arbiter` is stateful (it
  holds a committed goal for hysteresis), but sim state lives in the serializable
  world and agency types deliberately do not serialize, and `Arbiter::new` has no
  "resume with this incumbent" seam. With today's single drive there is never a
  challenger, so the loop news a fresh arbiter each tick and re-picks; hysteresis is
  dormant and that is correct. A second, competing drive is what would make
  commitment observable, and that is the change that motivates a small `Arbiter`
  resume API plus a game-owned serializable commitment tag (2b). Until then the seam
  is passed honestly (a real hysteresis band) so adding the second drive needs no
  change at the call site.

## Falsifiability

The oracle is in `hoard.rs` against a real world through the real `perform`: an idle
magpie's urge climbs over sub-threshold ticks with the bead still loose, then at the
threshold the loop plans and runs `take → put` and the bead ends in the nest with the
urge relieved. A controlled bird's urge is frozen and the bead untouched; a bird with
no shiny within reach stays restless (the Abandoned branch). Nothing moves the bead
but the arbiter/driver loop and nothing moves the urge but the metabolism and the
consummation, so a break in any link fails a test.

## Relation to the other docs

- [README](README.md): the agency stack and build order; drives are stack layer 1,
  the last built because falsifiability put them last.
- [arbiter.md](arbiter.md): the layer the drive feeds; the goals here are exactly the
  candidate set `select` ranks, and the imperative-goal seam is the same injection
  point a drive uses.
- [execution.md](execution.md): the driver this loop runs, and the one-beat-per-tick
  interleaving still deferred.
- [planner.md](planner.md): the single-existential goal binding the magpie's goal
  relies on, and the mid-search binding a future *consume* drive would need.
- [../concurrency.md](../concurrency.md): the tick and the system pipeline this loop
  is a system on, beside `wander`.
