//! The account authority: who exists, what each account may do, and how an account
//! resolves to an authorization [`Verdict`]. A cohesive unit (the caps registry, the
//! account records, the store, and the live authority) that lifts to a `musce_auth`
//! crate as one piece when a second consumer lands. It depends on `musce_action`
//! only for the check vocabulary it produces (`CapId`/`CapSet`/`Verdict`) and knows
//! nothing of dispatch, `Sessions`, or the `World`. See
//! `docs/architecture/accounts.md`.

use std::collections::BTreeMap;
use std::collections::HashMap;

use musce_action::{CapId, CapSet, Verdict};

/// The version stamped on a freshly written account record, so the record is
/// self-describing and a later durable backend can migrate its shape. Bumped when
/// the record's fields change; no migrations exist yet.
pub const RECORD_VERSION: u32 = 1;

/// Stable account identity: persisted, the store's primary key. Distinct from the
/// ephemeral `ConnectionId` and from any `EntityId` (accounts are not world
/// entities), so the two never get confused at a call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(pub u64);

/// The persisted form of an account: identity, grants **by string name** (stable
/// across id churn, and the game's vocabulary rather than the engine's), the
/// superuser bit, and a self-describing version. The live authority resolves the
/// grant strings to `CapId`s at load against the same registry the gates were built
/// from.
#[derive(Clone, Debug)]
pub struct AccountRecord {
    pub id: AccountId,
    pub caps: Vec<String>,
    pub is_su: bool,
    pub version: u32,
}

/// The durable backing for accounts: load every record at boot, save the full set.
/// Takes no `World` and no host types, so it lifts with the rest of the auth module.
/// The slice-1 backend is in-memory ([`MemoryAccountStore`]); a durable backend with
/// its own storage home (not the entities table) lands with authentication.
pub trait AccountStore {
    /// Every persisted record. The slice-1 in-memory backend cannot fail; a durable
    /// backend's load error is a refuse-to-boot at the call site, never treated as an
    /// empty store.
    fn load(&self) -> Vec<AccountRecord>;
    /// Replace the persisted set with `records`.
    fn save(&mut self, records: &[AccountRecord]);
}

/// The slice-1 account backend: records in memory, no durability across a restart.
/// The `AccountStore` trait is the reserved seam; a durable backend is slice 2's,
/// alongside authentication. Constructible with seed records so a test can drive the
/// populated boot cases.
#[derive(Default)]
pub struct MemoryAccountStore {
    records: Vec<AccountRecord>,
}

impl MemoryAccountStore {
    /// An empty store: first boot bootstraps its operator.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store pre-populated with `records`, for exercising the populated boot paths.
    pub fn with_records(records: Vec<AccountRecord>) -> Self {
        Self { records }
    }
}

impl AccountStore for MemoryAccountStore {
    fn load(&self) -> Vec<AccountRecord> {
        self.records.clone()
    }

    fn save(&mut self, records: &[AccountRecord]) {
        self.records = records.to_vec();
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
}

impl CapRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern a capability name, returning its stable id. Idempotent: the same name
    /// always returns the same id, so registering a gate and later resolving a grant
    /// of the same name agree.
    pub fn register_cap(&mut self, name: &str) -> CapId {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = CapId(self.names.len() as u32);
        self.names.push(name.to_string());
        self.ids.insert(name.to_string(), id);
        id
    }

