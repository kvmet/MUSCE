# Drives, and the live agency loop

> Status: **built (two competing drives, live wiring, cross-tick commitment).** The
> reference game's magpie (`musce_ref`, `hoard.rs`) is the first autonomous agent, and
> it runs *two* competing drives: a `Hoarder` need it relieves by stowing a shiny in
> its nest, and a `Curiosity` need it relieves by holding one. They pull the same bead
> in opposite directions, so the arbiter must commit to one and hold it, which is what
> makes hysteresis observable and what motivated **cross-tick commitment**
> (`Arbiter::resume` plus a serializable `Committed` tag). A *consume* drive and
> per-beat interleaving remain deferred, below.

A **drive** turns an NPC's internal need-state into a goal with an urgency. It reads
the NPC's *own* components, never the world or its beliefs: "am I hungry / restless /
covetous" is a reading of the self. The arbiter ranks the goals drives emit and commits
to one; the planner and driver carry it out. The magpie closes this loop on a real
creature, on the sim tick, through the same `perform` a player hits.

## Two drives, one bead

A drive is unfalsifiable until a need-component actually *changes* and some action
*relieves* it: with a frozen need and no consumer, nothing distinguishes a working
drive from a constant. The magpie supplies two needs whose relief pulls one object
opposite ways, which is also what makes the *arbiter* falsifiable:

- **Hoard** is `Hoarder { urge }`. Its goal is `∃x. related(x, nest, contained_by) ∧
  tag(x, shiny)`: get some shiny thing into the nest. The relieving act is `put`.
- **Admire** is `Curiosity { itch }`. Its goal is `∃x. related(x, actor, contained_by)
  ∧ tag(x, shiny)`: hold some shiny thing. The relieving act is `take`.

The two goal predicates differ only in the container (the nest, a constant the bird
reads from its own `Nest` edge, versus the actor itself), so they are never equal: the
arbiter can hold one as an incumbent while the other challenges, and the commitment
record round-trips unambiguously by predicate. `x` is the fungible slot the planner
binds against what the bird knows; this is the single-existential goal the planner
already binds (planner step 4b), with `tag(x, shiny)` filtering candidates and
regression planning the containment.

With both needs pressing, the bead is wanted in the nest and in the claws at once.
Without a held commitment the bird yo-yos it between the two every tick or two; the
arbiter's job is to finish one phase before switching. That thrash-versus-commitment
contrast is the oracle (below).

## Symmetric, state-based relief

Each need moves on its own curve, updated by a **metabolism** step, and the drive reads
only the resulting component. The metabolism is the one place that reads the world:

```text
urge:  shiny in nest ? cool toward 0 : warm toward MAX
itch:  holding shiny ? cool toward 0 : warm toward MAX
```

Two properties of these curves are load-bearing, and both were chosen deliberately
after a first cut got them wrong:

- **Relief is gradual and symmetric, not an instant reset.** A satisfied need cools one
  step per tick rather than snapping to zero. That gives the satisfied drive a
  multi-tick window in which it *still offers its goal*, so the two drives genuinely
  contend while one is being served. An instant reset would drop the just-served drive
  out of the running entirely, leaving nothing for the arbiter to hold a commitment
  *against*, which would make hysteresis untestable (both a committed and an
  uncommitted bird would behave the same).
- **The cool floor (zero) sits below the drive threshold.** So a served need eventually
  falls silent, the drive retires, and the arbiter re-picks. This guarantees the bird
  cannot deadlock holding a commitment forever; the pursuit always terminates.

Satisfaction is read from the world *here*, in the metabolism (the bead's location),
never in the drive and never in the arbiter. The drive still reads only its own
component, preserving "a drive reads the self, not the world."

## What the bird can perceive

The planner binds the fungible `x` from a **known** set. The engine's `known_here` seed
is co-located room contents only. A magpie needs more: to stow the bead it is holding,
or to reclaim one from its nest, it must *perceive* its own inventory and its own cache,
neither of which is a room content. So the magpie supplies its own seed, `known_here ∪
its inventory ∪ its nest's contents`.

This is game policy, not an engine guarantee. A creature is not assumed to see into
every container it touches (a locked box it carries would not qualify); the magpie
knows its *own* claws and its *own* nest because those are its own state. General
perception into arbitrary containers, and sense-propagation at range, stay a deferred
layer.

## Why *stow* and *hold*, not *consume*

The reference verb set (`take`/`drop`/`put`, no `go`) supports drives that acquire and
*place* a thing, where the placing relieves the need. It does not yet support a
**consume** drive (hunger → eat), where a new consumer verb destroys the thing: routing
that through the planner needs mid-search existential binding (the consumer's effect is
about the actor being fed, so the food role stays free after effect-unification), a
deferred planner capability. Both magpie drives are place-drives:

- **Their relieving acts, `put` and `take`, are already in the affordance table.** The
  whole loop routes through `perform` with no new verb and no mid-search binding.
- The hoard goal is a genuine **two-step plan** (`take → put`), so replan-on-veto is
  reachable (smash the nest mid-plan and the container guard vetoes for real).
- They are **non-destructive**: the bead's location is visible world state, a cleaner
  assertion than "the food vanished."

When a future drive genuinely wants a consumed resource, that is the step that motivates
mid-search binding, on its own falsifiable footing.

## The loop, on the sim tick

One system (`hoard`) runs the loop, per uncontrolled magpie, on ticks that are a
multiple of `HOARD_EVERY`, mirroring `wander`'s cadence and its controller check (a
piloted bird halts):

