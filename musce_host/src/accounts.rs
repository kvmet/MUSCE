//! The off-thread account task and the ops it serves.
//!
//! Anything that touches the account store runs here, never on the sim thread: the
//! store is async, and credential hashing and verification are CPU-heavy enough
//! (argon2, on a blocking pool) that they must not stall the tick. The sim hands
//! this task an [`AccountOp`] and gets
//! an [`AccountOutcome`] back, which it applies against session state and feeds to
//! the connection. This is the account analogue of the cold-content task. See
//! `docs/architecture/authorization.md`.

use std::sync::Arc;

use musce_action::{CapRegistry, CapSet};
use musce_auth::{Account, AccountId, AccountStatus};
use musce_core::Value;
use musce_persistence::{AccountStore, WorldStore};
use musce_proto::ConnectionId;
use tokio::sync::mpsc::UnboundedReceiver;

/// The username of the account seeded when the store holds no superuser. `@operator`
/// authenticates as it (loopback-only) so a fresh world is administrable.
pub(crate) const OPERATOR_USERNAME: &str = "operator";

/// A restricted view of an account handed to the app's login veto: identity and the
/// app's own data, never the credential hash. The app reads `status`/`app_data` to
/// decide admission (approval workflows, region locks) without ever seeing the
/// secret.
pub struct AccountView<'a> {
    pub username: &'a str,
    pub status: AccountStatus,
    pub app_data: &'a Value,
}

/// The app's login veto: `Ok(())` admits, `Err(reason)` refuses (the reason is shown
/// to the connection). It can only *further* restrict an account the engine already
/// admitted; it cannot lift a hard refusal. An app that does not gate logins uses
/// `|_| Ok(())`.
pub type LoginVeto = fn(&AccountView) -> Result<(), String>;

/// An account operation the sim hands to the off-thread task. Each carries the
/// originating connection so the outcome routes back to it.
pub(crate) enum AccountOp {
    /// Authenticate `conn` as `username`. `password` is `None` only for the
    /// passwordless `@operator` bootstrap, which the floor has loopback-gated;
    /// `@login` always supplies a password, verified against the stored credential.
    Authenticate {
        conn: ConnectionId,
        username: String,
        password: Option<String>,
    },
    /// Create a new account with the given password (hashed off-thread here).
    Create {
        conn: ConnectionId,
        username: String,
        password: String,
    },
    /// Add or remove a capability on an account.
    Grant {
        conn: ConnectionId,
        target: String,
        cap: String,
        add: bool,
    },
    /// Change the password on `account` (the connection's own, self-service). The old
    /// password is verified before the new one is hashed and stored.
    SetPassword {
        conn: ConnectionId,
        account: AccountId,
        old: String,
        new: String,
    },
}

/// The resolved authorization to cache in a session: an account's id, its resolved
/// capabilities, and its superuser bit.
pub(crate) struct Authorization {
    pub account: AccountId,
    pub caps: CapSet,
    pub su: bool,
}

/// What an [`AccountOp`] produced. Always a line for the originating connection;
/// optionally a session binding (an authentication) or a refresh of some account's
/// cached authorization (a mutation that must reach an online target).
pub(crate) struct AccountOutcome {
    pub conn: ConnectionId,
    pub line: String,
    /// Present iff this op authenticated `conn`: bind it to this authorization.
    pub authenticated: Option<Authorization>,
    /// Present iff this op changed an account's authorization: refresh it wherever
    /// that account is online.
    pub refreshed: Option<Authorization>,
}

impl AccountOutcome {
    /// An outcome that only feeds a line back, changing no session state.
    fn line(conn: ConnectionId, line: impl Into<String>) -> Self {
        Self {
            conn,
            line: line.into(),
            authenticated: None,
            refreshed: None,
        }
    }
}

