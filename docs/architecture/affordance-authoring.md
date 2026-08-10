# Affordance Authoring Language

> Status: **target design specified; implementation pending.** App affordances
> will normally be declared through a Rust `affordance!` procedural macro that
> lowers into the canonical representation in
> [affordances.md](affordances.md).

The authoring language resembles a typed function signature with a logical
contract around an ordinary Rust implementation. It removes parameter-id and
type-erasure bookkeeping without creating another semantic model.

## Declaration shape

```rust
affordance! {
    hang(
        item: Entity,
        support: Entity,
        fastener: Entity,
    ) {
        requires {
            item.has_component(Picture) => "That is not a picture.";
            support.has_component(HangingSurface)
                => "That cannot support a hanging.";
            related(item, Actor, ControlledBy)
                => "You do not control that item.";
            related(fastener, Actor, ControlledBy)
                => "You do not control that fastener.";

            exists(locus: Entity) {
                at_locus(Actor, locus);
                at_locus(support, locus);
            } => "You cannot reach that support.";
        }

        effects {
            set_relation(item, MountedOn, support);
        }

        resolution Deterministic;
        execute musce_ref::act::hang;
        narrate musce_ref::act::narrate_hang;
    }
}
```

The signature declares action-local parameters and their sorts. `Actor` is
privileged, supplied by the execution context, and therefore absent from the
parameter list. A formula refers to it explicitly as `Actor`.

Each `requires` entry is an ordered guard written as
`condition => refusal_reason`. Together the conditions form the affordance's
conjunctive precondition. Execution reports the first failed guard in declaration
order; the planner reads only the conditions, and the refusal prose never
participates in unification.

`effects` contains unconditional planner-visible promises of every successful
commit. `resolution` declares whether true guards guarantee commitment, permit a
contested failure, or leave the act opaque and non-plannable. `execute` names the
ordinary Rust function that commits the act and returns result bindings. `narrate`
optionally names the shared typed narrator invoked after a commit.

For `Deterministic`, the macro adapter and runtime enforce this contract:

```text
ground inputs + admitting gate + true guards => successful commit
```

The handler cannot add an ordinary gameplay veto. A structural executor error or
refusal after the antecedent holds is contract drift and fails executable-oracle
tests. `Contested` explicitly permits a valid attempt to fail without committing;
`Opaque` is executable but absent from the planner's effect index.

## Generated Rust interface

The macro expands to ordinary Rust containing:

- a factory for the canonical affordance value and its stable parameter
  declarations;
- typed input and result structures such as `HangInputs` and `HangResults`;
- an adapter that sort-checks canonical values and invokes the typed handler;
- an adapter that invokes the typed narrator with the same values;
- registration metadata used by commands, offers, scripts, and planning.

The generated adapter supplies the actor through `Ctx`, separately from the typed
inputs. Conceptually the implementation has this shape:

```rust
struct HangInputs {
    item: EntityId,
    support: EntityId,
    fastener: EntityId,
}

struct HangResults {}

fn hang(ctx: &mut Ctx, inputs: HangInputs) -> PerformResult<HangResults>;

fn narrate_hang(
    ctx: &NarrationCtx,
    inputs: &HangInputs,
    results: &HangResults,
) -> Narration;
```

The exact public result type follows the shared affordance execution API. The
stable guarantee is that app code receives typed fields instead of indexed raw
value arrays. The canonical `GroundAction` carries inputs beneath the adapter;
`ActionOutcome` carries results. The narrator references generated fields rather
than fixed participant roles, so arbitrary arity does not reintroduce a frame.
`NarrationCtx` exposes the actor, pre-commit observations captured by the shared
performer, and post-commit world/audience access. Movement narration can therefore
address both the vanished departure locus and the resulting arrival locus.

## Result parameters

Results use function-return syntax:

```rust
affordance! {
    craft(material: Entity) -> (product: Entity) {
        requires {
            related(material, Actor, ControlledBy)
                => "You do not control that material.";
        }

        effects {
            create(product);
            product.set_component(CraftedItem);
            set_relation(product, ControlledBy, Actor);
        }

        resolution Deterministic;
        execute musce_ref::act::craft;
        narrate musce_ref::act::narrate_craft;
    }
}
```

Inputs must be bound before execution. Results cannot appear in `requires`, are
absent from perform requests and `Needs`, and must be returned on every successful
path. Effects and narration may reference them. Planning through `Create` remains
deferred, but the schema, generated types, and wire distinguish results now.

Non-entity sorts use the same signature form:

```rust
affordance! {
    say(text: Text) {
        requires {
            Actor.has_component(Voice) => "You cannot speak.";
            not(Actor.has_component(Muted)) => "You are muted.";
        }

        effects {}

        resolution Deterministic;
        execute musce_ref::act::say;
        narrate musce_ref::act::narrate_say;
    }
}
```

The generated `SayInputs::text` is a typed text value. Its canonical input is
still subject to the grounding rule that non-enumerable text must be supplied rather than
invented by the planner.

## Closed logical vocabulary

The macro accepts only constructs that lower into the canonical condition and
effect algebra. It cannot declare an arbitrary predicate callback. Relation,
component, and gauge names select registered app vocabulary; they do not add new
logical operators.

Conveniences are permitted as syntax-level expansions. For example:

```rust
same_locus(Actor, item) => "You cannot reach that.";
```

is valid only if schema construction expands it into canonical conditions and an
existential local, such as:

```rust
exists(locus: Entity) {
    at_locus(Actor, locus);
    at_locus(item, locus);
} => "You cannot reach that.";
```

The registered schema and planner retain the expansion, never the helper name.
`at_locus` is canonical: it reads the engine's derived `LocusOf` slot through
`enclosing_locus`. Relation syntax similarly lowers to equality over the
source-functional `RelationTarget(source, kind)` slot; `set_relation` assigns that
slot and `clear_relation` clears it. This boundary keeps effects and conditions
statically comparable.

## Validation boundary

The procedural macro reports errors knowable from one declaration during Rust
compilation, including duplicate parameter names, undeclared names, illegal local
or result use, and operations incompatible with a parameter sort.

Registration validates facts that depend on the assembled app vocabulary,
including unknown relation/component/gauge ids, duplicate affordance ids, missing
implementations or narrators, contradictory slot assignments, and registry-wide
constraints. Grounding validates supplied input values against the
already-registered signature. Behavioral oracle tests enforce deterministic
commitment and advertised effects; a macro cannot prove arbitrary Rust behavior.

## Other authoring languages

The canonical AST is independent of macro syntax and remains directly
constructible by engine internals and tests. A future data format, Lua surface, or
Lisp-like language may target the same AST when hot reload or non-Rust content
authors justify a separate parser and runtime.

Such a language must preserve the same sorts, closed algebra, guard ordering,
effect grounding rules, and registration validation. It is another authoring
front end, not a second execution runtime or planner model.
