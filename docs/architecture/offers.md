# Offers: Partial Grounding for Pointing Clients

> Status: **target representation specified; implementation and wire update
> pending.** Offers expose an affordance signature plus a partial typed input
> substitution. The client supplies missing inputs; successful execution may
> return typed results.

A text parser starts with a verb and maps noun phrases onto affordance parameters.
A pointing client starts with an entity and asks which affordances can accept it.
Both are partial-grounding front ends over the same affordance schema.

## A pure read

Offer enumeration is a private query. It performs no mutation, emits no
narration, and triggers no in-world reactions. Active `examine` remains a gameplay
act; passively asking what may be done with an entity does not run it.

The same boundary applies to entity details and containment trees. They are
private projections of world state, distinct from narrated actions that may be
observed or reacted to.

## Parameter-aware offers

An offer carries:

- the affordance id and display name;
- its typed input and result declarations;
- the partial input substitution already supplied;
- the inputs still requiring values;
- the result of evaluating every guard that is ground enough to test.

The selected entity does not implicitly mean `object` or `target`. The app's
offer rule binds it to a declared parameter:

```text
selected chest:
    put.container = chest

selected coin:
    take.item = coin
    put.item = coin

selected exit:
    go.exit = exit
```

Those parameter names are action-local labels carried in-band. A generic client
does not need app-specific knowledge of argument positions.

## Classification

Offer status is:

```text
Available
Needs(inputs)
Vetoed(reason)
```

- `Available` means every input is ground and every declarative guard holds.
- `Needs` lists the unbound inputs the client must ask the user or solver to
  fill.
- `Vetoed` means at least one fully ground guard is false. Its reason comes from
  the earliest declared guard currently proven false; an earlier unground guard
  may later replace that reason.

`Available` means the attempt is applicable. It guarantees commitment for a
deterministic affordance; a contested affordance may still resolve as failure.
Opaque affordances use the same offer classification for their declared guards,
then apply their additional rule during execution.

An unbound input is not a false predicate. A guard is evaluated only when every
term used by that guard is ground. This prevents "choose an item" from being
misreported as "you aren't carrying that."

After each pick, the client sends the expanded partial substitution for
reclassification. A complete input substitution becomes the same `GroundAction`
used by text commands, scripts, and plans.

## Types and candidate choices

Each missing input includes its sort and a presentation label. Entity
inputs may additionally carry candidate ids selected by the app's
knowledge/perception and reachability policy. Non-enumerable text is supplied through text
input; it is never enumerated.

Types prevent invalid wire values, but they do not replace guards. Two inputs
may both be `Entity` while requiring different facts:

```text
hang.item      requires RelationTarget(item, ControlledBy) == Actor
hang.support   requires ComponentPresent(support, HangingSurface)
hang.fastener requires ComponentPresent(fastener, Fastener)
```

The app may narrow a picker from these structural conditions. The final grounded
action still evaluates the full guards before execution.

## Perception is not control

An entity being visible or co-located does not make it usable. Offer enumeration
uses the same structural distinctions as the affordance:

- perception determines whether an entity may be named or selected;
- reachability determines whether physical interaction can be attempted;
- possession and control are explicit relations;
- component and gauge conditions express categorical and ordered requirements.

No generic `available(actor, entity)` predicate collapses these meanings.

## Wire shape

The wire representation mirrors the canonical signature rather than fixing an
arity:

```text
Offer {
    affordance,
    parameters: [ParameterDecl], // includes Input/Result mode
    bindings: [ParameterBinding],
    status,
}

Perform {
    affordance,
    inputs: [ParameterBinding],
}

Performed {
    results: [ParameterBinding],
}
```

Bindings name parameters by their stable schema id and carry a typed wire value.
The client supplies only input bindings; results come only from the successful
server outcome. The server validates ids, modes, sorts, completeness, visibility,
and guards; the client cannot gain authority by manufacturing a binding.

The actor is derived from the authenticated session's live embodiment. It is
never a wire field or caller-supplied parameter binding.

This shape supports any fixed affordance arity, non-entity values, and produced
results without changing the envelope for each new verb. Results appear in an
offer for presentation but never under `Needs`.

## Where policy lives

The engine owns generic schema validation and partial-substitution mechanics. The
app owns:

- which affordances are exposed for a selected entity;
- which parameter receives that entity;
- perception and candidate enumeration;
- concrete guards, refusal prose, and execution.

The division keeps the wire generic without teaching the engine what a chest,
exit, picture, or fastener means.

## Relation to the other docs

- [affordances.md](affordances.md): typed inputs/results, grounding, and guard
  evaluation.
- [agency/preconditions.md](agency/preconditions.md): substitution and candidate
  binding.
- [networking-and-sessions.md](networking-and-sessions.md): transport of queries,
  replies, and perform requests.
