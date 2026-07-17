# Affordances, Predicates, and Guards

> Status: **phase A and B built; C not yet built.** The affordance/predicate
> vocabulary now lives in the engine, non-optional, in `musce_action`
> (`affordance.rs`), along with `Guard { clause, reason }` and the
> `Affordance::veto` evaluator (phase B); the optional `musce_agency` crate
> re-exports the vocabulary and keeps the planner-side `CostModel` and `bind_var`.
> Negation (phase C) is still to come. This doc records the promotion decision and
> the phased migration; the `> Status:` flips per phase as each lands.

The affordance vocabulary was built agency-first, on the assumption that only a
planner needs it. That assumption is wrong in a useful way: a verb-gate ("may
this actor do this now, and if not, what do they hear?") exists whether or not
anything plans. The GOAP planner is one *consumer* of that gate, not its owner.
Promoting the vocabulary and a declarative veto into the engine gives verbs and
the planner a *single* precondition to read, and leaves agency as what it should
be: a drive/goal planner on top.

## Why this belongs in the engine

- **Guards are a dispatch concern, not a planner concern.** Gating a verb by a
  declarative precondition is useful to any game, and to non-GOAP automation, with
  no planner in sight. That breadth is the justification for making it
  non-optional, not a violation of minimalism.
- **"The engine owns a kind iff it reads it" is satisfied.** The `veto` evaluator
  is engine code that genuinely *reads* predicates: it iterates a guard's clause
  and calls back into the game-supplied `WorldModel`, the only thing that knows
  what `"contained_by"` or `"container"` mean. The engine interprets no game
  vocabulary itself. A handler calls `veto` where its hand-written precondition
  check used to sit (see the note on *where* the check runs, below).
- **The planner becomes a client, not the home.** `musce_agency` keeps regression,
  `bind_var`, `CostModel`, the arbiter, and drives, and depends on the engine
  vocabulary like any other consumer.

## Layering after promotion

- **Engine, non-optional (`musce_action`):** `Term` / `Predicate` / `Clause` /
  `Affordance` / `Frame`, the `WorldModel` evaluation seam, `Guard { clause,
  reason }`, and the `Affordance::veto` evaluator. This crate already owns the
  `Action` set the predicates mirror and the command dispatch a handler runs
  under, so the types land where the handlers that call `veto` are. (The
  alternative, a new low crate between core and action, was weighed and set aside
  for crate-count minimalism.)
- **Optional (`musce_agency`):** the planner regression, `bind_var`, `CostModel`,
  arbiter, drives. GOAP consumes the engine vocabulary.

## Where the guard check runs: the handler, not `dispatch_command`

The intuitive home for a precondition gate is `dispatch_command`, before the
handler runs. It cannot live there. Evaluating a guard needs a `Frame` of
resolved entities, and turning `"coin in chest"` into entities is *name
resolution*, which is game policy that runs inside the handler. So the engine
provides the guard vocabulary and `Affordance::veto`, and the handler calls it at
the point its hand-written check used to sit, once it has resolved its entities.
This is still the de-duplication that matters: the handler and the planner read
the *same* affordance clause, so they cannot drift on what a verb permits.

A second consequence: name-resolution *scope* already enforces some
preconditions. `put` resolves its item in the actor's inventory, which *is* the
"item is held" guard; by the time the handler builds a frame, that guard is
guaranteed to pass, and the container guard is the one that does real work. The
planner has no resolver, so it needs the full guard set (held *and* container);
the handler evaluating a guard resolution already guaranteed is harmless.

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
- **B. Guard model and the veto evaluator. (Built.)** Added `Guard { clause,
  reason }` and `Affordance::veto`, and replaced `put`'s handler `has::<Container>`
  check with a `veto` call over the same guard clauses the planner reads. Player
  messages are unchanged; the container check now has a single source of truth.
  The check runs *in the handler* after entity resolution, not in
  `dispatch_command` (see "Where the guard check runs"), which the original plan
  had wrong. `take` stayed guard-less: its rule is a negation the vocabulary
  cannot yet express.
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
- [command-dispatch.md](command-dispatch.md): the dispatch path a handler runs
  under. The guard check is *not* a `dispatch_command` gate; it is the handler
  calling `Affordance::veto` after resolving entities (name resolution is game
  policy, so the frame cannot exist before the handler runs).
- [actions.md](actions.md): the structural-only executor stays the commit-time
  backstop that guards deliberately do not replace.
- [engine-and-game.md](engine-and-game.md): the `WorldModel` seam is the new
  game-supplied surface this promotion adds to the engine/game boundary.