/// Serve account ops until the sim drops the sender. Holds its own store clone (a
/// pooled handle), the app's capability registry (to resolve grant names to a
/// `CapSet`), and the app's login veto.
pub(crate) async fn account_task(
    store: WorldStore,
    caps: Arc<CapRegistry>,
    veto: LoginVeto,
    mut ops: UnboundedReceiver<AccountOp>,
    outcomes: crossbeam_channel::Sender<AccountOutcome>,
) {
    while let Some(op) = ops.recv().await {
        let outcome = match op {
            AccountOp::Authenticate {
                conn,
                username,
                password,
            } => authenticate(&store, &caps, veto, conn, username, password).await,
            AccountOp::Create {
                conn,
                username,
                password,
            } => create(&store, conn, username, password).await,
            AccountOp::Grant {
                conn,
                target,
                cap,
                add,
            } => mutate(&store, &caps, conn, target, cap, add).await,
            AccountOp::SetPassword {
                conn,
                account,
                old,
                new,
            } => set_password(&store, conn, account, old, new).await,
        };
        if outcomes.send(outcome).is_err() {
            break; // the sim is gone; nothing more to serve
        }
    }
}

/// Resolve an account's grant names to a `CapSet`, logging any the vocabulary no
/// longer defines rather than dropping them silently.
fn resolve_caps(caps: &CapRegistry, account: &Account) -> CapSet {
    let (set, unknown) = caps.resolve_set(account.caps());
    if !unknown.is_empty() {
        tracing::warn!(
            username = account.username(),
            ?unknown,
            "account holds capability grants the vocabulary no longer defines"
        );
    }
    set
}

async fn authenticate(
    store: &WorldStore,
    caps: &CapRegistry,
    veto: LoginVeto,
    conn: ConnectionId,
    username: String,
    password: Option<String>,
) -> AccountOutcome {
    let account = match store.account_by_username(&username).await {
        Ok(Some(a)) => a,
        Ok(None) => return AccountOutcome::line(conn, format!("No account named \"{username}\".")),
        Err(e) => {
            tracing::error!(error = %e, username, "account lookup failed");
            return AccountOutcome::line(conn, "Authentication failed.");
        }
    };

    // Engine hard gate: a disabled account cannot authenticate, ahead of any app
    // policy, and the app veto below cannot lift it.
    if account.status() == AccountStatus::Disabled {
        return AccountOutcome::line(conn, "That account is disabled.");
    }

    // Credential check. A `None` password is the passwordless `@operator` bootstrap
    // (loopback-gated at the floor) and is valid only against a credential-less
    // account; every other combination is a real password verified off-thread, so
    // argon2 never runs on the async runtime's core threads.
    match (password, account.credential().map(str::to_owned)) {
        (None, None) => {}
        (None, Some(_)) => {
            return AccountOutcome::line(conn, "That account requires a password.");
        }
        (Some(_), None) => {
            return AccountOutcome::line(conn, "That account has no password set.");
        }
        (Some(pw), Some(hash)) => {
            match tokio::task::spawn_blocking(move || musce_auth::verify_password(&pw, &hash)).await
            {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) => return AccountOutcome::line(conn, "Incorrect password."),
                Ok(Err(e)) => {
                    // A stored hash that will not parse: corrupt data, never a wrong
                    // password, so it is logged and refused, not counted as a miss.
                    tracing::error!(error = %e, username, "stored credential is malformed");
                    return AccountOutcome::line(conn, "Authentication failed.");
                }
                Err(e) => {
                    tracing::error!(error = %e, username, "verify task failed");
                    return AccountOutcome::line(conn, "Authentication failed.");
                }
            }
        }
    }

    // App soft veto: may further restrict an otherwise-admitted account.
    let view = AccountView {
        username: account.username(),
        status: account.status(),
        app_data: account.app_data(),
    };
    if let Err(reason) = veto(&view) {
        return AccountOutcome::line(conn, reason);
    }

    let authz = Authorization {
        account: account.id(),
        caps: resolve_caps(caps, &account),
        su: account.is_su(),
    };
    AccountOutcome {
        conn,
        line: format!("You are now logged in as {username}."),
        authenticated: Some(authz),
        refreshed: None,
    }
}

