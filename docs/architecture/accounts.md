# Accounts: the authority, its store, and how the check resolves

> Status: **authorization built, including the runtime account surface; real
> authentication and the durable backend not started.** This is the implementation
> half of the authorization design: where account records live, how a connection's
> grants reach the gate, and how the system boots. The permission *model* it serves
> (capabilities, the superuser bit, quell) lives in
> [authorization.md](authorization.md); read that first. The authority still boots
> from an empty snapshot every run (no durability); the relational SQLite store
> described below is decided and lands next, ahead of authentication.

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
out of the check layer is what lets the authority live in its own crate.

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
the sim thread owns**, mirroring World-as-truth: grant and account mutations (and
never su in-band at all) are sim-thread calls, the guards below are in-memory
invariants, and durability rides the authority's own persisted form the way the
world rides `Snapshot`. The whole persisted state is one value, **`AccountsSnapshot`**
(every record plus the `next_id` high-water mark): it is the boot input, the
per-mutation persist message, and what the store reads and writes. There is no store
trait: the sim thread never holds a store at all (`boot` takes the loaded snapshot as
plain data; tests build one in place), and a second backend can earn a trait when it
exists.

Two invariants ride the snapshot's shape:

- **`next_id` is persisted, never rebuilt from `max(id) + 1`.** Once anything
  references an `AccountId` (provenance, a characters table), a deleted account's id
  must never be reissued to a new account; the persisted high-water mark is what
  survives the delete.
- **Every save is the full snapshot, so writes are idempotent and self-healing.** A
  failed or lost write is repaired by the next one, and no ordering of partial writes
  can strand the store populated-but-su-less: the operator record rides along in
  every save.

Mutation durability hangs on **one choke point**: every mutator (including the
boot-time bootstrap that mints the operator) marks the authority dirty, and the sim
loop checks `take_dirty()` once per tick, sending a fresh snapshot to the async
writer task (the same channel-to-task shape as entity persistence, so the sim thread
never blocks on the write). No mutator can forget to save, because none of them save.

**Accounts are first-class relational data**, unlike the world's opaque entity
blobs: a future web or oauth frontend, admin tooling, and offline queries all read
the same account set, and the account id is the foreign-key target for provenance
and a later characters table. So the durable store is its **own SQLite database**
(`accounts.sqlite`, separate from the world DB, which a dev reseed deletes) with
real columns: an `accounts` table (`id` primary key app-assigned by the authority,
unique `handle`, `is_su`), an `account_caps` join table (`account_id`, `cap_name`),
and a `meta` table carrying the schema version and the persisted `next_id`. The
world's blob rule exists because world data is never queried at rest; accounts are,
so they take the queryable shape.

The authority is its **own leaf crate**, `musce_auth`: the caps registry, the account
records and snapshot, and the `Accounts` authority, as one cohesive unit.
Account identity is the one piece of the system a consumer beyond the sim host (a web
or oauth frontend, admin tooling) will read, so it does not live inside the host; the
host re-exports it as `musce_host::auth` so a game keeps one import surface. The check
vocabulary it feeds (`CapId`, `CapSet`, `Verdict`) stays in `musce_action`, because
`Gate::Cap` holds a `CapId` and `musce_action` sits below the host; `musce_auth`
therefore depends on `musce_action`, not the reverse. The `conn -> account` resolution
stays welded to the host's `Sessions`.

## Account identity, and slice 1 without auth

An account's identity is a **stable `AccountId`**, persisted, distinct from the
ephemeral `ConnectionId`; it is the store's primary key. Authentication (slice 2) is
what maps a credential to an `AccountId`, but slice 1 ships the authority model before
auth exists, so it needs a stand-in that does not prejudge the identity:

- A connection **defaults to guest**: no account, `Open`-only, no caps and no su.
- The bootstrap creates **one su operator account** with a known `AccountId` and the
  login handle `operator`.
- Two **stub floor attaches** elevate a connection without a credential, both
  **loopback-only**: `@operator` attaches to the su operator, and `@login <handle>`
  attaches to any account by its handle (the general form the credential check will
  replace). Loopback means the peer is present *and* loopback
  (`peer.is_some_and(|p| p.ip().is_loopback())`). A peerless connection is **refused,
  never default-permitted**: the check must not read a `None` peer as trusted, or a
  `.map_or(true, ..)` slip ships elevation on every peerless connection. Over TCP the
  kernel fills the accept peer, so a remote client cannot spoof loopback and the stub
  never ships remote god-mode against a bound port. The `peer` rides `Input::Connected`
  and is threaded into the `Session`. Gating the *entry* this way is consistent with
  leaving un-quell (the operator's own fallback) ungated. Authentication replaces these
  stubs with a real credential check resolving to the same `AccountId`, no shape change.

The operator manages the account set at runtime through su-gated floor commands:
`@account new <handle>` creates a plain (non-su) account, `@grant <handle> <cap>` and
`@revoke <handle> <cap>` adjust its capabilities against the game's registry. These
are floor commands, not game admin-table verbs, because only the host's `Dispatch`
owns the account authority; the verb references the game's cap *vocabulary* (resolved
through the registry) while the *mechanism* (grant a cap to an account) is engine. su
never enters this way (it is out of band), so these mutators never touch the su count
and the su-count floor is not engaged by them; the delete / su-write surface that would
engage it lands with authentication.

So the model is falsifiable end to end: a guest is refused a `Gate::Cap` verb, the
operator grants a **non-su** account a cap, that account logs in and passes the verb,
and `@quell` drops the granted cap (and su) back to baseline. No line of real auth is
needed for any of it.

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

- **Slice 1 (authorization core).** Retire `Staff` and strip its seed body marker; the
  capability model, the caps registry (`register_cap` interning names to `CapId`), and
  `Gate::Cap`; the su account bit and the `(cap, verdict)` `permits`; `@quell` on the
  floor; the in-memory account authority (the `AccountId` identity, the loopback-only
  operator stub, first-account-su, the boot check); `conn -> account` resolved at the
  dispatch seam as a verdict pinned to the account and carried on `Ctx`. Built.
- **Slice 2a (the account surface).** Quellable caps (`register_baseline_cap`, the
  baseline/quellable split in the verdict); the runtime account mutators
  (`create_account`/`grant`/`revoke`) and their operator-only floor verbs
  (`@account`/`@grant`/`@revoke`); a login `handle` on the record and the loopback
  `@login <handle>` stub. This makes the composable model real end to end (a non-su
  account holding a granted cap, reachable by login, dropped by quell). Built.
- **Slice 2b.1 (the durable store).** The relational `accounts.sqlite` store above
  (`accounts` / `account_caps` / `meta` with the persisted `next_id`), loaded in
  `run`'s async context, written by an async writer task fed by the dirty-flag beat.
  Decided (this design), landing next.
- **Slice 2b.2 (authentication).** Real credentials (passwords first, then
  tokens/oauth) replacing the `@login`/`@operator` stubs; a nullable `pw_hash`
  column joins the `accounts` table then. Needs the password-hashing dependency
  (argon2, already parked in `musce_auth`).
- **Slice 2b.3 (delete and su writes).** The surface that finally engages the
  **post-image su-count floor** (no mutator may drop the su count below one, checked
  atomically on the post-image); id reuse is already impossible via the persisted
  `next_id`.

Entity locking stays fully game-side and is untouched (the `Locked`-marker rule
pattern plus a game-registered key relation; see the roadmap and
[networking-and-sessions.md](networking-and-sessions.md)).
