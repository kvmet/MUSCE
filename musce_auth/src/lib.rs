//! The account authority: who exists, what each account may do, and how an account
//! resolves to an authorization [`Verdict`]. A leaf crate so account identity can
//! serve consumers beyond the sim host (a web or oauth frontend reads the same
//! accounts). It depends on `musce_action` only for the check vocabulary it produces
//! (`CapId`/`CapSet`/`Verdict`) and knows nothing of dispatch, sessions, or the
//! `World`. See `docs/architecture/accounts.md`.

mod store;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use musce_action::{CapId, CapSet, Verdict};

pub use store::{ACCOUNTS_SCHEMA_VERSION, AccountStore, StoreError};

/// Stable account identity: persisted, the store's primary key. Distinct from the
/// ephemeral `ConnectionId` and from any `EntityId` (accounts are not world
/// entities), so the two never get confused at a call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(pub u64);

/// The persisted form of an account: identity, a login **handle** (the name a
/// connection authenticates against), grants **by string name** (stable across id
/// churn, and the game's vocabulary rather than the engine's), and the superuser
/// bit. The live authority resolves the grant strings to `CapId`s at load against
/// the same registry the gates were built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountRecord {
    pub id: AccountId,
    pub handle: String,
    pub caps: Vec<String>,
    pub is_su: bool,
}

/// The authority's full persisted state: every record plus the id high-water mark.
/// One value is the whole story, so it is the boot input, the per-mutation persist
/// message, and what the store reads and writes. `next_id` is carried explicitly
/// (never rebuilt from `max(id) + 1`) so a deleted account's id is never reissued to
/// a later account: anything that comes to reference an `AccountId` (provenance, a
/// characters table) must never watch it change owners.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountsSnapshot {
    pub records: Vec<AccountRecord>,
    pub next_id: u64,
}

impl AccountsSnapshot {
    /// The empty store: no accounts yet, ids start at 1. Booting from this
    /// bootstraps the operator.
    pub fn empty() -> Self {
        AccountsSnapshot {
            records: Vec::new(),
            next_id: 1,
        }
    }

    /// A snapshot for a fresh store seeded with `records`, the high-water mark
    /// taken from the records themselves. A convenience for tests and seeding; a
    /// durable store loads the persisted `next_id` instead, which survives deletes.
    pub fn from_records(records: Vec<AccountRecord>) -> Self {
        let next_id = records.iter().map(|r| r.id.0).max().unwrap_or(0) + 1;
        AccountsSnapshot { records, next_id }
    }
}

/// The caps registry: interns a game's capability names to stable `CapId`s. The game
/// builds it while wiring its gates, so a gate for `"build"` and a grant of `"build"`
/// resolve to the same id; the authority then resolves account grant strings against
/// this same registry. It lives with the accounts, not the dispatch check, because it
/// exists only to serve grants.
#[derive(Default)]
pub struct CapRegistry {
    ids: HashMap<String, CapId>,
    names: Vec<String>,
    /// Whether each cap (indexed by `CapId`) drops under `@quell`. Parallel to
    /// `names`. Default-true: an elevated grant a builder sets aside to verify the
    /// normal-player experience, the same rationale as su. A rare baseline right (a
    /// `member` cap) opts out via [`register_baseline_cap`].
    quellable: Vec<bool>,
}

impl CapRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern a **quellable** capability name (the default), returning its stable id.
    /// Idempotent: the same name always returns the same id, so registering a gate
    /// and later resolving a grant of the same name agree.
    pub fn register_cap(&mut self, name: &str) -> CapId {
        self.intern(name, true)
    }

    /// Intern a **baseline** (non-quellable) capability: a right that survives
    /// `@quell` because it is not elevated authority (e.g. a `member` cap). The only
    /// added surface over [`register_cap`]; everything else defaults to quellable.
    pub fn register_baseline_cap(&mut self, name: &str) -> CapId {
        self.intern(name, false)
    }

    /// Intern `name` with the given quell behavior. On a re-register the first
    /// registration's behavior stands (the id is stable and so is its flag).
    fn intern(&mut self, name: &str, quellable: bool) -> CapId {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = CapId(self.names.len() as u32);
        self.names.push(name.to_string());
        self.quellable.push(quellable);
        self.ids.insert(name.to_string(), id);
        id
    }

    /// Resolve a name registered earlier. `None` if it was never registered, which
    /// the authority treats as a hard load error rather than a silent drop.
    pub fn resolve(&self, name: &str) -> Option<CapId> {
        self.ids.get(name).copied()
    }

    /// Whether `cap` drops under `@quell`. An unknown id reads as quellable, the safe
    /// default (an elevated grant should not survive quell by accident).
    pub fn is_quellable(&self, cap: CapId) -> bool {
        self.quellable.get(cap.0 as usize).copied().unwrap_or(true)
    }
}