async fn create(
    store: &WorldStore,
    conn: ConnectionId,
    username: String,
    password: String,
) -> AccountOutcome {
    match store.account_by_username(&username).await {
        Ok(Some(_)) => {
            return AccountOutcome::line(
                conn,
                format!("An account named \"{username}\" already exists."),
            );
        }
        Ok(None) => {}
        Err(e) => {
            tracing::error!(error = %e, username, "account lookup failed");
            return AccountOutcome::line(conn, "Could not create the account.");
        }
    }
    // Hash off-thread, like verification: argon2 is deliberately slow.
    let hash = match tokio::task::spawn_blocking(move || musce_auth::hash_password(&password)).await
    {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            tracing::error!(error = %e, username, "password hashing failed");
            return AccountOutcome::line(conn, "Could not create the account.");
        }
        Err(e) => {
            tracing::error!(error = %e, username, "hash task failed");
            return AccountOutcome::line(conn, "Could not create the account.");
        }
    };
    let mut account = Account::new(&username);
    account.set_credential(Some(hash));
    if let Err(e) = store.account_upsert(&account).await {
        tracing::error!(error = %e, username, "account create failed");
        return AccountOutcome::line(conn, "Could not create the account.");
    }
    AccountOutcome::line(conn, format!("Created account \"{username}\"."))
}

async fn set_password(
    store: &WorldStore,
    conn: ConnectionId,
    account_id: AccountId,
    old: String,
    new: String,
) -> AccountOutcome {
    let mut account = match store.account_by_id(&account_id).await {
        Ok(Some(a)) => a,
        // The session holds an authenticated id no account matches: stale or corrupt
        // state, never an ordinary path. Log and refuse rather than expose it.
        Ok(None) => {
            tracing::error!(%account_id, "password change for an account that no longer exists");
            return AccountOutcome::line(conn, "Could not change your password.");
        }
        Err(e) => {
            tracing::error!(error = %e, %account_id, "account lookup failed");
            return AccountOutcome::line(conn, "Could not change your password.");
        }
    };

    // Self-service *changes* an existing password. Setting a first password on a
    // passwordless account (the operator, future external-auth accounts) is a
    // different, out-of-scope operation.
    let Some(hash) = account.credential().map(str::to_owned) else {
        return AccountOutcome::line(conn, "This account has no password to change.");
    };

    // Verify the current password off-thread, exactly as login does; a corrupt stored
    // hash is a fault, never a wrong password.
    match tokio::task::spawn_blocking(move || musce_auth::verify_password(&old, &hash)).await {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => return AccountOutcome::line(conn, "Incorrect password."),
        Ok(Err(e)) => {
            tracing::error!(error = %e, %account_id, "stored credential is malformed");
            return AccountOutcome::line(conn, "Could not change your password.");
        }
        Err(e) => {
            tracing::error!(error = %e, %account_id, "verify task failed");
            return AccountOutcome::line(conn, "Could not change your password.");
        }
    }

    // Hash the new password off-thread and persist it. Authorization is unchanged, so
    // no session refresh: existing live sessions keep running under their cached caps.
    let new_hash = match tokio::task::spawn_blocking(move || musce_auth::hash_password(&new)).await
    {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            tracing::error!(error = %e, %account_id, "password hashing failed");
            return AccountOutcome::line(conn, "Could not change your password.");
        }
        Err(e) => {
            tracing::error!(error = %e, %account_id, "hash task failed");
            return AccountOutcome::line(conn, "Could not change your password.");
        }
    };
    account.set_credential(Some(new_hash));
    if let Err(e) = store.account_upsert(&account).await {
        tracing::error!(error = %e, %account_id, "password change failed");
        return AccountOutcome::line(conn, "Could not change your password.");
    }
    AccountOutcome::line(conn, "Password changed.")
}

