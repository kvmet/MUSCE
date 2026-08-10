# Affordance Migration

> Status: **in progress.** The canonical schema/value substrate, typed state
> readers, qualitative gauges, immutable registration, typed execution/narration,
> runtime injection, generic pointing offers, and typed perform wire are built.
> The reference `give` command and pointing surface are canonical; text and agency
> consumers and the final prototype removal remain pending.

The running engine still carries the smaller prototype that the canonical
affordance system replaces. `musce_action` exposes `Term::{Const, Var}`,
`Predicate::{Related, Tag}`, `Clause`, ordered `Guard`s, a fixed
`Frame { actor, object, target }`, and `Affordance { name, guards, effect }`. There
is no prototype affordance registry or stable affordance id. `musce_ref` constructs
five affordances (`take`, `drop`, `put`, `eat`, `go`) for text and agency consumers;
their performer, narrator, and planner tables join them by name. `give` is outside
that path: its text verb and pointing exposure ground the canonical typed
definition directly. The web wire is wholly canonical and carries named typed
bindings; it has no fixed frame or compatibility adapter.

`musce_agency` is real, not merely proposed. Its bounded add-only regression
planner supports at most one free variable, its driver replans around vetoed steps
and verifies the live goal after committed beats, and two reference NPC systems
exercise it.

The canonical implementation lands additively beside that runtime until every
entry point can move together. Stable symbolic ids are suitable for schemas and
the wire; each parameter's `slot` is a separate dense array index, so internal
layout cannot silently rename a binding. Typed state readers and gauge evaluators
interpret canonical formulas. The host builds the canonical registry after world
type registration and injects it into every `Ctx` and `SystemCtx`. The shipped
`give` verb uses it for its ordered guards, contested structural resolution,
containment effect, and post-commit narration. Pointing proposals are classified
against that schema, and complete client groundings execute through that same
performer. The planner path still uses the prototype.

The text verb only resolves `item` and `recipient`. The typed definition owns the
held-item, explicit gift-recipient, distinct-recipient, and shared-locus guards;
advertises the containment assignment; commits the `Move`; and narrates from
observations captured before mutation. It is contested rather than deterministic
because the closed condition algebra cannot express transitive containment
ancestry: an otherwise applicable adversarial world can put the recipient below the
held item, and the acyclic executor must then refuse.

The prototype is frozen at its current five affordances. No compatibility layer
is part of the public design; new content targets only the canonical schema. The
prototype types and name-keyed joins are removed once text commands, pointing,
scripts, and agency all execute through the immutable registry and shared
performer.

See [affordances.md](affordances.md) for the canonical representation and
[affordance-contracts.md](affordance-contracts.md) for its execution guarantees.
