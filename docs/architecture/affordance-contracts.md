# Affordance Execution Contracts

> Status: **registration and shared execution built; behavioral oracles pending.**
> The immutable registry validates the structural and assembled-vocabulary rules,
> builds the initial reverse effect index, and runs grounding, liveness, authority,
> ordered guards, handlers, and result validation through one performer. The typed
> adapter boundary enforces mutation-only execution followed by read-only
> post-commit narration. Redundant-progress diagnostics, advertised-effect
> verification, and executable content oracles remain pending.

Logical planning requires more than effects that describe successful outcomes. It
also requires a contract for when an applicable action succeeds and which effects
are reliable.

## Applicability and guards

An affordance's applicability is a conjunction of conditions. Conditions used for
player-facing refusal are grouped into ordered guards:

```rust
struct Guard {
    condition: Formula,
    reason: &'static str,
}
```

The first unsatisfied guard supplies the refusal reason. The planner reads the same
formula and ignores the prose.

Input grounding and entity-liveness validation precede guard evaluation. A text
handler resolves noun phrases into a partial input substitution; a pointing
client supplies typed values; a planner unifies an effect with a goal and solves
the remaining inputs. Once all inputs are ground and live, the shared performer
evaluates the gate and guards immediately before resolution. A stale or destroyed
input invalidates the proposed action before the deterministic contract's
antecedent is established; it triggers replanning rather than contract drift.

A condition whose state slot no registered effect changes still belongs in guards
as a candidate filter. Being unachievable does not justify hiding applicability
policy in Rust. It must still belong to the canonical condition algebra. Raw
`GaugeTarget` queries are handler facilities, not guards, and cannot serve as a
hidden veto in a deterministic handler.

## Resolution modes

Every affordance declares one mode:

- **Deterministic:** ground inputs whose entity values are live, an admitting
  gate, and true guards require the handler to commit. An ordinary refusal
  afterward is contract drift.
- **Contested:** the same facts establish that an attempt is valid, but resolution
  may fail without committing. The planner may attempt it and replan from live
  state.
- **Opaque:** execution depends on unmodeled rules. The act remains available to
  commands or scripts but is excluded from the planner's effect index.

A deterministic, planner-relevant refusal must therefore be a guard. A
deterministic handler contains calculations, structural commits, and result
construction, not another applicability policy. Contested resolution is the
explicit exception to guaranteed commitment, not a general handler-veto hatch.

`Committed` means the act completed successfully; it need not imply a persistent
world mutation. Speech can commit with empty effects and shared narration while
remaining absent from the planner's reverse effect index.

The structural transition model rejects invalid assignments such as cycles in an
acyclic relation. The planner must reject the same invalid successor; the
structural executor is the commit-time backstop. An executor error in an applicable
deterministic action is a schema, planner, or implementation bug.

## Unconditional effects

Every advertised effect is an unconditional promise of every successful commit.
Conditional or delayed consequences are modeled as reactions to guaranteed state
changes, not as effects that sometimes occur.

For example, an attack may guarantee `ShiftGauge(target, Health, Down)`. A kill
goal targets the fatal health threshold; reaching it triggers a death reaction
that destroys the entity. The attack does not advertise `Destroy(target)` unless
every successful attack destroys it.

A contested affordance may have several outcomes, but every effect in its schema
must hold for every successful committed outcome. An outcome-specific state change
requires an explicit outcome model in a future extension; it is not silently made
conditional now.

Narration is not an effect. `PerformCtx` has no output capability. A typed
definition captures app-owned observations before mutation; after the handler
commits with valid result bindings, its narrator receives those observations,
typed inputs/results, and a read-only `NarrationCtx`. Only that context can stage
the shared first- and third-person account. Refusal and contract-error paths never
invoke it or release output.

## Schema registration

Affordance registration rejects malformed schemas:

- duplicate parameter ids or mode-local slots, and non-dense slot layouts;
- a term referring to an undeclared parameter or local;
- a value sort used in an incompatible condition/effect position;
- an input or result used in an illegal position;
- `Create` not targeting exactly one entity result;
- incompatible assignments to the same state slot;
- both gauge directions on the same gauge slot;
- an effect whose declared state id is not registered;
- a positive `Exists` condition on an entity input whose grounding already proves
  liveness;
- an implementation missing for an executable affordance.

These checks are built in `AffordanceRegistryBuilder`; supplying the handler in
the same registration call makes a missing implementation unrepresentable.
Non-enumerable inputs are valid. A text command or script may supply them; a
planner simply cannot form a grounding while one remains unbound. Redundant
effect/progress analysis remains pending; until it lands, registration rejects
incompatible assignments but does not diagnose an effect already implied by a
guard.

Reverse-index construction omits every effect containing a `Result` term until
fresh-result regression is enabled. This is an indexing rule, not a schema error:
the effect remains part of the execution contract and of assignment-interference
analysis.

Validation happens once at registration. Grounding then uses compact ids and typed
values without repeating schema checks. The pending macro will catch structural
and sort errors knowable during Rust compilation; registration handles errors that
depend on the assembled app vocabulary, such as unknown ids or duplicate
registrations.
The built performer treats a deterministic handler refusal and malformed result
bindings as contract errors. Executable-oracle tests will enforce advertised
post-commit effects that registration cannot infer from arbitrary Rust handler
code.

## Behavioral oracles

Registration validates the schema but cannot inspect arbitrary Rust behavior.
Each executable affordance therefore has an oracle that:

1. grounds live inputs across representative applicable states;
2. proves refusal when each declared guard is false;
3. proves an applicable deterministic affordance commits;
4. validates every returned result sort and required result binding;
5. verifies every advertised state-slot assignment after commitment, including
   the moved root and every post-commit containment descendant for a `SetLocus`
   or `ClearLocus` effect;
6. verifies movement to a strictly different registered region for every
   advertised gauge shift;
7. exercises every successful contested outcome and verifies the common effects;
8. treats structural failure in an applicable deterministic case as a test
   failure.

These tests do not mathematically prove arbitrary code over every world. They make
the intended completeness and soundness obligations executable, reviewable, and
fail-fast when content changes.

## Relation to the other docs

- [affordances.md](affordances.md): canonical inputs, results, terms, and state
  slots.
- [affordance-authoring.md](affordance-authoring.md): macro declarations for
  guards, resolution, handlers, and narrators.
- [agency/planner.md](agency/planner.md): regression that relies on these
  contracts.
- [actions.md](actions.md): atomic structural commit below the handler.
