# Accounts: the authority, its store, and how the check resolves

> Status: **slice 1 built; slice 2 (authentication) not started.** This is the
> implementation half of the authorization design: where account records live, how a
> connection's grants reach the gate, and how the system boots. The permission *model*
> it serves (capabilities, the superuser bit, quell) lives in
> [authorization.md](authorization.md); read that first. Slice 1's backend is the
> in-memory `MemoryAccountStore` (in `musce_host::auth`); the durable backend behind
> the same `AccountStore` trait lands with slice 2.

The model says the engine authorizes accounts on a flat set of capabilities plus a
superuser bit. This covers the machinery that makes that real: resolving a
connection to an authority **verdict** at dispatch, the in-memory account authority
and its store backend, account identity before authentication exists, and the
bootstrap and last-su invariants. Authentication itself (passwords, tokens, oauth) is
a later slice behind the same store seam.

## How the check gets the grants

Three concerns meet at dispatch, and the `Verdict` is the only type that crosses
between them. **The check** (`musce_action`: `Gate`, `CapId`, `CapSet`, `Verdict`,
`permits`) decides whether a verdict admits a cap and knows nothing of accounts.
**The account authority** (the auth module: the caps registry, the account records,
the store, and the `Accounts` authority) produces a `Verdict` and knows nothing of
dispatch or `Ctx`. **The wiring** here resolves one from the other, and is the only
layer that knows all three exist. Keeping the caps registry and the account records
out of the check layer is what lets the authority lift to `musce_auth` as one piece.

`Gate` lives in `musce_action`, below the session and account authority in the host,
so the check needs the actor's grants without `musce_action` depending on the host.
Resolution happens at the `dispatch_through_actor` seam in the host, the single choke
point both the game table and the admin table route through, so resolving once there
gates both identically. That seam already computes the acting actor; alongside it the
host resolves the **authority verdict** and passes it into `dispatch_command` as its
own parameter. The account authority is a new field on the host's `Dispatch` (beside
`floor` and `game`) threaded into that seam; the resolution is not something
`Sessions` can do alone.

- **Grants key off the account, never the resolved actor.** The gated subject is the
  focused puppet, an arbitrary world entity; keying grants off it would let
  possessing or `@play`-selecting a privileged body borrow authority. This is enforced
  at the **type boundary, not by one test**: the verdict arrives as a value the host
  built from `conn -> session -> account`, carrying nothing derivable from `actor` or
  `World`, so no present or future actor-derived index (the audience `Actors`,
  `control_root`, a slice-2 site) can reintroduce the borrow. A test that fails if
  grants resolve from the actor backs the boundary but does not replace it.
- **The verdict is `{ caps, su_override: bool }`, not a fake universal set.** The host
  collapses the su/quell half (it reads host-only `connection.quelled`) into the
  `su_override` flag; `musce_action`'s `permits` changes from `(world, actor)` to
  `(cap, verdict)` and checks `verdict.su_override || verdict.caps.contains(cap)`.
  Keeping su an explicit flag, not a set stuffed to answer everything, keeps audit
  honest: "su authorized this" stays distinguishable from "cap X authorized this."
- **The verdict rides on `Ctx`**, so a game handler's inline rules read it (the
  su-aware scoped checks in [authorization.md](authorization.md)) exactly as the gate
  does. `Ctx` carries only this **resolved, read-only verdict**, never the mutable
  source it was collapsed from (`connection.quelled`, the account authority). That
  single line explains both why the verdict may ride `Ctx` and why `@quell` cannot be
  a table verb: toggling the source needs the host, while reading the snapshot does
  not. `SystemCtx` is a separate struct with no actor and never carries a verdict.

Resolution touches no `World` (only session state and the account authority), so it is
cheap per dispatch and is **not cached**: a mid-connection quell change takes effect
on the next command, and when the slice-2 writer lands, a live grant change will too,
with no cache to invalidate. `Session` grows an `account` field to carry the
`conn -> account` link, alongside the `quelled` flag; both are per-connection state on
the same struct.

## The account authority and its store

Accounts are **not world entities**, so they are never in a `Snapshot` and do not
round-trip through the entity store. The live authority is an **in-memory structure
the sim thread owns**, mirroring World-as-truth: what grant and su mutations exist
(boot load, and eventually the slice-2 writer; never a table verb, and never su
in-band at all) are sim-thread calls, the guards below are in-memory invariants, and
durability rides a **store backend** the way the world rides `Snapshot`. That settles the "single-writer" claim
honestly: the authority is sim-thread-owned like the world, the store is its
save/load target, and slice 2's web/oauth writer is the second writer that will need
real serialization. Slice 1 stands up **no parallel async save task**: the trivial
backend loads accounts once at boot and saves on the existing shutdown/cadence beat
the loop already runs. The store *trait* is the seam worth reserving now; a dedicated
channel and task like the entity persistence path wait for the real backend.

The store backend is the genuine decide-now artifact:

- A **trait taking no `World` and no host types** (auth needs neither the ECS nor host
  internals), which is what keeps it liftable.
- Its **own storage home**, not the `entities` table: an account has no `EntityId`, no
  zone, and no place in the O(world) snapshot, so bolting it in inherits all three and
  is the migration to avoid.
