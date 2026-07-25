# Affordances, Predicates, and Guards

> Status: **phases A, B, and C built.** The affordance/predicate vocabulary lives
> in the engine, non-optional, in `musce_action` (`affordance.rs`), along with
> `Guard { clause, reason }` and the `Affordance::veto` evaluator (phase B) and
> engine-owned negation via `Literal { negated, predicate }` (phase C); the
> optional `musce_agency` crate re-exports the vocabulary and keeps the
> planner-side `CostModel` and `bind_var`. `go`'s locked-exit veto is the first
> negated guard. This doc records the promotion decision and the phased migration;
> the remaining agency work (the planner and up) sits on top.

The affordance vocabulary was built agency-first, on the assumption that only a
planner needs it. That assumption is wrong in a useful way: a verb-gate ("may
this actor do this now, and if not, what do they hear?") exists whether or not
anything plans. The GOAP planner is one *consumer* of that gate, not its owner.
Promoting the vocabulary and a declarative veto into the engine gives verbs and
the planner a *single* precondition to read, and leaves agency as what it should
be: a drive/goal planner on top.

## Why this belongs in the engine

- **Guards are a dispatch concern, not a planner concern.** Gating a verb by a
  declarative precondition is useful to any app, and to non-GOAP automation, with
  no planner in sight. That breadth is the justification for making it
  non-optional, not a violation of minimalism.
- **"The engine owns a kind iff it reads it" is satisfied.** The `veto` evaluator
  is engine code that genuinely *reads* predicates: it iterates a guard's clause
  and calls back into the app-supplied `WorldModel`, the only thing that knows
  what `"contained_by"` or `"container"` mean. The engine interprets no app
  vocabulary itself. A handler calls `veto` where its hand-written precondition
  check used to sit (see the note on *where* the check runs, below).
- **The planner becomes a client, not the home.** `musce_agency` keeps regression,
  `bind_var`, `CostModel`, the arbiter, and drives, and depends on the engine
  vocabulary like any other consumer.

## Layering after promotion

- **Engine, non-optional (`musce_action`):** `Term` / `Predicate` / `Literal` /
  `Clause` / `Affordance` / `Frame`, the `WorldModel` evaluation seam, `Guard {
  clause, reason }`, the `Affordance::veto` evaluator, and the affordance's
  authority `gate` (`Affordance::permits`), the act's capability requirement that
  an automation entry checks against the acting verdict, distinct from the gameplay
  guards (see "The gate" below). This crate already owns the
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
resolution*, which is app policy that runs inside the handler. So the engine
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
   Reasons are app content and belong with the verb, never welded into the
   evaluation seam.
2. **Structural invariants only knowable by attempting.** The containment cycle
   ("put the held bag into itself") is not a cheap predicate over app state; the
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
dispatcher evaluates a verb's guards in order via the app's `WorldModel`, and the
first failing `clause` refuses with its `reason`. The planner reads the same
`clause`s and ignores the prose. The bool is an *ingredient* of the guard, not the
whole veto.

`Affordance::veto` returns the whole failing `Guard`, not just its `reason`, so the
caller decides how to use it: today every caller reads `reason` for the player, but
a second audience (an observer, a log) or a later rendering rework can read the
`clause` instead without changing the seam. That keeps the presentation choice with
the caller rather than baking it into the veto's return type.

**What guards do not replace.** The handler still owns the effect (mutation), the
narration, the structural invariants the executor re-checks at commit, stochastic
resolution, and any veto the current vocabulary cannot express. Those stay an
imperative tail. A guard covers the declarative, app-state portion of the veto,
which the [expressibility experiment](agency/affordances.md) showed is a large
share of the common verbs but never all of them.

## The gate: authority, distinct from the guards

An affordance carries an authority `gate: Gate` (`Open`, or `Cap(id)`) alongside
its guards. The two look alike but answer different questions, and the difference
is why they are separate fields rather than one predicate set:

| | guard | gate |
|---|---|---|
| about whom | the acting *body* (actor + objects) | the *principal's* account authority |
| source of truth | world state, via `WorldModel` | the `Verdict`, deliberately not in the world |
| applies to an NPC? | yes, same body, same rule | no, an NPC has no account |
| the planner reads it? | yes, for applicability | no |

Folding the gate into a guard clause would force `WorldModel::holds` to see the
`Verdict` and let an app conflate "is true in the world" with "is authorized", and
would make the planner reason about authority an NPC does not have. So the gate is
its own field, evaluated outside `WorldModel` by whoever performs the act.

The gate lives on the affordance (not only on the command table) so the *act*
carries its requirement to every entry: `perform` checks `Affordance::permits`
against the acting verdict (non-connection automation defaults to `Verdict::guest`,
so a cap-gated act is unavailable until an app hands the automation authority).
A player verb's command keeps its `CommandTable` gate, so both entries express the
same requirement at their own boundary; unifying an act-verb's two gates onto the
affordance is deferred with the admin-verb table. Read-only commands that perform
no affordance (a gated query) keep the `CommandTable` gate as their only home.

## Scope discipline: what we will not build

The generality here is an *emergent property of a clean shape*, not a license to
build surface no verb exercises. Held lines:

- **Negation only, and only where a verb needs it.** The experiment proved
  negation is the one real gap (`go`'s `not Locked`, `take`'s non-fixture rule).
  Disjunction is avoidable with positive markers (`give`'s recipient); value
  comparison appears in zero current verbs. Negation shipped in phase C as a
  `Literal` wrapper (below); `go` and `take` use it, and no other predicate
  operator is built until a verb pulls it.
- **No general rules engine, guard DSL, or event hooks.** Just `Guard { clause,
  reason }` over `Literal`s of the vocabulary we have.

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
  had wrong. `take` stayed guard-less at this phase: its non-fixture rule is a
  negation the vocabulary could not yet express (converted once phase C shipped
  negation; `do_take` now reads a `¬locus ∧ ¬player ∧ ¬creature` guard).
- **C. Negation, when `go` needs it. (Built.)** Added negation as an engine-owned
  `Literal { negated, predicate }` (a `Clause` is now a conjunction of literals),
  *not* a `Predicate::Not` variant: the app's `WorldModel::holds` still answers
  only atomic `Related`/`Tag` questions and never sees `¬`, which the engine
  evaluates in `Literal::holds` as `holds(predicate) != negated`. `go`'s
  `¬ tag(exit, "locked")` veto is now a guard, evaluated in `can_traverse` through
  the same `RefWorldModel` the planner reads; a proven test shows the guard
  predicts `can_traverse`'s permit/refuse and message. Disjunction and value
  comparison stay deferred. (The `Predicate::Not` spelling the earlier draft named
  was set aside because it forces every app's `holds` to implement negation, and
  to implement it correctly; the wrapper keeps boolean logic engine-side.)

The remaining agency work (the planner, drives, and per-actor learning) then sits
on top of a stable engine vocabulary.

## Relation to the other docs

- [agency/](agency/README.md): after promotion, the agency docs narrow to the
  planner, arbiter, and drives. The crate-boundary argument there (the generic /
  app split) still holds, but the split moves from *crate-optional vocabulary* to
  *engine-non-optional vocabulary plus an optional planner*.
- [command-dispatch.md](command-dispatch.md): the dispatch path a handler runs
  under. The guard check is *not* a `dispatch_command` gate; it is the handler
  calling `Affordance::veto` after resolving entities (name resolution is app
  policy, so the frame cannot exist before the handler runs).
- [actions.md](actions.md): the structural-only executor stays the commit-time
  backstop that guards deliberately do not replace.
- [engine-and-app.md](engine-and-app.md): the `WorldModel` seam is the new
  app-supplied surface this promotion adds to the engine/app boundary.
- [gauges.md](gauges.md): the split between stored components (truth: present,
  settable) and gauges (derived, read-only qualitative readings on a bounded
  quantity space that a planner regresses over by direction), and why an effect on
  a gauge is a direction rather than an exact transition.
