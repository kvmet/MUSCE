# Authorization

> Status: built. Password login is real: `@login <username> <password>` verifies
> against a stored argon2 credential off-thread, `@account new <username> <password>`
> hashes on creation, `@password <old> <new>` (alias `@pw`) changes an account's own
> password, and `@operator` remains the passwordless loopback bootstrap. The
> primitives, the off-thread account task, the async auth round-trip with
> pending-auth rejection, the app login veto, operator bootstrap, and the host
> command wiring are all implemented and tested. Still deferred: operator-set
> passwords for another account, and non-password auth (OAuth). The line-mode
> transport carries passwords in the clear until a secure transport lands.

Two things are kept apart. **Authentication** proves which account a connection is:
a credential check that yields an `AccountId`. **Authorization** decides what that
connection may do: a `Verdict` the gate consults. They are separate because a
connection has an account long before it has a body, and because the credential
check is slow and off-thread while the gate check is a hot per-command read.

The shaping decision: **an account is infrastructure, not a world entity.** Accounts
exist before any character, must not ride the world's snapshot cadence, and must not
be walked by game systems or the reflection admin layer. So they are *not* entities
in the ECS world. They are a thin, columnar table in the same store as the world.
Authorization is then a `Verdict` resolved from an account's capabilities and its
superuser bit, consulted at the dispatch seam.

## Accounts live in the one store, not a parallel one

A prior design gave accounts their own authority object, their own database, their
own snapshot-the-whole-set writer, and their own id space. That was a parallel
universe bolted onto a World-as-truth engine, and it is the design this replaces.

The rule that holds instead is the same one the rest of the engine follows: **one
persistence path.** The `accounts` table lives in the shared `WorldStore` (same
database, same trait, same pool, same migration story), written with ordinary
per-row upserts. A dedicated *table* is not a separate *path*; the mistake was the
parallel store and the whole-set snapshot, never the fact that accounts have their
own rows.

The account authentication secret is not special-cased out of this. A password hash
is a column on the account row like any other, kept out of the hot world for free
because accounts are not resident world state to begin with.

## The account record

`Account` ([account.rs](../../musce_auth/src/account.rs)) is deliberately dumb and
app-agnostic. Its fields, and why each is shaped the way it is:

- **`id`** — a stable, opaque v7-UUID surrogate key, minted at creation, never
  reused. Everything durable (sessions later, character ownership later) points at
  this, not at the name. Natural keys are the problem it avoids.
- **`username`** — the unique human login name. Kept *separate* from the id and
  therefore free to change: rename is a non-event precisely because the username is
  not load-bearing. Uniqueness is a store-level `UNIQUE` constraint plus a
  creation-time check; the record cannot enforce it alone.
- **`credential`** — a nullable PHC hash. `None` is a real state: the bootstrap
  operator (reachable via the loopback stub) and future external-auth accounts have
  no password.
- **`caps`** — capability *names*, as a JSON array. Names, not ids: the record is a
  leaf that knows nothing of any app's interned vocabulary. Stored as names because
  a `CapId` is a runtime handle, not a stable key (see below).
- **`su`** — the superuser bit, its own column. A distinct axis, *not* a capability,
  so it maps onto the verdict's su override and stays off the generic grant path
  (elevating someone to su is a guarded operation, not "grant a cap like any
  other"), and "who are the superusers" is a real query.
- **`status`** — `Active` / `Disabled`, the one authorization axis the engine
  enforces itself. See below.
- **`app_data`** — an opaque JSON value the engine stores and hands back but never
  reads. All the app's own account machinery (subscriber tiers, approval flags, ban
  reasons) lives here. Same treatment as cold content: the engine carries structure
  it does not interpret.

The engine reads exactly two account axes, `su` and `status`, and holds each as a
column; caps are opaque membership; everything else is app data. That is "the engine
owns a kind iff it reads it" applied to accounts.

## Capabilities and the verdict