/// Why the account authority refused to boot. Both cases require offline store
/// manipulation to reach (normal play never drops the su count or writes an unknown
/// grant), so refusing is safe: recovery is the same offline access that caused it.
#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    /// A record grants a capability name the registry never registered.
    UnknownGrant { account: AccountId, name: String },
    /// The store holds accounts but none is a superuser.
    NoSuperuser,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::UnknownGrant { account, name } => {
                write!(f, "account {account:?} grants unknown capability {name:?}")
            }
            AuthError::NoSuperuser => {
                write!(f, "account store has accounts but no superuser")
            }
        }
    }
}

impl std::error::Error for AuthError {}

/// Why a runtime account mutation failed. Distinct from [`AuthError`] (which is
/// boot-only): these surface as operator feedback on a `@grant`/`@revoke`, not a
/// refuse-to-boot.
#[derive(Debug, PartialEq, Eq)]
pub enum AccountError {
    /// The target account id does not exist.
    NoSuchAccount(AccountId),
    /// The capability name is not in the registry (a typo, or an unregistered cap).
    UnknownCap(String),
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountError::NoSuchAccount(id) => write!(f, "no such account {id:?}"),
            AccountError::UnknownCap(name) => write!(f, "no such capability {name:?}"),
        }
    }
}

impl std::error::Error for AccountError {}

/// One live account in the authority: its grants resolved to ids (with the
/// non-quellable subset kept apart so `@quell` can drop the rest), its login handle,
/// and its su bit. The grant strings are kept so the record round-trips back to the
/// store unchanged.
struct Account {
    /// Every granted capability.
    caps: CapSet,
    /// The subset of `caps` that survives `@quell` (the non-quellable grants).
    baseline: CapSet,
    grants: Vec<String>,
    handle: String,
    is_su: bool,
}

/// The live account authority: sim-thread-owned, mirroring World-as-truth. Holds each
/// account's resolved caps and su bit in memory, owns (shares) the cap registry it is
/// defined against so the runtime grant surface resolves without threading it, and
/// answers `verdict_for` at dispatch. The at-least-one-superuser invariant holds across
/// boot (bootstrap or refuse). The runtime surface here grants and revokes non-su caps
/// only; su is out of band, so the per-mutation su-count floor lands with the
/// delete/su-write surface in the authentication slice.
pub struct Accounts {
    accounts: BTreeMap<AccountId, Account>,
    next_id: u64,
    /// The cap vocabulary this authority resolves grants against, shared with the game
    /// it was booted from. Immutable (registration finished at game construction).
    registry: Arc<CapRegistry>,
    /// Set by every mutation (including the boot-time bootstrap), cleared by
    /// [`take_dirty`]. The one choke point durability hangs off: the host checks it
    /// each tick and persists a fresh [`snapshot`], so no mutator can forget to save.
    ///
    /// [`take_dirty`]: Accounts::take_dirty
    /// [`snapshot`]: Accounts::snapshot
    dirty: bool,
}

impl Accounts {
    /// Build the authority from a loaded snapshot, resolving each record's grant
    /// strings to `CapId`s against `registry` (which the authority then holds for its
    /// runtime grant surface). Enforces the boot cases: an **empty** snapshot
    /// bootstraps one su operator; a **populated** snapshot with **zero** su refuses;
    /// an **unknown grant** refuses. (A store *load* error is the caller's
    /// refuse-to-boot, never routed here as empty.)
    pub fn boot(snapshot: AccountsSnapshot, registry: Arc<CapRegistry>) -> Result<Self, AuthError> {
        let mut accounts = BTreeMap::new();
        for rec in snapshot.records {
            let mut caps = CapSet::new();
            let mut baseline = CapSet::new();
            for name in &rec.caps {
                let id = registry
                    .resolve(name)
                    .ok_or_else(|| AuthError::UnknownGrant {
                        account: rec.id,
                        name: name.clone(),
                    })?;
                caps.insert(id);
                if !registry.is_quellable(id) {
                    baseline.insert(id);
                }
            }
            accounts.insert(
                rec.id,
                Account {
                    caps,
                    baseline,
                    grants: rec.caps,
                    handle: rec.handle,
                    is_su: rec.is_su,
                },
            );
        }

        let mut authority = Accounts {
            accounts,
            next_id: snapshot.next_id,
            registry,
            dirty: false,
        };

        if authority.accounts.is_empty() {
            // First-ever boot: lay down one operator. The first account created is su
            // by default (see `create`), so this is the bootstrap superuser.
            authority.create();
        } else if authority.su_count() == 0 {
            return Err(AuthError::NoSuperuser);
        }

        Ok(authority)
    }