    /// Resolve a name registered earlier. `None` if it was never registered, which
    /// the authority treats as a hard load error rather than a silent drop.
    pub fn resolve(&self, name: &str) -> Option<CapId> {
        self.ids.get(name).copied()
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

/// One live account in the authority: its grants resolved to ids, and its su bit.
/// The grant strings are kept so the record round-trips back to the store unchanged.
struct Account {
    caps: CapSet,
    grants: Vec<String>,
    is_su: bool,
}

/// The live account authority: sim-thread-owned, mirroring World-as-truth. Holds each
/// account's resolved caps and su bit in memory and answers `verdict_for` at dispatch.
/// The at-least-one-superuser invariant holds across boot (bootstrap or refuse); a
/// runtime grant/su-write surface, and the per-mutation floor it needs, lands with
/// slice 2 (slice 1 exposes no such surface, so there is nothing to guard yet).
pub struct Accounts {
    accounts: BTreeMap<AccountId, Account>,
    next_id: u64,
}

impl Accounts {
    /// Build the authority from a store, resolving each record's grant strings to
    /// `CapId`s against `registry`. Enforces the boot cases: an **empty** store
    /// bootstraps one su operator; a **populated** store with **zero** su refuses; an
    /// **unknown grant** refuses. (A store *load* error is the caller's refuse-to-boot,
    /// never routed here as empty.)
    pub fn boot(store: &impl AccountStore, registry: &CapRegistry) -> Result<Self, AuthError> {
        let mut accounts = BTreeMap::new();
        let mut max_id = 0;
        for rec in store.load() {
            let mut caps = CapSet::new();
            for name in &rec.caps {
                let id = registry
                    .resolve(name)
                    .ok_or_else(|| AuthError::UnknownGrant {
                        account: rec.id,
                        name: name.clone(),
                    })?;
                caps.insert(id);
            }
            max_id = max_id.max(rec.id.0);
            accounts.insert(
                rec.id,
                Account {
                    caps,
                    grants: rec.caps,
                    is_su: rec.is_su,
                },
            );
        }

        let mut authority = Accounts {
            accounts,
            next_id: max_id + 1,
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

    /// Create an account. The **first** account in an empty authority is superuser by
    /// default (the bit written here and carried on the record, never re-derived from
    /// ordering at load); every later one starts with no grants and no su.
    fn create(&mut self) -> AccountId {
        let id = AccountId(self.next_id);
        self.next_id += 1;
        let is_su = self.accounts.is_empty();
        self.accounts.insert(
            id,
            Account {
                caps: CapSet::new(),
                grants: Vec::new(),
                is_su,
            },
        );
        id
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

    /// The verdict a connection runs under: its account's caps, with su in force
    /// unless the connection is quelled. No account (a guest) is the empty, no-su
    /// verdict. Keyed off the account, never an actor, so no possessed body can borrow
    /// authority.
    pub fn verdict_for(&self, account: Option<AccountId>, quelled: bool) -> Verdict {
        match account.and_then(|id| self.accounts.get(&id)) {
            Some(a) => Verdict::new(a.caps.clone(), a.is_su && !quelled),
            None => Verdict::guest(),
        }
    }

    /// The current authority as records, for a store save.
    pub fn to_records(&self) -> Vec<AccountRecord> {
        self.accounts
            .iter()
            .map(|(&id, a)| AccountRecord {
                id,
                caps: a.grants.clone(),
                is_su: a.is_su,
                version: RECORD_VERSION,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: u64, caps: &[&str], is_su: bool) -> AccountRecord {
        AccountRecord {
            id: AccountId(id),
            caps: caps.iter().map(|s| s.to_string()).collect(),
            is_su,
            version: RECORD_VERSION,
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
        let store = MemoryAccountStore::new();
        let auth = Accounts::boot(&store, &reg).unwrap();

        assert_eq!(auth.su_count(), 1, "bootstrap lays down exactly one su");
        let op = auth.stub_operator().expect("a bootstrapped operator");
        assert!(
            auth.verdict_for(Some(op), false).is_su(),
            "the operator runs as su"
        );
    }

    #[test]
    fn quell_drops_su_from_the_operator_verdict() {
        let reg = CapRegistry::new();
        let store = MemoryAccountStore::new();
        let auth = Accounts::boot(&store, &reg).unwrap();
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
        let auth = Accounts::boot(&MemoryAccountStore::new(), &reg).unwrap();
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
        let store = MemoryAccountStore::with_records(vec![
            record(1, &[], true),
            record(2, &["build"], false),
        ]);
        let auth = Accounts::boot(&store, &reg).unwrap();

        let builder = auth.verdict_for(Some(AccountId(2)), false);
        assert!(builder.permits(build), "granted cap admitted");
        assert!(!builder.permits(ban), "ungranted cap refused");
        assert!(!builder.is_su(), "a plain account is not su");
    }

    #[test]
    fn populated_store_with_zero_su_refuses_to_boot() {
        let mut reg = CapRegistry::new();
        reg.register_cap("build");
        let store = MemoryAccountStore::with_records(vec![record(1, &["build"], false)]);
        assert_eq!(
            Accounts::boot(&store, &reg).err(),
            Some(AuthError::NoSuperuser),
            "a populated store with no su refuses rather than minting one"
        );
    }

    #[test]
    fn unknown_grant_refuses_to_boot() {
        let reg = CapRegistry::new(); // never registers "build"
        let store = MemoryAccountStore::with_records(vec![record(1, &["build"], true)]);
        assert_eq!(
            Accounts::boot(&store, &reg).err(),
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
        let store = MemoryAccountStore::with_records(vec![
            record(1, &[], true),
            record(2, &["build"], false),
        ]);
        let auth = Accounts::boot(&store, &reg).unwrap();

        let mut out = auth.to_records();
        out.sort_by_key(|r| r.id.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, AccountId(1));
        assert!(out[0].is_su);
        assert_eq!(out[1].caps, vec!["build".to_string()]);
        assert_eq!(out[1].version, RECORD_VERSION);
    }
}
