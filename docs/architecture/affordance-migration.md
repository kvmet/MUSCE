# Affordance Migration

> Status: **in progress.** The canonical schema/value substrate, typed state
> readers, qualitative gauges, immutable registration, and shared execution are
> built additively. Typed authoring/narration, consumer migration, and the final
> cutover remain pending.

The running engine still carries the smaller prototype that the canonical
affordance system replaces. `musce_action` exposes `Term::{Const, Var}`,
`Predicate::{Related, Tag}`, `Clause`, ordered `Guard`s, a fixed
`Frame { actor, object, target }`, and `Affordance { name, guards, effect }`. There
is no prototype affordance registry or stable affordance id. `musce_ref` constructs
five affordances (`take`, `drop`, `put`, `eat`, `go`); its performer, narrator,
offer mapper, and planner tables join them by name. The web wire remains
`Perform { name, focus, with }`.

`musce_agency` is real, not merely proposed. Its bounded add-only regression
planner supports at most one free variable, its driver replans around vetoed steps
and verifies the live goal after committed beats, and two reference NPC systems
exercise it.

The canonical implementation lands additively beside that runtime until every
entry point can move together. Stable symbolic ids are suitable for schemas and
the wire; each parameter's `slot` is a separate dense array index, so internal
layout cannot silently rename a binding. Typed state readers and gauge evaluators
interpret canonical formulas, and the canonical registry executes them through
`Ctx` or `SystemCtx`; no shipped verb, pointing action, or planner path uses that
registry yet.

The prototype is frozen at its current five affordances. No compatibility layer
becomes a new public design: temporary adapters may translate the existing pointing
envelope during cutover, but new content targets only the canonical schema. The
prototype types and name-keyed joins are removed once text commands, pointing,
scripts, and agency all execute through the immutable registry and shared
performer.

See [affordances.md](affordances.md) for the canonical representation and
[affordance-contracts.md](affordance-contracts.md) for its execution guarantees.