Capabilities are **strings in code and in the database, interned to a `CapId` at
startup.** They cannot be a single compile-time enum: the engine and the app are
different crates that must share one `CapId` space so a single `Verdict` can hold
both, and an enum cannot span crates. So the app declares its vocabulary while
wiring its gates, and `CapRegistry` ([registry.rs](../../musce_action/src/registry.rs))
mints a `CapId` per name. Gate registration and grant resolution both go through
that one registry, so a gate's id and a grant's id denote one capability. The
**engine registers no capabilities of its own** (su and status are columns, not
caps); every name is the app's.

Because ids are runtime handles, grants persist as *names* and resolve at load.
`CapRegistry::resolve_set` returns any names it could not resolve *separately* from
the ones it could, so vocabulary drift (a grant naming a capability the current
build no longer defines) surfaces as a log line rather than a silently dropped
grant.

The `Verdict` ([caps.rs](../../musce_action/src/caps.rs)) is the pure primitive the
gate compares against: a resolved `CapSet` plus a superuser override. `permits(cap)`
is `su_override || caps.contains(cap)`. `Verdict::resolved(caps, su, quelled)`
builds it from an account's authorization and a connection's quell state, and is the
one place the quell rule lives.

Two properties the verdict enforces:

- **Authority is per-account, not per-body.** The verdict keys off the connection's
  account, never the resolved actor, so possessing or `@play`-selecting a privileged
  body cannot borrow its authority.
- **`@quell` makes you your character.** A quelled connection drops to the guest
  verdict, setting aside *both* su and every granted cap. Quell is not "set aside
  god-mode only"; it is "act as a plain user," which is what the vast majority of
  connections already are. (A cap that should survive quell would be a later opt-out
  flag, unbuilt because nothing needs it.)

## Credentials and hashing

Password hashing ([password.rs](../../musce_auth/src/password.rs)) is argon2id,
PHC-string encoded (algorithm, cost parameters, and a per-hash random salt all
embedded in the output). It owns no storage: it turns a password into a
self-describing string and checks a password against one.

Two rules it holds:

- **Never on the sim thread.** argon2 is deliberately slow; a verify on the 10 Hz
  tick would stall every connected player. Hashing and verifying run on the async
  side, on a blocking pool.
- **A corrupt hash is not a wrong password.** `verify_password` returns `Ok(false)`
  for a valid hash the password does not satisfy, but `Err` for a stored hash that
  will not parse. A storage fault must never be counted as an ordinary failed login.

## Storage: the `accounts` table

The table is columnar, one column per field
([accounts_table_ddl](../../musce_persistence/src/lib.rs)): `id`, `username`
(`UNIQUE`), `credential` (nullable), `caps` (JSON text), `su` (the dialect's boolean
word, `INTEGER` on SQLite / `BOOLEAN` on Postgres), `status`, and `app_data` (JSON
text). Columnar over a JSON blob so the row is legible and `username`/`su` are real
indexed columns rather than fields buried in a document.

`AccountStore` is a trait beside `Persistence` and `KvStore`, implemented on both
backends and forwarded through `WorldStore`, with five methods: `accounts_init`,
`account_by_username` (login and admin lookup), `account_by_id` (self-service, where
a connection holds its authenticated `AccountId`, not a typed username),
`account_upsert` (create / grant / revoke / status / password change), and
`any_superuser` (the bootstrap gate). No full load, no whole-set snapshot: the sim
holds no global account map, so the store is read by username or id on demand and
written per row on mutation.

## The runtime flow

**Startup.** The app declares its cap vocabulary (each name interned to a `CapId`);
the shared store opens (world tables, kv, and `accounts` in one database). Bootstrap:
if `any_superuser` is false, seed one su operator so the world can be administered.