```text
metabolism:   move both needs from the world (rise while unmet, fall while met)
drives:       goals = [hoard_drive(bird), admire_drive(bird)]   // 0, 1, or 2 goals
arbiter:      goal  = commit_and_select(bird, H)                // resume + persist
driver:       pursue(bird, goal, magpie_known, world, perform-as-beat)
narrate:      a beat that moved something, by the drive it served
```

`commit_and_select` is where cross-tick commitment lives (next section). The driver
runs the **whole committed plan in this one call**; interleaving it a beat per tick is
the deferred sim refinement the [execution](execution.md) doc describes. There is no
separate consummation step any more: relief is the metabolism reading the world next
tick, not a reset fired off the driver's `Progress`.

## Cross-tick commitment (built)

Hysteresis only bites across ticks, but the sim's persisted state is the serializable
world, and agency types (`Goal`, `Clause`, `Arbiter`) deliberately do not serialize. So
the bird does not keep an arbiter alive between ticks; it records **which drive** it
committed to as ordinary world state, a `Committed(Drive)` component, and rebuilds the
arbiter each tick from it:

1. Gather this tick's goals, each labeled by its drive.
2. Map the persisted `Committed` tag to *this tick's* goal from that drive, the live
   incumbent, or `None` if that drive has gone quiet, so a stale tag never revives a
   retired goal.
3. `Arbiter::resume(hysteresis, incumbent).select(goals)` matches the incumbent into
   the candidate set by predicate exactly as an in-run commitment is, and applies the
   band.
4. Map the chosen goal back to its drive and write `Committed`.

`Arbiter::resume` is the whole engine-side seam this needed (`new(h)` is now just
`resume(h, None)`); everything else is game content. The loop never calls
`Arbiter::release`: a served need expresses itself as fading urgency, and when it drops
below threshold the drive stops offering, so the arbiter retires the incumbent on its
own. `release` stays for the explicit, event-driven drop an imperative order uses.

## Two decisions this surfaced

- **Consummation lives in the game, not the planner.** The needs are the bird's own
  components; the planner and driver are world-only and never touch them. So the game
  moves them, in the metabolism, off the world state the driver leaves behind. This
  keeps `musce_agency` free of any need vocabulary, the boundary the crate split holds.
- **Cross-tick commitment is a game-owned tag plus one arbiter seam.** The stateful
  arbiter cannot persist, so the serializable half (which goal) lives on the bird and
  the logic half (the band, the matching) stays in `musce_agency`, reached through
  `resume`. This is the resolution of the tension the single-drive wiring first
  surfaced: the arbiter is reconstructed, not stored.

## Falsifiability

The oracles are in `hoard.rs`, against a real world through the real `perform`:

- **The arbiter earns its keep** (`commitment_stops_the_two_drives_thrashing_the_bead`):
  with hoard and admire both pressing, a committed bird moves the bead far less than a
  no-commitment control (a fresh zero-band arbiter each tick) that thrashes it nearly
  every tick, while still serving both needs (the bead is both held and stowed over the
  run). The two arms share the same metabolism, drives, and pursuit, so the move-count
  gap is attributable to commitment alone.
- **The hoard drive, live** (`an_idle_magpie_grows_restless_then_stows_a_shiny`): an
  idle bird's urge climbs sub-threshold with the bead loose, then at the threshold the
  loop plans and runs `take → put`, the bead lands in the nest, and the urge cools as
  the hoard rests there.
- **The halts** (`a_controller_halts_it`, `nothing_to_steal_leaves_it_restless`): a
  piloted bird is frozen and the bead untouched; a bird with no shiny within reach stays
  restless (the Abandoned branch never relieves the need).

Nothing moves the bead but the arbiter/driver loop and nothing moves a need but the
metabolism, so a break in any link fails a test.

## Relation to the other docs

- [README](README.md): the agency stack and build order; drives are stack layer 1,
  built last because falsifiability put them there.
- [arbiter.md](arbiter.md): the layer the drives feed; the goals here are exactly the
  candidate set `select` ranks, and `resume` is the cross-tick-commitment seam.
- [execution.md](execution.md): the driver this loop runs, and the one-beat-per-tick
  interleaving still deferred.
- [planner.md](planner.md): the single-existential goal binding both drives rely on, and
  the mid-search binding a future *consume* drive would need.
- [../concurrency.md](../concurrency.md): the tick and the system pipeline this loop is
  a system on, beside `wander`.
