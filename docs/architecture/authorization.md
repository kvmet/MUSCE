# Authorization: capabilities, superuser, and quell

> Status: **slice 1 (authorization) built; slice 2 (authentication) not started.**
> The capability model, the superuser bit, `@quell`, and the account authority are
> live: `Gate` is `Open | Cap` in `musce_action`, the authority resolves a
> connection's account to a verdict at the dispatch seam, and the reference game
> re-gates its admin verbs on capabilities. What remains for slice 2 is real
> authentication (passwords, tokens, oauth) behind the same store seam; until then a
> connection is a guest unless it elevates through the loopback-only `@operator` stub.

This covers the permission **model**: **who may do what**. The account as the bearer
of permissions, the capability model that gates verbs, the superuser bit, and how to
set it aside. The machinery that makes it real (resolving a connection to a verdict at
dispatch, the account authority and its store, bootstrapping) is the implementation
half in [accounts.md](accounts.md). This does **not** cover authentication (proving
identity: passwords, tokens, oauth), a later slice behind the same store seam, nor
session identity and embodiment, which live in
[networking-and-sessions.md](networking-and-sessions.md).

## The engine authorizes accounts, nothing finer

Account identity is the only permission-bearing thing the engine knows. A
**character** is game vocabulary the engine never interprets, so anything finer than
the account is not a lighter version of this system: it is a **different mechanism**,
expressed as ordinary validation inside a game verb handler, exactly where `pilot`'s
"you may only pilot what you control" and the takeable rule already live.

This line resolves several questions at once. A **per-character** permission (may
this warrior cast) and a **scoped** permission (may I build *in this zone*) are both
finer than the account, so both are game inline validation, not engine capabilities;
the engine's grant set stays flat and global, and scope never enters it. `Gate`
gates a verb on an account capability, full stop, and must not grow into a
per-entity or per-scope framework. So that the game can write those inline checks
without flying blind, the resolved permission verdict (see [accounts.md](accounts.md))
is **available to game handlers**: an inline rule can ask "is this actor su / does it
hold cap X" and decide accordingly.

## Capabilities, not tiers

There is no `guest < builder < admin` ladder; a linear hierarchy cannot express
someone who edits rooms but cannot ban players. The model is composable:

- A **capability** is an atomic named grant (`build.room`, `teleport`, `ban`),
  **game vocabulary**: the game defines which capabilities exist and mean.
- A **role** is a named bundle of capabilities, **pure game config**. The engine
  never sees roles; the game expands a role to its capabilities and the engine sees
  only the resolved set. Roles never reshape the persisted account.
- An **account holds a flat set of grants**, persisted **by string name** (stable,
  mirrors `NamedComponent::TAG`), resolved to fast ids at load. The set (not a tier)
  is the decide-now cardinality; composability requires it.

`Gate` generalizes from `Open | Staff` to `Open | Cap(CapId)`. The `Staff`
world-marker **retires**, and the reference seed's `Staff` on the avatar body is
stripped in the same change (leaving it is a possession-borrow escalation, below).
The placement test applied straight: the game defines the vocabulary, the engine
owns only the membership check.

A capability is a **string, not a Rust type**, so it cannot register the way
components and relations do (those key off `NamedComponent::TAG`, a type). The game
declares its cap vocabulary through a **caps registry**, a `register_cap("build.room")
-> CapId` that interns each name to a stable `CapId` (like `register_relation`
interns a name to an id, but a string-keyed interner on the caps registry, not the
type-keyed table on `World`; a cap is a runtime string, not a type). `Gate::Cap` holds that interned id, taken **at table-build time** when the
game builds its command tables; an account's persisted grant strings resolve against
the **same** registry at load, so a gate's `CapId` and a grant's `CapId` denote the
same cap. An unknown grant string at load is an error, never a silent drop. This
registry is not deferrable behind the version seam: slice 1's very first `Gate::Cap`
verb needs a `CapId` in hand, so the registry ships in slice 1.

## Superuser is an account bit, not a capability

The engine reserves one concept, `superuser`, as a **boolean property of the
account**, not a capability in the grant set. The effective check is:

```
permits(cap) = (account.is_su && !connection.quelled) || account.caps.contains(cap)
```

su is a peer bit checked first, not a set member. A bit rather than a reserved
capability is load-bearing:

- **It kills role-escalation structurally.** Roles expand only into `caps`; a bit is
  not nameable in game vocabulary, so "a role cannot grant su" needs no runtime rule
  on the wrong side of the boundary.
- **It stays honest and enumerable.** A set that must answer `true` for game caps the
  engine has never heard of is a bypass wearing a set's clothes; the bit is what it
  is, and an account's actual grants stay listable.
- **Its source is operator-only**, settable only through out-of-band account
  administration (below), never through grant or role expansion. (The one other
  writer, the boot promotion in [accounts.md](accounts.md), is constrained so it
  cannot become an escalation.)

