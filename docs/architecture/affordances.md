# Affordances, Predicates, and Guards

> Status: **phase A built; B and C not yet built.** The affordance/predicate
> vocabulary now lives in the engine, non-optional, in `musce_action`
> (`affordance.rs`); the optional `musce_agency` crate re-exports it and keeps the
> planner-side `CostModel` and `bind_var`. The guard-based dispatch veto (phase B)
> and negation (phase C) are still to come. This doc records the promotion
> decision and the phased migration; the `> Status:` flips per phase as each
> lands.

The affordance vocabulary was built agency-first, on the assumption that only a
planner needs it. That assumption is wrong in a useful way: a verb-gate ("may
this actor do this now, and if not, what do they hear?") exists whether or not
anything plans. The GOAP planner is one *consumer* of that gate, not its owner.
Promoting the vocabulary and a declarative veto into the engine makes verb
dispatch itself precondition-aware, and leaves agency as what it should be: a
drive/goal planner on top.

## Why this belongs in the engine

- **Guards are a dispatch concern, not a planner concern.** Gating a verb by a
  declarative precondition is useful to any game, and to non-GOAP automation, with
  no planner in sight. That breadth is the justification for making it
  non-optional, not a violation of minimalism.
- **"The engine owns a kind iff it reads it" is satisfied.** Post-promotion the
  dispatcher genuinely *reads* predicates: it evaluates a verb's guards before
  running the handler body. The engine still interprets no game vocabulary. It
  iterates a clause and calls back into the game-supplied `WorldModel`, which is
  the only thing that knows what `"contained_by"` or `"container"` mean.
- **The planner becomes a client, not the home.** `musce_agency` keeps regression,
  `bind_var`, `CostModel`, the arbiter, and drives, and depends on the engine
  vocabulary like any other consumer.

## Layering after promotion

- **Engine, non-optional (`musce_action`):** `Term` / `Predicate` / `Clause` /
  `Affordance` / `Frame`, the `WorldModel` evaluation seam, `Guard { clause,
  reason }`, and the dispatch-time precondition gate. This crate already owns the
  `Action` set the predicates mirror and `dispatch_command` where the gate lives,
  so the types land where dispatch is. (The alternative, a new low crate between
  core and action, was weighed and set aside for crate-count minimalism.)
- **Optional (`musce_agency`):** the planner regression, `bind_var`, `CostModel`,
  arbiter, drives. GOAP consumes the engine vocabulary.

## The veto model: a guard is a predicate *plus a reason*, not a bool

The tempting move is to make the veto a bool, mirroring `WorldModel::holds`. That
is a regression, because a bare bool cannot carry three things the execution veto
must produce:

1. **The reason.** "It's locked" / "You aren't carrying that" / "You can't put
   things in that" are distinct messages the player needs. `holds` is a bool
   because it answers a *factual* question; a veto answers a *diagnostic* one.
   Reasons are game content and belong with the verb, never welded into the
   evaluation seam.
2. **Structural invariants only knowable by attempting.** The containment cycle
   ("put the held bag into itself") is not a cheap predicate over game state; the
   handler discovers it by attempting the `Move` and catching the executor's
   error. "Try and observe" is beyond predicate evaluation. A proven test pins
   this: `put`'s precondition *permits* the bag-in-bag case while the handler
   *refuses* it, and that divergence is correct.
3. **Stochastic outcomes.** A contested action (combat, a skill roll) resolves by
   a roll. Its precondition may be a simple bool, but its *resolution* is not a
   predicate at all.

So there are two consumers with genuinely different needs, and one representation
should not serve both by being worse at each:

| consumer | question | reads |
|----------|----------|-------|
| planner  | is this applicable? | the guard's `clause` (bool, via `holds`) |
| player   | may I, and why not? | the failed guard's `reason` |
| executor | did the commit hold? | attempt-and-observe (structural backstop) |

`Guard { clause, reason }` unifies the first two without collapsing them: the
dispatcher evaluates a verb's guards in order via the game's `WorldModel`, and the
first failing `clause` refuses with its `reason`. The planner reads the same
`clause`s and ignores the prose. The bool is an *ingredient* of the guard, not the
whole veto.

**What guards do not replace.** The handler still owns the effect (mutation), the
narration, the structural invariants the executor re-checks at commit, stochastic
resolution, and any veto the current vocabulary cannot express. Those stay an
imperative tail. A guard covers the declarative, game-state portion of the veto,
which the [expressibility experiment](agency/affordances.md) showed is a large
share of the common verbs but never all of them.

## Scope discipline: what we will not build

The generality here is an *emergent property of a clean shape*, not a license to
build surface no verb exercises. Held lines:

- **Negation only, and only when a verb needs it.** The experiment proved
  negation is the one real gap (`go`'s `not Locked`, `take`'s non-fixture rule).
  Disjunction is avoidable with positive markers (`give`'s recipient); value
  comparison appears in zero current verbs. So we add `Not(Predicate)` when we
  convert `go`, and defer the rest until a verb pulls it.
- **No general rules engine, guard DSL, or event hooks.** Just `Guard { clause,
  reason }` over the vocabulary we have.

## Phased plan

Each phase is independently falsifiable and reversible until the next begins.

- **A. Move the vocabulary, no behavior change. (Built.)** `Term`/`Predicate`/
  `Clause`/`Affordance`/`Frame`/`WorldModel` moved from `musce_agency` into
  `musce_action` (`affordance.rs`); `musce_agency` re-exports them and the `musce`
  facade exposes them under `musce::action` (non-optional) as well as through
  `musce::agency`. Ground truth held: every existing test green after the pure
  move. `CostModel` / `UnitCost` / `bind_var` stayed on the planner side in
  `musce_agency`.
- **B. Guard model and the dispatch gate.** Add `Guard { clause, reason }` and a
  dispatch-time precondition check. Prove it on `put`: replace the handler's
  `has::<Container>` check with a guard, show the player messages are unchanged
  *and* the planner reads the same clause. This is the de-duplication made real:
  one guard, two consumers.
- **C. Negation, when `go` needs it.** Add `Not(Predicate)`, convert `go`'s
  `not Locked` veto to a guard, prove agreement. Disjunction and comparison stay
  deferred.

The remaining agency work (the planner, drives, and per-actor learning) then sits
on top of a stable engine vocabulary.

## Relation to the other docs

- [agency/](agency/README.md): after promotion, the agency docs narrow to the
  planner, arbiter, and drives. The crate-boundary argument there (the generic /
  game split) still holds, but the split moves from *crate-optional vocabulary* to
  *engine-non-optional vocabulary plus an optional planner*.
- [command-dispatch.md](command-dispatch.md): home of the dispatch-time guard
  gate added in phase B.
- [actions.md](actions.md): the structural-only executor stays the commit-time
  backstop that guards deliberately do not replace.
- [engine-and-game.md](engine-and-game.md): the `WorldModel` seam is the new
  game-supplied surface this promotion adds to the engine/game boundary.