    /// Lay down the bootstrap superuser: the first account in an empty authority,
    /// su by default (the bit written here and carried on the record, never
    /// re-derived from ordering at load). Called only from the empty-store boot path,
    /// so it is always the first account; the su bit reflects that.
    fn create(&mut self) -> AccountId {
        let is_su = self.accounts.is_empty();
        self.insert(Account {
            caps: CapSet::new(),
            baseline: CapSet::new(),
            grants: Vec::new(),
            handle: "operator".into(),
            is_su,
        })
    }

    /// Create a plain account with a login `handle` and no grants or su. The runtime
    /// account-creation surface (the operator's `@account new`); the caller is
    /// responsible for handle uniqueness (see [`account_by_handle`]).
    pub fn create_account(&mut self, handle: &str) -> AccountId {
        self.insert(Account {
            caps: CapSet::new(),
            baseline: CapSet::new(),
            grants: Vec::new(),
            handle: handle.to_string(),
            is_su: false,
        })
    }

    /// Insert an account under a fresh id and return it. `next_id` only ever climbs
    /// (and is persisted in the snapshot), so an id is never reissued, even after a
    /// future delete.
    fn insert(&mut self, account: Account) -> AccountId {
        let id = AccountId(self.next_id);
        self.next_id += 1;
        self.accounts.insert(id, account);
        self.dirty = true;
        id
    }

    /// The account with login `handle`, if any. Backs `@login` and the `@grant`
    /// target lookup; handles are unique because creation goes through this check.
    pub fn account_by_handle(&self, handle: &str) -> Option<AccountId> {
        self.accounts
            .iter()
            .find(|(_, a)| a.handle == handle)
            .map(|(&id, _)| id)
    }

    /// Grant capability `cap_name` to an account. Idempotent: a repeat grant is a
    /// no-op. Updates the resolved set and, for a non-quellable cap, the baseline so
    /// `@quell` keeps it. Never touches su (su is out of band), so no su-floor
    /// concern arises here.
    pub fn grant(&mut self, id: AccountId, cap_name: &str) -> Result<(), AccountError> {
        let cap = self
            .registry
            .resolve(cap_name)
            .ok_or_else(|| AccountError::UnknownCap(cap_name.to_string()))?;
        let quellable = self.registry.is_quellable(cap);
        let account = self
            .accounts
            .get_mut(&id)
            .ok_or(AccountError::NoSuchAccount(id))?;
        if account.caps.insert(cap) {
            account.grants.push(cap_name.to_string());
        }
        if !quellable {
            account.baseline.insert(cap);
        }
        self.dirty = true;
        Ok(())
    }

    /// Revoke capability `cap_name` from an account. A no-op if it was not held.
    /// Never touches su, so the su-count floor is not engaged.
    pub fn revoke(&mut self, id: AccountId, cap_name: &str) -> Result<(), AccountError> {
        let cap = self
            .registry
            .resolve(cap_name)
            .ok_or_else(|| AccountError::UnknownCap(cap_name.to_string()))?;
        let account = self
            .accounts
            .get_mut(&id)
            .ok_or(AccountError::NoSuchAccount(id))?;
        account.caps.remove(cap);
        account.baseline.remove(cap);
        account.grants.retain(|g| g != cap_name);
        self.dirty = true;
        Ok(())
    }

    /// How many accounts are superuser. The at-least-one-su invariant reads this.
    pub fn su_count(&self) -> usize {
        self.accounts.values().filter(|a| a.is_su).count()
    }

    /// The account the slice-1 loopback stub attaches to: the lowest-id superuser.
    /// Deterministic whether the authority was bootstrapped or loaded.
    pub fn stub_operator(&self) -> Option<AccountId> {
        self.accounts
            .iter()
            .find(|(_, a)| a.is_su)
            .map(|(&id, _)| id)
    }

    /// The verdict a connection runs under. Un-quelled: the account's full caps with
    /// its su bit. Quelled: su is suppressed **and** the quellable caps drop, leaving
    /// only the baseline set, so a builder can set aside every elevated grant (su and
    /// caps alike) to verify the normal-player experience. No account (a guest) is the
    /// empty, no-su verdict. Keyed off the account, never an actor, so no possessed
    /// body can borrow authority.
    pub fn verdict_for(&self, account: Option<AccountId>, quelled: bool) -> Verdict {
        match account.and_then(|id| self.accounts.get(&id)) {
            Some(a) if quelled => Verdict::new(a.baseline.clone(), false),
            Some(a) => Verdict::new(a.caps.clone(), a.is_su),
            None => Verdict::guest(),
        }
    }

