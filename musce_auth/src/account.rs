//! The account record: the engine's minimal identity and authorization surface for
//! one account.
//!
//! Deliberately dumb and app-agnostic. It holds an opaque credential hash, a set of
//! capability *names* (resolving those to the running app's capability ids is a
//! higher layer's job, since only the app knows its own vocabulary), the two
//! engine-owned authorization axes it enforces itself (a superuser bit and a binary
//! status), and an opaque `app_data` value the engine stores but never reads.
//! Everything richer (approval workflows, subscriber tiers, ban reasons) lives in
//! `app_data` as the app's own machinery; the engine ships only the floor it must
//! enforce.

use musce_core::{Map, Value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A stable, opaque account id: a v7 UUID minted at creation, never reused and never
/// rename-cascaded. The human-facing login name is [`Account::username`], which is
/// free to change precisely because it is not this key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountId(Uuid);

impl AccountId {
    /// Mint a fresh id. v7 is time-ordered, so ids sort by creation and index well.
    pub fn generate() -> Self {
        AccountId(Uuid::now_v7())
    }
}

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for AccountId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<Uuid>().map(AccountId)
    }
}

/// The engine-owned account status: the one authorization axis the engine enforces
/// itself, because "may this account authenticate at all" is a security floor it
/// cannot delegate to app code. Everything finer-grained is the app's, expressed
/// through its login veto and `app_data`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountStatus {
    /// May authenticate and hold authority, subject to the app's login veto.
    #[default]
    Active,
    /// The engine hard-refuses authentication. Absolute: the app's veto can further
    /// restrict an `Active` account but cannot lift `Disabled`.
    Disabled,
}

impl AccountStatus {
    /// The stable string form, the same tag serde emits, held by the store column.
    pub fn as_str(self) -> &'static str {
        match self {
            AccountStatus::Active => "active",
            AccountStatus::Disabled => "disabled",
        }
    }
}

/// A stored status string named no known status: corrupt or forward-version data.
#[derive(Debug)]
pub struct ParseStatusError(String);

impl std::fmt::Display for ParseStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown account status {:?}", self.0)
    }
}

impl std::error::Error for ParseStatusError {}

impl std::str::FromStr for AccountStatus {
    type Err = ParseStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(AccountStatus::Active),
            "disabled" => Ok(AccountStatus::Disabled),
            other => Err(ParseStatusError(other.to_owned())),
        }
    }
}

/// One account. Constructed via [`Account::new`] and mutated through focused
/// setters; a small record does not need a builder.
///
/// `username` is unique across accounts, but the record cannot enforce that on its
/// own: uniqueness is a store-level constraint plus a check at creation time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    id: AccountId,
    username: String,
    credential: Option<String>,
    caps: Vec<String>,
    /// Superuser: the engine's authorization bypass. A distinct axis, not a cap, so
    /// it maps onto the verdict's su override and stays off the generic grant path.
    su: bool,
    status: AccountStatus,
    app_data: Value,
}

impl Account {
    /// A fresh account: minted id, the given username, not su, `Active`, no
    /// password, no caps, and an empty `app_data` object.
    pub fn new(username: impl Into<String>) -> Self {
        Account {
            id: AccountId::generate(),
            username: username.into(),
            credential: None,
            caps: Vec::new(),
            su: false,
            status: AccountStatus::Active,
            app_data: Value::Object(Map::new()),
        }
    }

    /// Rebuild a record from stored parts: the persistence layer's rehydration path.
    /// Application code creates accounts with [`Account::new`] (which mints a fresh
    /// id) and never sets an id from outside; this is the one place a stored id is
    /// restored onto a record.
    pub fn from_stored(
        id: AccountId,
        username: String,
        credential: Option<String>,
        caps: Vec<String>,
        su: bool,
        status: AccountStatus,
        app_data: Value,
    ) -> Self {
        Account {
            id,
            username,
            credential,
            caps,
            su,
            status,
            app_data,
        }
    }

    /// The stable surrogate key.
    pub fn id(&self) -> AccountId {
        self.id
    }

    /// The human login name. Mutable; not the key.
    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn set_username(&mut self, username: impl Into<String>) {
        self.username = username.into();
    }

    /// The stored PHC credential hash, or `None` for a credential-less account (the
    /// bootstrap operator, or a future external-auth account).
    pub fn credential(&self) -> Option<&str> {
        self.credential.as_deref()
    }

    /// Set or clear the credential. The value is an already-hashed PHC string (see
    /// the `password` module); this record never sees a plaintext password.
    pub fn set_credential(&mut self, hash: Option<String>) {
        self.credential = hash;
    }

    /// The capability names this account holds. Names, not ids: the record is a leaf
    /// that knows nothing of any app's interned capability vocabulary.
    pub fn caps(&self) -> &[String] {
        &self.caps
    }