**What su bypasses, and what it does not.** su bypasses **gates** (`Gate::Cap`
checks), so an su account passes any gated verb, including the admin verbs, which are
pure gates with no inline rule (intended: su is total on the builder surface). It
does **not** bypass game **rules**, the inline handler validation above. That
distinction is a property of the **player-verb surface**, not a global guarantee:
whether su overrides a given restriction depends on which side the game author coded
it. A "who may" check the author wants su to bypass belongs on the gate (a cap); a
genuine world rule even su obeys stays inline. Because scoped authority cannot be a
flat cap it lands inline, so an author who wants su to bypass a scoped check reads
the authority verdict there and lets su through explicitly. The engine supplies the
verdict; the placement is the game's call.

## Quell: setting su aside for a connection

A superuser usually does not want god-mode live: it invites accidental edits and
hides how the game plays for a normal account. `@quell` toggles a **per-connection
suppression flag**, the `!connection.quelled` term above, that drops the su bit from
the effective check. Quelled, an su account is evaluated purely on its actual `caps`,
playing as exactly the authority those grants describe.

Quell is **per-connection and ephemeral**: each new connection starts un-quelled, so
opening a fresh connection reliably restores full su and there is no lockout. A
second concurrent connection on the same account is quelled independently, so an
operator who quells one connection still has su live on another; this is a footgun,
not an escalation (it is the operator's own account), and is documented rather than
prevented. Reconnect resetting the flag is the never-locked-out guarantee, so quell
is deliberately *not* account-durable session state that survives disconnect.
Suppressing su is the whole MVP; a finer grain (per control-stack, or an arbitrary
reduced grant set) is an additive later refinement with no present consumer,
deferred.

`@quell` is a **floor command handled in the host loop, like `@quit`**, not a
`CommandTable` entry: it writes host-only `Session` state, which a `CommandTable`
handler (`fn(&mut Ctx, &str)`, and `Ctx` has no session handle) cannot reach. Its
rationale for being floor is narrow and exact: `connection.quelled` is a term in the
engine's own `permits()` evaluation, read nowhere else, so the command that toggles
it is engine mechanism. Un-quell is the same ungated floor command, **never behind a
cap**, or a quelled su whose actual caps lack that cap would be stranded. (One
consequence to accept: a quelled su lacking a possess cap cannot `@unpossess` a
puppet it took while un-quelled until it un-quells; minor, and un-quell is always one
floor command away. If a later slice moves possession lifecycle to the floor, this
footgun vanishes; re-read it then.)

## Recovery is out of band

A game rule can wedge the world (every exit `Locked`, no key entity exists). su
bypasses gates but not rules, so su cannot type its way out with an in-game verb, and
this is correct: the engine ships no builder command table to smuggle a god-verb
into. A wedged world is un-wedged **out of band**, over the `execute(world, Action)`
seam that is already engine-owned, run **offline against a stopped world** (a
maintenance mode, not the live sim). Running it against the live sim would be a second
writer holding `&mut World`, the exact single-writer invariant atomicity rests on, so
recovery is explicitly the offline path. That surface is present in every game because
`execute` is engine, so su has a recovery path without the engine owning a command.

A bad lock is a verb-or-data problem, so fixing it at the code/data layer offline is
the right layer. A developer who could author a self-wedging lock could already do
anything with code access, so the out-of-band fix concedes nothing. The tool is
deferred; the architectural commitment (no engine in-game verbs, recovery is offline)
is made now.

## Modifying su is out of band

The su bit has **no in-band write path, ever**: no verb, no floor command, no role
expansion. It changes through the store, out of band; with the trivial slice-1
backend that means edit the store and restart. This is the commitment, not a
stopgap: the set of people who can bypass gates is controlled only from outside the
game, so no in-game surface can be misused into minting one.

Ordinary cap grants are not under that commitment; they simply ship no runtime
surface in slice 1 (the seed plus offline store edits). Whatever live surface later
arrives (the slice-2 writer, or a floor command if one is wanted sooner) obeys two
constraints: it cannot be a `CommandTable` verb (the verdict on `Ctx` is a
read-only snapshot; the same line that keeps `@quell` on the floor), and it can
never write the su bit.

A game that wants runtime-grantable authority of its own builds it as **game
vocabulary**, per the finer-than-account line above: game state checked by inline
rules (the retired `Staff` pattern returning deliberately, as a character marker or
any game-side record), granted live through its own verbs like anything else in the
world. That mechanism is fully in-band and fully the game's; the one thing it can
never reach is the su bit, because nothing in-band can.

One scope note: account **creation** is still a runtime store mutation once slice-2
registration exists, so the post-image su floor and the first-account-su promotion
(see [accounts.md](accounts.md)) are runtime invariants on the store, binding every
future grant surface too.

## Relation to existing docs

When built, this supersedes the `Gate` tiers in
[command-dispatch.md](command-dispatch.md) (`Open | Staff` becomes `Open | Cap`) and
the `Staff`-gated framing of the admin frame in [admin-verbs.md](admin-verbs.md) (the
admin verbs re-gate from `Staff` to capabilities; the reference seed's body `Staff`
marker is removed). Those docs are updated in the same change that builds this, not
before.
