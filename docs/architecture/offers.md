# Offers: enumerating affordances for a client

> Status: **the query is built and wire-exposed.** In `musce_ref` (`offers.rs`)
> the enumeration returns the affordances available on an entity, each annotated;
> `pointing.rs` projects them to the `musce_proto::web` DTOs behind the
> `Game.offers` seam, and the WebSocket read `Query::Offers` round-trips the sim
> thread to reply with them (see
> [networking-and-sessions.md](networking-and-sessions.md)). The type filter noted
> below is still proposed.

A text parser never needs to *enumerate* affordances: a player types a verb and
the parser resolves it straight to one handler. A pointing client (click, tap,
examine) does need it. It holds an entity and asks the game "what can I do to
this?", so it can render a live control, a greyed one with the reason, or a
prompt for a still-missing piece. This is the renderer-side consumer the
[affordances doc](affordances.md) foreshadowed when it noted a second audience
could read a guard's `clause` where the handler reads its `reason`.

## A pure read, not a verb

Enumeration is a private read: no world mutation, no audience, no narration. It
must **not** be wired to the `examine` verb. `examine` is an in-world act that can
narrate ("the adventurer peers at the chest") and later trigger reactions; routing
a curious click through it would spam the room and fire side effects on every
poke. So passive inspection (description + offers, private to that player) is kept
distinct from active, narrated `examine`. `affordances_on` is a `Ctx`-free
function over `&World` precisely so it stays off the command/event path by
construction.

The same split governs the tree the client shows: "what is here" is
`World::contents` / `container_of` / `enclosing_locus` rendered as nesting, an
existing read over containment, not new world state. Each node also carries a
passive **detail bag**: game-projected `(label, value)` pairs an actor perceives by
presence (today its `Description`), so a focused entity renders without a second
round-trip. That bag is the passive-inspection half of the split above, delivered
as read data; the narrated `examine` act reveals the same prose but broadcasts and
can trigger reactions, and lands with the `perform` slice. Whether the tree reveals
the contents of a *closed* container is a visibility layer that does not exist yet
(`Container` is a bare tag); for now it shows all contents.

## Three shapes the query forces, that `veto` alone did not

`Affordance::veto` answers one question about one fully-bound frame: does a guard
fail, and if so which. Enumeration needs three things it cannot give:

1. **Which role the pointed-at entity fills** (`focus_role`). The parser's name
   resolution and `perform`'s match arms both know this implicitly; a
   resolver-less client has neither, so the convention is stated: `put`/`go` act
   on a `target` (a container, an exit), the rest on an `object`.
2. **Unbound-role is not veto.** An unbound role-`Var` reads as a false predicate
   (`WorldModel::holds` answers a free `Var` as not-held), so bare `veto` on `put`
   with nothing chosen to put would report "You aren't carrying that" when the
   truth is "pick something". So role-completeness is checked *before* `veto`, and
   the status is three-way (`OfferStatus`): `Available`, `Vetoed(reason)`, or
   `NeedsRole(role)`. `NeedsRole` is the sub-pick the client opens (choose the
   object once the container is picked); re-classifying the now-complete frame
   yields `Available` or the real guard `Vetoed`.
3. **Required roles are free; the parser's type filter is not.** The roles a
   client must supply are recovered by scanning the affordance's *guards* for
   role-vars (`required_roles`), so no new field on `Affordance` is needed: arity
   is already latent in the clauses. Guards, not the effect, because a guard names
   an entity whose state must be validated, while an effect may name a *derived*
   destination the game fills itself (`drop`'s target is the actor's room, never a
   pick). What is *not* recovered is the parser's implicit kind gate: `go north`
   resolves to an exit by construction, but a click does not, so the query offers
   `go` on a rock (not locked) and `take` on a chest (not a being). Recovering
   that filter (a declared focus-role kind, or leaning on click context) is a
   deferred design decision, pinned by a characterization test so the eventual
   filter is deliberate, not accidental.

   The resolution scope's *reachability* half **is** recovered, though, because it
   is a safety boundary, not a nicety: a manipulated object must be `reachable`
   (held by the actor or loose in the actor's locus), which perception alone is not.
   Perception spans the whole locus subtree, so an item nested in another creature's
   inventory is visible; without the reachability gate a click could take it, which
   the text path's room-scoped resolution never allows. This is one containment
   level, distinct from the deferred kind filter: a reachable wrong-kind object (a
   chest on the floor) still classifies by its guards.

Points 2 and 3 are the same gap the planner has, from a new angle: a
resolver-less consumer needs the full guard set and loses the resolution scope
that silently enforced some preconditions (see the affordances doc's note on name
resolution as an implicit guard). The click UI is the third such consumer, after
the handler and the planner.

## Where it lives

`offers.rs` is game content in `musce_ref`, like `perform`: it names the concrete
affordance set and reads them through `RefWorldModel`. The *classification*
(`classify`, `required_roles`, `focus_role`) is generic and could promote into
`musce_action` once a second game wants it, the same promotion discipline the
affordance vocabulary itself followed; it stays in the reference game until that
second consumer exists.

## Relation to the other docs

- [affordances.md](affordances.md): the veto model this reads. `OfferStatus` is
  the "richer renderer reads the `clause`" audience that doc anticipated.
- [networking-and-sessions.md](networking-and-sessions.md): the wire form, now
  built. Because enumeration is a read, it rides a `Query`/`Reply` message pair
  distinct from the command/event action path, over the WebSocket transport a
  browser client needs.
- [agency/README.md](agency/README.md): the planner, the other resolver-less
  consumer of the same guards, which required the full guard set for the same
  reason enumeration does.