    /// The authority's full persisted state, for a store save. Each snapshot is the
    /// complete story (every record plus `next_id`), so a save is idempotent and
    /// self-healing: whatever a prior write lost, the next one restores.
    pub fn snapshot(&self) -> AccountsSnapshot {
        AccountsSnapshot {
            records: self
                .accounts
                .iter()
                .map(|(&id, a)| AccountRecord {
                    id,
                    handle: a.handle.clone(),
                    caps: a.grants.clone(),
                    is_su: a.is_su,
                })
                .collect(),
            next_id: self.next_id,
        }
    }

    /// Whether a mutation has landed since the last check, clearing the flag. The
    /// host's persist beat: true means "take a [`snapshot`](Accounts::snapshot) and
    /// save it."
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: u64, caps: &[&str], is_su: bool) -> AccountRecord {
        AccountRecord {
            id: AccountId(id),
            handle: format!("acct{id}"),
            caps: caps.iter().map(|s| s.to_string()).collect(),
            is_su,
        }
    }

    #[test]
    fn registry_interns_idempotently() {
        let mut reg = CapRegistry::new();
        let a = reg.register_cap("build");
        let b = reg.register_cap("ban");
        assert_ne!(a, b, "distinct names get distinct ids");
        assert_eq!(
            reg.register_cap("build"),
            a,
            "same name returns the same id"
        );
        assert_eq!(reg.resolve("build"), Some(a));
        assert_eq!(reg.resolve("never"), None);
    }

    #[test]
    fn empty_store_bootstraps_one_su_operator() {
        let reg = CapRegistry::new();
        let auth = Accounts::boot(AccountsSnapshot::empty(), Arc::new(reg)).unwrap();

        assert_eq!(auth.su_count(), 1, "bootstrap lays down exactly one su");
        let op = auth.stub_operator().expect("a bootstrapped operator");
        assert!(
            auth.verdict_for(Some(op), false).is_su(),
            "the operator runs as su"
        );
        assert_eq!(
            auth.account_by_handle("operator"),
            Some(op),
            "the bootstrap operator is reachable by its handle"
        );
    }

    #[test]
    fn quell_drops_su_from_the_operator_verdict() {
        let reg = CapRegistry::new();
        let auth = Accounts::boot(AccountsSnapshot::empty(), Arc::new(reg)).unwrap();
        let op = auth.stub_operator().unwrap();

        assert!(auth.verdict_for(Some(op), false).is_su());
        assert!(
            !auth.verdict_for(Some(op), true).is_su(),
            "quelled, the same account is evaluated without su"
        );
    }

    #[test]
    fn guest_verdict_has_no_authority() {
        let mut reg = CapRegistry::new();
        let build = reg.register_cap("build");
        let auth = Accounts::boot(AccountsSnapshot::empty(), Arc::new(reg)).unwrap();
        let v = auth.verdict_for(None, false);
        assert!(!v.is_su());
        assert!(!v.permits(build), "a guest holds no capability");
    }

    #[test]
    fn grants_resolve_to_the_registry_ids() {
        let mut reg = CapRegistry::new();
        let build = reg.register_cap("build");
        let ban = reg.register_cap("ban");
        // A populated store: one su operator plus a plain builder granted only "build".
        let store = AccountsSnapshot::from_records(vec![
            record(1, &[], true),
            record(2, &["build"], false),
        ]);
        let auth = Accounts::boot(store, Arc::new(reg)).unwrap();

        let builder = auth.verdict_for(Some(AccountId(2)), false);
        assert!(builder.permits(build), "granted cap admitted");
        assert!(!builder.permits(ban), "ungranted cap refused");
        assert!(!builder.is_su(), "a plain account is not su");
    }

    #[test]
    fn populated_store_with_zero_su_refuses_to_boot() {
        let mut reg = CapRegistry::new();
        reg.register_cap("build");
        let store = AccountsSnapshot::from_records(vec![record(1, &["build"], false)]);
        assert_eq!(
            Accounts::boot(store, Arc::new(reg)).err(),
            Some(AuthError::NoSuperuser),
            "a populated store with no su refuses rather than minting one"
        );
    }

    #[test]
    fn unknown_grant_refuses_to_boot() {
        let reg = CapRegistry::new(); // never registers "build"
        let store = AccountsSnapshot::from_records(vec![record(1, &["build"], true)]);
        assert_eq!(
            Accounts::boot(store, Arc::new(reg)).err(),
            Some(AuthError::UnknownGrant {
                account: AccountId(1),
                name: "build".into(),
            }),
            "an unknown grant string is a hard error, not a silent drop"
        );
    }

    #[test]
    fn records_round_trip_through_the_authority() {
        let mut reg = CapRegistry::new();
        reg.register_cap("build");
        let store = AccountsSnapshot::from_records(vec![
            record(1, &[], true),
            record(2, &["build"], false),
        ]);
        let auth = Accounts::boot(store, Arc::new(reg)).unwrap();

        let snap = auth.snapshot();
        let mut out = snap.records;
        out.sort_by_key(|r| r.id.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, AccountId(1));
        assert_eq!(out[0].handle, "acct1");
        assert!(out[0].is_su);
        assert_eq!(out[1].caps, vec!["build".to_string()]);
        assert_eq!(snap.next_id, 3, "the high-water mark rides the snapshot");
    }

    #[test]
    fn quellable_caps_drop_under_quell_but_baseline_survives() {
        let mut reg = CapRegistry::new();
        let build = reg.register_cap("build"); // quellable (the default)
        let member = reg.register_baseline_cap("member"); // survives quell
        let store = AccountsSnapshot::from_records(vec![
            record(1, &[], true),
            AccountRecord {
                id: AccountId(2),
                handle: "builder".into(),
                caps: vec!["build".into(), "member".into()],
                is_su: false,
            },
        ]);
        let auth = Accounts::boot(store, Arc::new(reg)).unwrap();

        let live = auth.verdict_for(Some(AccountId(2)), false);
        assert!(
            live.permits(build) && live.permits(member),
            "both held live"
        );

        let quelled = auth.verdict_for(Some(AccountId(2)), true);
        assert!(!quelled.permits(build), "an elevated cap drops under quell");
        assert!(quelled.permits(member), "a baseline cap survives quell");
        assert!(!quelled.is_su());
    }

    #[test]
    fn runtime_grant_then_revoke_tracks_the_verdict() {
        let mut reg = CapRegistry::new();
        let build = reg.register_cap("build");
        let mut auth = Accounts::boot(AccountsSnapshot::empty(), Arc::new(reg)).unwrap();

        let builder = auth.create_account("builder");
        assert_eq!(auth.account_by_handle("builder"), Some(builder));
        assert!(!auth.verdict_for(Some(builder), false).permits(build));

        auth.grant(builder, "build").unwrap();
        assert!(auth.verdict_for(Some(builder), false).permits(build));
        // Quellable by default, so a quelled builder sets it aside.
        assert!(!auth.verdict_for(Some(builder), true).permits(build));

        auth.revoke(builder, "build").unwrap();
        assert!(!auth.verdict_for(Some(builder), false).permits(build));
    }

    #[test]
    fn mutations_mark_the_authority_dirty() {
        let mut reg = CapRegistry::new();
        reg.register_cap("build");

        // The boot-time bootstrap is a mutation like any other: if it did not mark
        // dirty, the operator would exist only in memory until the next grant.
        let mut auth = Accounts::boot(AccountsSnapshot::empty(), Arc::new(reg)).unwrap();
        assert!(auth.take_dirty(), "bootstrap must be persisted");
        assert!(!auth.take_dirty(), "taking clears the flag");

        let builder = auth.create_account("builder");
        assert!(auth.take_dirty(), "create marks dirty");
        auth.grant(builder, "build").unwrap();
        assert!(auth.take_dirty(), "grant marks dirty");
        auth.revoke(builder, "build").unwrap();
        assert!(auth.take_dirty(), "revoke marks dirty");
    }

    #[test]
    fn a_loaded_boot_starts_clean() {
        let mut reg = CapRegistry::new();
        reg.register_cap("build");
        let store = AccountsSnapshot::from_records(vec![record(1, &[], true)]);
        let mut auth = Accounts::boot(store, Arc::new(reg)).unwrap();
        // Nothing mutated: booting from a populated store must not trigger a
        // pointless save of what was just read.
        assert!(!auth.take_dirty());
    }

    #[test]
    fn grant_of_an_unregistered_cap_errors() {
        let reg = CapRegistry::new(); // never registers "build"
        let mut auth = Accounts::boot(AccountsSnapshot::empty(), Arc::new(reg)).unwrap();
        let acc = auth.create_account("someone");
        assert_eq!(
            auth.grant(acc, "build"),
            Err(AccountError::UnknownCap("build".into())),
            "granting an unknown cap is a typed error, not a silent no-op"
        );
    }
}