    /// Add a capability name. Idempotent: returns whether it was newly added, so a
    /// double grant is a no-op reporting `false`. Whether the name is a real
    /// capability is a higher layer's check; the record just holds the string.
    pub fn grant(&mut self, cap: impl Into<String>) -> bool {
        let cap = cap.into();
        if self.caps.iter().any(|held| held == &cap) {
            false
        } else {
            self.caps.push(cap);
            true
        }
    }

    /// Remove a capability name; returns whether it was present.
    pub fn revoke(&mut self, cap: &str) -> bool {
        if let Some(i) = self.caps.iter().position(|held| held == cap) {
            self.caps.remove(i);
            true
        } else {
            false
        }
    }

    /// Whether this account is a superuser. The engine's authorization bypass, a
    /// distinct axis from caps: it feeds the verdict's su override and is set through
    /// its own guarded path, never `grant`.
    pub fn is_su(&self) -> bool {
        self.su
    }

    pub fn set_su(&mut self, su: bool) {
        self.su = su;
    }

    pub fn status(&self) -> AccountStatus {
        self.status
    }

    pub fn set_status(&mut self, status: AccountStatus) {
        self.status = status;
    }

    /// The opaque app-owned data. The engine persists and returns it but never reads
    /// its contents; the running app parses its own structure out of it.
    pub fn app_data(&self) -> &Value {
        &self.app_data
    }

    pub fn set_app_data(&mut self, data: Value) {
        self.app_data = data;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_active_bare_and_uniquely_identified() {
        let a = Account::new("alice");
        assert_eq!(a.username(), "alice");
        assert_eq!(a.status(), AccountStatus::Active);
        assert!(!a.is_su(), "a fresh account is never su");
        assert!(a.credential().is_none());
        assert!(a.caps().is_empty());
        assert_eq!(a.app_data(), &Value::Object(Map::new()));

        // Each account mints its own id, so two accounts with the same username are
        // still distinct records.
        let b = Account::new("alice");
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn grant_is_idempotent() {
        let mut a = Account::new("builder");
        assert!(a.grant("build"), "first grant is newly added");
        assert!(!a.grant("build"), "re-granting reports no change");
        assert_eq!(
            a.caps(),
            ["build".to_string()],
            "the cap is held exactly once"
        );
    }

    #[test]
    fn revoke_reports_presence() {
        let mut a = Account::new("builder");
        a.grant("build");
        assert!(a.revoke("build"), "revoking a held cap reports true");
        assert!(!a.revoke("build"), "revoking an absent cap reports false");
        assert!(a.caps().is_empty());
    }

    #[test]
    fn serde_roundtrips_through_the_engine_value() {
        // Round-trip through serde_json::Value, the engine's persistence currency,
        // to prove the record serializes cleanly with every field populated: the id
        // as a uuid string, the status lowercased, and app_data nested as real JSON.
        let mut a = Account::new("carol");
        a.set_credential(Some("$argon2id$v=19$m=19456,t=2,p=1$stub".into()));
        a.grant("build");
        a.grant("teleport");
        a.set_su(true);
        a.set_status(AccountStatus::Disabled);
        a.set_app_data(serde_json::json!({ "tier": "founder", "approved": true }));

        let value = serde_json::to_value(&a).unwrap();
        let back: Account = serde_json::from_value(value).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn account_id_display_and_parse_roundtrip() {
        let id = AccountId::generate();
        assert_eq!(id.to_string().parse::<AccountId>().unwrap(), id);
        assert!("not-a-uuid".parse::<AccountId>().is_err());
    }

    #[test]
    fn status_string_form_roundtrips_and_matches_serde() {
        for status in [AccountStatus::Active, AccountStatus::Disabled] {
            assert_eq!(status.as_str().parse::<AccountStatus>().unwrap(), status);
            // as_str is the tag serde emits, so the DB column and the JSON agree.
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::json!(status.as_str())
            );
        }
        assert!("bogus".parse::<AccountStatus>().is_err());
    }

    #[test]
    fn from_stored_reconstructs_an_identical_record() {
        // Mirror the persistence round-trip: pull an account apart into its columns,
        // rebuild it from those parts, and get an equal record back.
        let mut a = Account::new("dave");
        a.set_su(true);
        a.grant("build");
        a.set_status(AccountStatus::Disabled);
        a.set_app_data(serde_json::json!({ "note": "hi" }));

        let rebuilt = Account::from_stored(
            a.id(),
            a.username().to_owned(),
            a.credential().map(str::to_owned),
            a.caps().to_vec(),
            a.is_su(),
            a.status(),
            a.app_data().clone(),
        );
        assert_eq!(rebuilt, a);
    }

    #[test]
    fn status_serializes_lowercase() {
        // The status is an engine-owned closed set; its wire form is a stable
        // lowercase tag, not the Rust variant name.
        assert_eq!(
            serde_json::to_value(AccountStatus::Disabled).unwrap(),
            serde_json::json!("disabled")
        );
    }
}