async fn mutate(
    store: &WorldStore,
    caps: &CapRegistry,
    conn: ConnectionId,
    target: String,
    cap: String,
    add: bool,
) -> AccountOutcome {
    // A grant of a name outside the app's vocabulary is a typo, not a stored grant:
    // refuse it up front rather than persist a cap no gate will ever check.
    if caps.resolve(&cap).is_none() {
        return AccountOutcome::line(conn, format!("There is no capability named \"{cap}\"."));
    }

    let mut account = match store.account_by_username(&target).await {
        Ok(Some(a)) => a,
        Ok(None) => return AccountOutcome::line(conn, format!("No account named \"{target}\".")),
        Err(e) => {
            tracing::error!(error = %e, target, "account lookup failed");
            return AccountOutcome::line(conn, "Could not update the account.");
        }
    };

    let changed = if add {
        account.grant(&cap)
    } else {
        account.revoke(&cap)
    };
    if !changed {
        let state = if add { "already has" } else { "does not have" };
        return AccountOutcome::line(conn, format!("{target} {state} \"{cap}\"."));
    }

    if let Err(e) = store.account_upsert(&account).await {
        tracing::error!(error = %e, target, "account update failed");
        return AccountOutcome::line(conn, "Could not update the account.");
    }

    let (verb, prep) = if add {
        ("Granted", "to")
    } else {
        ("Revoked", "from")
    };
    AccountOutcome {
        conn,
        line: format!("{verb} \"{cap}\" {prep} {target}."),
        authenticated: None,
        refreshed: Some(Authorization {
            account: account.id(),
            caps: resolve_caps(caps, &account),
            su: account.is_su(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_persistence::AccountStore;

    async fn store() -> WorldStore {
        let s = WorldStore::connect("sqlite::memory:").await.unwrap();
        s.accounts_init().await.unwrap();
        s
    }

    fn caps() -> CapRegistry {
        let mut c = CapRegistry::new();
        c.register("build");
        c
    }

    fn admit(_: &AccountView) -> Result<(), String> {
        Ok(())
    }

    #[tokio::test]
    async fn authenticate_binds_a_known_account_with_resolved_caps() {
        let store = store().await;
        let mut acc = Account::new("builder");
        acc.grant("build");
        store.account_upsert(&acc).await.unwrap();

        // A passwordless account authenticates via the `None` (operator/stub) path.
        let out = authenticate(
            &store,
            &caps(),
            admit,
            ConnectionId(1),
            "builder".into(),
            None,
        )
        .await;
        let authz = out.authenticated.expect("a known account authenticates");
        assert_eq!(authz.account, acc.id());
        assert!(!authz.su);
        assert_eq!(authz.caps.len(), 1, "the build grant resolved");
    }

    #[tokio::test]
    async fn create_hashes_a_password_and_login_verifies_it() {
        let store = store().await;
        create(&store, ConnectionId(1), "alice".into(), "hunter2".into()).await;

        // The correct password binds.
        let ok = authenticate(
            &store,
            &caps(),
            admit,
            ConnectionId(1),
            "alice".into(),
            Some("hunter2".into()),
        )
        .await;
        assert!(
            ok.authenticated.is_some(),
            "the right password authenticates"
        );

        // The wrong password is a clean refusal, not an error.
        let bad = authenticate(
            &store,
            &caps(),
            admit,
            ConnectionId(1),
            "alice".into(),
            Some("nope".into()),
        )
        .await;
        assert!(bad.authenticated.is_none());
        assert!(bad.line.contains("Incorrect password"));
    }

    #[tokio::test]
    async fn password_login_refused_when_no_credential_is_set() {
        let store = store().await;
        store.account_upsert(&Account::new("noword")).await.unwrap();
        let out = authenticate(
            &store,
            &caps(),
            admit,
            ConnectionId(1),
            "noword".into(),
            Some("x".into()),
        )
        .await;
        assert!(out.authenticated.is_none());
        assert!(out.line.contains("no password set"));
    }

    #[tokio::test]
    async fn passwordless_stub_refused_against_a_password_account() {
        let store = store().await;
        create(&store, ConnectionId(1), "alice".into(), "hunter2".into()).await;
        // The `@operator` stub path (no password) cannot claim a password account.
        let out = authenticate(
            &store,
            &caps(),
            admit,
            ConnectionId(1),
            "alice".into(),
            None,
        )
        .await;
        assert!(out.authenticated.is_none());
        assert!(out.line.contains("requires a password"));
    }

    #[tokio::test]
    async fn authenticate_refuses_an_unknown_account() {
        let out = authenticate(
            &store().await,
            &caps(),
            admit,
            ConnectionId(1),
            "ghost".into(),
            None,
        )
        .await;
        assert!(out.authenticated.is_none());
        assert!(out.line.contains("No account"));
    }

    #[tokio::test]
    async fn engine_gate_refuses_a_disabled_account_before_the_veto() {
        let store = store().await;
        let mut acc = Account::new("banned");
        acc.set_status(AccountStatus::Disabled);
        store.account_upsert(&acc).await.unwrap();

        // A veto that would admit anything still never runs: the hard gate is first.
        let out = authenticate(
            &store,
            &caps(),
            admit,
            ConnectionId(1),
            "banned".into(),
            None,
        )
        .await;
        assert!(out.authenticated.is_none());
        assert!(out.line.contains("disabled"));
    }

    #[tokio::test]
    async fn app_veto_can_refuse_an_active_account() {
        fn deny(_: &AccountView) -> Result<(), String> {
            Err("pending approval".into())
        }
        let store = store().await;
        store.account_upsert(&Account::new("newbie")).await.unwrap();

        let out = authenticate(
            &store,
            &caps(),
            deny,
            ConnectionId(1),
            "newbie".into(),
            None,
        )
        .await;
        assert!(out.authenticated.is_none());
        assert!(out.line.contains("pending approval"));
    }

    #[tokio::test]
    async fn set_password_changes_the_credential_when_the_old_one_matches() {
        let store = store().await;
        create(&store, ConnectionId(1), "alice".into(), "old-pw".into()).await;
        let id = store
            .account_by_username("alice")
            .await
            .unwrap()
            .unwrap()
            .id();

        let out = set_password(
            &store,
            ConnectionId(1),
            id,
            "old-pw".into(),
            "new-pw".into(),
        )
        .await;
        assert!(out.line.contains("Password changed"));

        // The new password now authenticates and the old one no longer does.
        let with_new = authenticate(
            &store,
            &caps(),
            admit,
            ConnectionId(1),
            "alice".into(),
            Some("new-pw".into()),
        )
        .await;
        assert!(with_new.authenticated.is_some(), "the new password works");

        let with_old = authenticate(
            &store,
            &caps(),
            admit,
            ConnectionId(1),
            "alice".into(),
            Some("old-pw".into()),
        )
        .await;
        assert!(with_old.authenticated.is_none(), "the old password is dead");
    }

    #[tokio::test]
    async fn set_password_refuses_a_wrong_old_password() {
        let store = store().await;
        create(&store, ConnectionId(1), "alice".into(), "old-pw".into()).await;
        let id = store
            .account_by_username("alice")
            .await
            .unwrap()
            .unwrap()
            .id();

        let out = set_password(&store, ConnectionId(1), id, "wrong".into(), "new-pw".into()).await;
        assert!(out.line.contains("Incorrect password"));

        // The password is unchanged: the original still authenticates.
        let still = authenticate(
            &store,
            &caps(),
            admit,
            ConnectionId(1),
            "alice".into(),
            Some("old-pw".into()),
        )
        .await;
        assert!(still.authenticated.is_some(), "a failed change is a no-op");
    }

    #[tokio::test]
    async fn set_password_refuses_a_passwordless_account() {
        let store = store().await;
        let acc = Account::new("noword");
        store.account_upsert(&acc).await.unwrap();

        let out = set_password(
            &store,
            ConnectionId(1),
            acc.id(),
            "anything".into(),
            "new-pw".into(),
        )
        .await;
        assert!(out.line.contains("no password to change"));
    }

    #[tokio::test]
    async fn grant_persists_and_refreshes() {
        let store = store().await;
        store
            .account_upsert(&Account::new("builder"))
            .await
            .unwrap();

        let out = mutate(
            &store,
            &caps(),
            ConnectionId(1),
            "builder".into(),
            "build".into(),
            true,
        )
        .await;
        let authz = out.refreshed.expect("a grant refreshes the target");
        assert_eq!(authz.caps.len(), 1);

        // It is durable: a re-fetch holds the grant.
        let back = store.account_by_username("builder").await.unwrap().unwrap();
        assert_eq!(back.caps(), ["build".to_string()]);
    }

    #[tokio::test]
    async fn grant_of_an_unknown_capability_is_refused() {
        let store = store().await;
        store
            .account_upsert(&Account::new("builder"))
            .await
            .unwrap();

        let out = mutate(
            &store,
            &caps(),
            ConnectionId(1),
            "builder".into(),
            "nonesuch".into(),
            true,
        )
        .await;
        assert!(out.refreshed.is_none(), "an unknown cap is never persisted");
        assert!(out.line.contains("no capability"));
    }
}