- A **version field in the record from day one** (a `u32` beside the grant set and su
  bit), so the record is self-describing. The record serializes as a **JSON blob**
  (mirroring `EntityBlob`), so the reserved version has a concrete encoding the later
  seam can compare and migrate. The compare-on-load and migration-*seam* machinery is
  deferred until the real backend lands (building it for a trivial slice-1 backend is
  a speculative parallel persistence layer); the reserved field is the addition-cheap
  hedge that makes the later seam possible.

The **crate lift is a single unit**: the whole auth module (the caps registry, the
account records, the `AccountStore` trait, and the `Accounts` authority) lifts into a
leaf `musce_auth` crate when the second consumer lands. The check vocabulary it feeds
(`CapId`, `CapSet`, `Verdict`) stays in `musce_action`, because `Gate::Cap` holds a
`CapId` and `musce_action` sits below the host; `musce_auth` therefore depends on
`musce_action`, not the reverse. The `conn -> account` resolution stays welded to the
host's `Sessions`.

## Account identity, and slice 1 without auth

An account's identity is a **stable `AccountId`**, persisted, distinct from the
ephemeral `ConnectionId`; it is the store's primary key. Authentication (slice 2) is
what maps a credential to an `AccountId`, but slice 1 ships the authority model before
auth exists, so it needs a stand-in that does not prejudge the identity:

- A connection **defaults to guest**: no account, `Open`-only, no caps and no su.
- The seed creates **one su operator account** with a known `AccountId`.
- A **stub floor attach** elevates a connection to that operator account. It is
  unauthenticated in slice 1 but **loopback-only**: it elevates only when the peer is
  present *and* loopback (`peer.is_some_and(|p| p.ip().is_loopback())`). A peerless
  connection is **refused, never default-permitted**: the check must not read a `None`
  peer as trusted, or a `.map_or(true, ..)` slip ships elevation on every peerless
  connection. Over TCP the kernel fills the accept peer, so a remote client cannot
  spoof loopback and slice 1 never ships remote god-mode against a bound port. The
  `peer` rides `Input::Connected` but is dropped at `Sessions::connect` today, so
  slice 1 threads it through into the `Session`. Gating the *entry* to su this way is
  consistent with leaving un-quell (the operator's own fallback) ungated. Slice 2
  replaces the stub with a real credential check resolving to the same `AccountId`, no
  shape change.

So slice 1 is falsifiable end to end: a guest connection is refused a `Gate::Cap`
verb, the elevated operator passes it, and `@quell` drops the operator to guest-level
authority, all without a line of real auth.

## Bootstrapping and the last-su invariant

su only ever enters through the out-of-band path in
[authorization.md](authorization.md), so the first su comes from the store's own
bootstrap, and "at least one su exists" must hold across every path, not just revoke:

- The **first account created is su by default** (removable later), the bit **written
  at promotion and persisted on the record**, never re-derived from account ordering
  (min `AccountId`, insertion order, map iteration) at load, so a reordered or rekeyed
  store cannot shift which account is su. Not *permanent* su, which breaks if that
  account is deleted.
- A **su-count floor**: no store mutator may drop the su count below one, enforced as
  an invariant on **every** mutating method (revoke, delete, and any bulk grant-set
  overwrite, record replacement, or import), not enumerated per method name. A
  whole-record write of the last su is exactly the path a per-method guard misses. The
  check is on the mutation's **post-image**, atomically, so a bulk import whose result
  sums to zero su is refused (a pre-image check would pass and then land zero).
- A **boot check that distinguishes three cases**: a genuinely **empty** store on
  first-ever boot promotes its first account to su (the bootstrap); a **populated**
  store that loads with **zero** su refuses to boot; a store **load error** also
  refuses to boot, never treated as empty, so a store that fails to read does not fall
  into the promote branch and mint a god over accounts it could not load. The refusal
  is safe for slice 1 because reaching a populated-zero-su or unreadable store already
  required offline store manipulation (normal play's floor keeps the count above
  zero), so recovery is the same offline access that caused it, and it never lets
  account-ordering silently mint a god on a restore.

## Slices

- **Slice 1 (authorization).** Retire `Staff` and strip its seed body marker; the
  capability model, the caps registry (`register_cap` interning names to `CapId`), and
  `Gate::Cap`; the su account bit and the `(cap, verdict)` `permits`; `@quell` on the
  floor; the in-memory account authority with its store trait and trivial backend (the
  `AccountId` identity, the loopback-only operator stub, first-account-su, the
  post-image su-count floor, the boot check, a reserved version field on a JSON
  record); `conn -> account` resolved at the dispatch seam as a verdict pinned to the
  account and carried on `Ctx`. No runtime grant or su administration ships in
  slice 1: su is out of band permanently (see [authorization.md](authorization.md)),
  grants only until a real surface lands. This makes the composable model real end to
  end.
- **Slice 2 (authentication).** Passwords, tokens, oauth, as additive backends behind
  the store trait; nothing here is built now.

Entity locking stays fully game-side and is untouched (the `Locked`-marker rule
pattern plus a game-registered key relation; see the roadmap and
[networking-and-sessions.md](networking-and-sessions.md)).