**Authenticate** (an async round-trip, the `ColdOp` analogue). A login line reaches
the sim, which issues an auth op to an off-thread auth task and marks the connection
*pending-auth*; further lines from a pending connection are **rejected**, not queued,
so one connection cannot spam parallel argon2 work. The auth task looks the account
up by username, applies the **engine hard gate** (`Disabled` is refused,
unconditionally), verifies the password against the stored argon2 hash on a blocking
pool (a `None` password is the passwordless `@operator` bootstrap, valid only against
a credential-less account), then runs the **app login veto**: an injected hook given
the account's status and `app_data` that may refuse an `Active` account (approval
workflows, region locks). The veto can only *further restrict*; it cannot lift
`Disabled`. On success the task hands the sim
`{ conn, AccountId, CapSet, su }`, and the sim caches the resolved caps and su in the
connection's session.

**Authorized command.** The gate builds the `Verdict` from the session's cached
`{ CapSet, su, quelled }`, no store touch on the hot path.

**Mutation.** `@grant` / `@revoke` / account admin (su-gated) load the target by
username, mutate, and upsert per-row; if the target is online, its cached caps are
refreshed too.

**Self-service password change.** `@password <old> <new>` (alias `@pw`) is not
su-gated: an account changes its own password. It loads the account by the session's
authenticated `AccountId` (never a typed username, so one connection cannot aim the
change at another account), verifies `old` against the stored hash on the blocking
pool, hashes `new` there too, and upserts. It changes only a credential, not
authorization, so it refreshes no session: live sessions keep running under their
cached caps. A passwordless account (the operator, a future external-auth account)
has no password to change and is refused; setting a *first* password is a separate,
still-deferred operation.

## Sessions and resumption

A session is in-memory and connection-scoped: the floor maps a `ConnectionId` to the
account it authenticated as, plus play state (the driven character, the quell bit).
It dies with the connection, which is correct: a process restart drops every
connection anyway, so persisting session state would persist garbage.

**Resumable sessions** (close the client, reconnect still logged in) are deferred and
purely additive: a `sessions` table of `{ token, account_id, expires_at }`, a token
minted at login, and a resume path that looks the token up to an `AccountId` and
skips the credential check. Two decisions already in place make it drop-in rather
than surgery: the token references the immutable `id` (not the rename-able username),
and authentication already yields an `AccountId` regardless of method, so
token-resume is a third method into the same "establish a session from an
`AccountId`" step. Getting the *body* back on reconnect is a separate concern
(durable `Controls`/`Focus` embodiment plus `@play`), not the session's job.

## Built vs deferred

Built and tested:

- Password hashing (`hash_password` / `verify_password`).
- The `Account` record and its `AccountId` / `AccountStatus`, with the columnar
  reconstruction API (`from_stored`, `AccountId: Display + FromStr`,
  `AccountStatus::as_str`).
- The `accounts` table and `AccountStore` (including `account_by_id`) on SQLite and
  Postgres.
- The `CapRegistry` interner and `Verdict::resolved` (the quell rule).
- The off-thread account task and its ops (authenticate, create, grant/revoke), the
  async round-trip with pending-auth rejection, the app login veto, operator
  bootstrap seeding, and the host command wiring, with session-cached authorization.
- Real credential verification: `@account new <username> <password>` hashes on
  creation and `@login <username> <password>` verifies against the stored argon2
  hash, both off-thread; `@operator` stays the passwordless loopback bootstrap.
- Self-service password change: `@password <old> <new>` (alias `@pw`) verifies the
  old password and stores a new hash, keyed off the session's `AccountId` via
  `account_by_id`, both hashing steps off-thread.

Deferred:

- Operator-set passwords for another account (resetting a forgotten password), still
  unbuilt; the `account_by_id` primitive it needs now exists.
- Password confidentiality in transit: the line-mode transport is cleartext until a
  secure transport lands.
- Resumable sessions (the token store).
- Cross-connection auth rate limiting (per-connection one-in-flight is the current
  floor).
- External authentication (OAuth) as an additional method resolving to an
  `AccountId`.
