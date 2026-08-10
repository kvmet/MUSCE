# Affordance Execution Contracts

> Status: **target design specified; implementation pending.** Affordances will
> declare ordered guards and a deterministic, contested, or opaque resolution
> mode. Executable oracles will enforce the behavioral guarantees that schema
> validation cannot prove from Rust handler code.

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

Input grounding precedes guard evaluation. A text handler resolves noun phrases
into a partial input substitution; a pointing client supplies typed values; a
planner unifies an effect with a goal and solves the remaining inputs. Once all
inputs are ground, the shared performer evaluates the gate and guards immediately
before resolution.

A condition whose state slot no registered effect changes still belongs in guards
as a candidate filter. Being unachievable does not justify hiding applicability
policy in Rust.

## Resolution modes

Every affordance declares one mode:

- **Deterministic:** ground inputs plus an admitting gate plus true guards require
  the handler to commit. An ordinary refusal afterward is contract drift.
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

Narration is not an effect. The affordance's typed narrator receives actor, inputs,
and successful results after commitment and emits the shared first- and
third-person account.

## Behavioral oracles

Registration validates the schema but cannot inspect arbitrary Rust behavior.
Each executable affordance therefore has an oracle that:

1. grounds inputs across representative applicable states;
2. proves refusal when each declared guard is false;
3. proves an applicable deterministic affordance commits;
4. validates every returned result sort and required result binding;
5. verifies every advertised state-slot assignment after commitment;
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
