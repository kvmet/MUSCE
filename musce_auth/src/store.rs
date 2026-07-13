//! The durable account store: a small relational SQLite database of its own,
//! separate from the world DB (which a dev reseed deletes). Accounts are
//! first-class queryable rows, unlike the world's opaque entity blobs, because
//! consumers beyond the sim host (a web or oauth frontend, admin tooling) read the
//! same account set, and the account id is the foreign-key target for provenance
//! and a later characters table. Runtime reads never come here: the sim reads the
//! in-memory authority, and an async writer task feeds full-snapshot saves.

use std::str::FromStr;

use sqlx::Row;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

use crate::{AccountId, AccountRecord, AccountsSnapshot};

/// The schema version a freshly written store carries. Bumped when the tables
/// change shape; a stored version this build does not know refuses to load rather
/// than guessing (no migrations exist yet).
pub const ACCOUNTS_SCHEMA_VERSION: u32 = 1;

const NEXT_ID_KEY: &str = "next_id";
const SCHEMA_VERSION_KEY: &str = "schema_version";

/// Why the store could not be read or written. A load error at boot is a
/// refuse-to-boot at the call site, never treated as an empty store.
#[derive(Debug)]
pub enum StoreError {
    Sqlx(sqlx::Error),
    /// The store was written at a schema version this build does not know how to
    /// read. Refusing is honest: no migration exists, and loading a guessed shape
    /// could silently drop grants.
    SchemaVersion {
        stored: u32,
        current: u32,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Sqlx(e) => write!(f, "account store: {e}"),
            StoreError::SchemaVersion { stored, current } => write!(
                f,
                "account store schema version {stored} is unknown to this build (current {current})"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> Self {
        StoreError::Sqlx(e)
    }
}

/// The account tables, written once so the two backends cannot drift apart
/// structurally. Only the dialect's type words vary: `int_ty` for the 64-bit id
/// columns (`INTEGER`/`BIGINT`) and `bool_ty` for the superuser flag
/// (`INTEGER`/`BOOLEAN`, matching how sqlx maps a Rust `bool` per backend).
fn accounts_tables_ddl(int_ty: &str, bool_ty: &str) -> [String; 3] {
    [
        format!(
            "CREATE TABLE IF NOT EXISTS accounts (
                id     {int_ty} PRIMARY KEY,
                handle TEXT UNIQUE NOT NULL,
                is_su  {bool_ty} NOT NULL
            )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS account_caps (
                account_id {int_ty} NOT NULL REFERENCES accounts(id),
                cap_name   TEXT NOT NULL,
                PRIMARY KEY (account_id, cap_name)
            )"
        ),
        "CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )"
        .to_string(),
    ]
}

/// Refuse a store written at a schema version this build cannot read. A missing
/// marker (a fresh store) reads as current; a real mismatch refuses rather than
/// guess a shape and silently drop grants. Backend-free, so both stores refuse
/// identically.
fn check_schema_version(stored: Option<u32>) -> Result<(), StoreError> {
    let stored = stored.unwrap_or(ACCOUNTS_SCHEMA_VERSION);
    if stored != ACCOUNTS_SCHEMA_VERSION {
        return Err(StoreError::SchemaVersion {
            stored,
            current: ACCOUNTS_SCHEMA_VERSION,
        });
    }
    Ok(())
}

/// Reassemble the rows a backend read into an `AccountsSnapshot`, independent of
/// the store engine. Records arrive ordered by id and caps ordered by
/// `(account_id, cap_name)`, so caps land in a stable alphabetical order per
/// account. `next_id` never falls to or below a live id: the persisted marker is
/// authoritative (it outlives deleted accounts), the max-row floor only defends a
/// store whose meta row went missing. Unit-tested without a database.
fn assemble_accounts(
    account_rows: Vec<(i64, String, bool)>,
    cap_rows: Vec<(i64, String)>,
    marker: Option<u64>,
) -> AccountsSnapshot {
    let mut records: Vec<AccountRecord> = account_rows
        .into_iter()
        .map(|(id, handle, is_su)| AccountRecord {
            id: AccountId(id as u64),
            handle,
            caps: Vec::new(),
            is_su,
        })
        .collect();
    for (account_id, cap_name) in cap_rows {
        let id = AccountId(account_id as u64);
        if let Some(rec) = records.iter_mut().find(|r| r.id == id) {
            rec.caps.push(cap_name);
        }
    }
    let floor = records.iter().map(|r| r.id.0 + 1).max().unwrap_or(1);
    let next_id = marker.unwrap_or(1).max(floor);
    AccountsSnapshot { records, next_id }
}

/// The SQLite-backed account store. Save/load only: `load` once at boot, `save`
/// whole snapshots from the writer task. Cheap to clone (the pool is shared).
#[derive(Clone)]
pub struct AccountStore {
    pool: SqlitePool,
}

impl AccountStore {
    /// Connect (creating the file if missing). Use `"sqlite::memory:"` for an
    /// in-memory database. A single connection keeps the writer serialized and
    /// keeps in-memory databases consistent across queries.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        let opts = SqliteConnectOptions::from_str(url)?.create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        Ok(Self { pool })
    }

    /// Create tables if absent.
    pub async fn init(&self) -> Result<(), StoreError> {
        for ddl in accounts_tables_ddl("INTEGER", "INTEGER") {
            sqlx::query(sqlx::AssertSqlSafe(ddl))
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// Replace the stored set with `snapshot`, in one transaction. Every save is
    /// the full snapshot (the set is tiny and mutations are rare admin ops), so a
    /// save is idempotent and self-healing: whatever a prior failed write lost,
    /// the next write restores.
    pub async fn save(&self, snapshot: &AccountsSnapshot) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;

        // Children first, parents second: the foreign key is enforced.
        sqlx::query("DELETE FROM account_caps")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM accounts")
            .execute(&mut *tx)
            .await?;

        for rec in &snapshot.records {
            sqlx::query("INSERT INTO accounts (id, handle, is_su) VALUES (?, ?, ?)")
                .bind(rec.id.0 as i64)
                .bind(&rec.handle)
                .bind(rec.is_su)
                .execute(&mut *tx)
                .await?;
            for cap in &rec.caps {
                sqlx::query("INSERT INTO account_caps (account_id, cap_name) VALUES (?, ?)")
                    .bind(rec.id.0 as i64)
                    .bind(cap)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        sqlx::query(
            "INSERT INTO meta (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(NEXT_ID_KEY)
        .bind(snapshot.next_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO meta (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(SCHEMA_VERSION_KEY)
        .bind(ACCOUNTS_SCHEMA_VERSION.to_string())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Load the full snapshot. A fresh (never-saved) store loads as the empty
    /// snapshot, which boots into the operator bootstrap; a store written at an
    /// unknown schema version refuses.
    pub async fn load(&self) -> Result<AccountsSnapshot, StoreError> {
        check_schema_version(read_meta(&self.pool, SCHEMA_VERSION_KEY).await?)?;

        // Extract primitives, then let the backend-free `assemble_accounts` build
        // the snapshot and enforce the next_id floor. Ordered reads keep caps in a
        // stable alphabetical order per account.
        let account_rows = sqlx::query("SELECT id, handle, is_su FROM accounts ORDER BY id")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|r| {
                (
                    r.get::<i64, _>("id"),
                    r.get::<String, _>("handle"),
                    r.get::<bool, _>("is_su"),
                )
            })
            .collect();
        let cap_rows = sqlx::query(
            "SELECT account_id, cap_name FROM account_caps ORDER BY account_id, cap_name",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|r| {
            (
                r.get::<i64, _>("account_id"),
                r.get::<String, _>("cap_name"),
            )
        })
        .collect();
        let marker = read_meta(&self.pool, NEXT_ID_KEY).await?;

        Ok(assemble_accounts(account_rows, cap_rows, marker))
    }
}

/// Read a `meta` value and parse it, `None` when the row is missing or does not
/// parse. SQLite-side; the Postgres store has its own `$1`-placeholder twin.
async fn read_meta<T: FromStr>(pool: &SqlitePool, key: &str) -> Result<Option<T>, StoreError> {
    Ok(sqlx::query("SELECT value FROM meta WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?
        .map(|r| r.get::<String, _>("value"))
        .and_then(|s| s.parse().ok()))
}

/// The Postgres-backed account store. Same save/load contract as [`AccountStore`],
/// differing only in `$1` placeholders and the `BIGINT`/`BOOLEAN` schema words.
#[derive(Clone)]
pub struct PostgresAccountStore {
    pool: PgPool,
}

impl PostgresAccountStore {
    /// Connect to an existing Postgres database (no create-if-missing; the
    /// database is provisioned out of band). A single connection serializes the
    /// writer, mirroring the SQLite store.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
        Ok(Self { pool })
    }

    pub async fn init(&self) -> Result<(), StoreError> {
        for ddl in accounts_tables_ddl("BIGINT", "BOOLEAN") {
            sqlx::query(sqlx::AssertSqlSafe(ddl))
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    pub async fn save(&self, snapshot: &AccountsSnapshot) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;

        // Children first, parents second: the foreign key is enforced.
        sqlx::query("DELETE FROM account_caps")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM accounts")
            .execute(&mut *tx)
            .await?;

        for rec in &snapshot.records {
            sqlx::query("INSERT INTO accounts (id, handle, is_su) VALUES ($1, $2, $3)")
                .bind(rec.id.0 as i64)
                .bind(&rec.handle)
                .bind(rec.is_su)
                .execute(&mut *tx)
                .await?;
            for cap in &rec.caps {
                sqlx::query("INSERT INTO account_caps (account_id, cap_name) VALUES ($1, $2)")
                    .bind(rec.id.0 as i64)
                    .bind(cap)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        sqlx::query(
            "INSERT INTO meta (key, value) VALUES ($1, $2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(NEXT_ID_KEY)
        .bind(snapshot.next_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO meta (key, value) VALUES ($1, $2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(SCHEMA_VERSION_KEY)
        .bind(ACCOUNTS_SCHEMA_VERSION.to_string())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn load(&self) -> Result<AccountsSnapshot, StoreError> {
        check_schema_version(read_meta_pg(&self.pool, SCHEMA_VERSION_KEY).await?)?;

        let account_rows = sqlx::query("SELECT id, handle, is_su FROM accounts ORDER BY id")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|r| {
                (
                    r.get::<i64, _>("id"),
                    r.get::<String, _>("handle"),
                    r.get::<bool, _>("is_su"),
                )
            })
            .collect();
        let cap_rows = sqlx::query(
            "SELECT account_id, cap_name FROM account_caps ORDER BY account_id, cap_name",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|r| {
            (
                r.get::<i64, _>("account_id"),
                r.get::<String, _>("cap_name"),
            )
        })
        .collect();
        let marker = read_meta_pg(&self.pool, NEXT_ID_KEY).await?;

        Ok(assemble_accounts(account_rows, cap_rows, marker))
    }
}

/// The Postgres twin of [`read_meta`]: a `$1`-placeholder read of a `meta` value.
async fn read_meta_pg<T: FromStr>(pool: &PgPool, key: &str) -> Result<Option<T>, StoreError> {
    Ok(sqlx::query("SELECT value FROM meta WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await?
        .map(|r| r.get::<String, _>("value"))
        .and_then(|s| s.parse().ok()))
}

/// The account store as the runtime holds it: one of the backends, chosen at
/// connect time by the URL scheme. Same role as `musce_persistence::WorldStore`
/// for the world; kept as an enum with inherent forwarding because this crate
/// never grew a store trait.
#[derive(Clone)]
pub enum AccountBackend {
    Sqlite(AccountStore),
    Postgres(PostgresAccountStore),
}

impl AccountBackend {
    /// Connect to whichever backend the URL scheme names: `postgres://` or
    /// `postgresql://` to Postgres, anything else to SQLite.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Ok(AccountBackend::Postgres(
                PostgresAccountStore::connect(url).await?,
            ))
        } else {
            Ok(AccountBackend::Sqlite(AccountStore::connect(url).await?))
        }
    }

    pub async fn init(&self) -> Result<(), StoreError> {
        match self {
            AccountBackend::Sqlite(s) => s.init().await,
            AccountBackend::Postgres(p) => p.init().await,
        }
    }

    pub async fn load(&self) -> Result<AccountsSnapshot, StoreError> {
        match self {
            AccountBackend::Sqlite(s) => s.load().await,
            AccountBackend::Postgres(p) => p.load().await,
        }
    }

    pub async fn save(&self, snapshot: &AccountsSnapshot) -> Result<(), StoreError> {
        match self {
            AccountBackend::Sqlite(s) => s.save(snapshot).await,
            AccountBackend::Postgres(p) => p.save(snapshot).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The initialized store under test. `MUSCE_TEST_DB` unset → in-memory SQLite
    /// (the local default); set → that URL's backend, each Postgres test in its
    /// own schema for parallel isolation.
    async fn backend() -> AccountBackend {
        let b = match std::env::var("MUSCE_TEST_DB") {
            Ok(base) => {
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let schema = format!("musce_acct_test_{}", NEXT.fetch_add(1, Ordering::Relaxed));
                let admin = PostgresAccountStore::connect(&base).await.unwrap();
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "CREATE SCHEMA IF NOT EXISTS {schema}"
                )))
                .execute(&admin.pool)
                .await
                .unwrap();
                let sep = if base.contains('?') { '&' } else { '?' };
                let url = format!("{base}{sep}options=-c%20search_path%3D{schema}");
                AccountBackend::Postgres(PostgresAccountStore::connect(&url).await.unwrap())
            }
            Err(_) => AccountBackend::connect("sqlite::memory:").await.unwrap(),
        };
        b.init().await.unwrap();
        b
    }

    /// A SQLite store for the white-box tests that reach the raw pool to seed a
    /// corrupt state; that manipulation is dialect-specific, so it stays SQLite.
    async fn sqlite_store() -> AccountStore {
        let s = AccountStore::connect("sqlite::memory:").await.unwrap();
        s.init().await.unwrap();
        s
    }

    fn record(id: u64, handle: &str, caps: &[&str], is_su: bool) -> AccountRecord {
        AccountRecord {
            id: AccountId(id),
            handle: handle.into(),
            caps: caps.iter().map(|c| c.to_string()).collect(),
            is_su,
        }
    }

    // `assemble_accounts` and `check_schema_version` are the backend-free core of
    // `load`; testing them here covers cap attachment, the next_id floor, and the
    // schema refusal for both backends with no database.

    #[test]
    fn assemble_attaches_caps_to_their_account() {
        let snap = assemble_accounts(
            vec![(1, "operator".into(), true), (2, "builder".into(), false)],
            vec![(2, "build".into()), (2, "possess".into())],
            Some(3),
        );
        assert_eq!(
            snap.records,
            vec![
                record(1, "operator", &[], true),
                record(2, "builder", &["build", "possess"], false),
            ]
        );
        assert_eq!(snap.next_id, 3);
    }

    #[test]
    fn assemble_next_id_clears_the_live_max() {
        // A marker at or below the live max would reissue an id; the floor wins.
        let snap = assemble_accounts(vec![(5, "a".into(), false)], vec![], Some(2));
        assert_eq!(snap.next_id, 6);
    }

    #[test]
    fn assemble_next_id_honors_a_marker_above_the_floor() {
        // The marker outlives the accounts that minted it (a shrunk set); it wins.
        let snap = assemble_accounts(vec![(1, "a".into(), false)], vec![], Some(9));
        assert_eq!(snap.next_id, 9);
    }

    #[test]
    fn schema_version_check_passes_current_and_missing() {
        assert!(check_schema_version(Some(ACCOUNTS_SCHEMA_VERSION)).is_ok());
        assert!(check_schema_version(None).is_ok());
    }

    #[test]
    fn schema_version_check_refuses_an_unknown_version() {
        match check_schema_version(Some(999)) {
            Err(StoreError::SchemaVersion { stored: 999, .. }) => {}
            other => panic!("expected a schema-version refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_fresh_store_loads_as_the_empty_snapshot() {
        let s = backend().await;
        assert_eq!(s.load().await.unwrap(), AccountsSnapshot::empty());
    }

    #[tokio::test]
    async fn a_snapshot_round_trips_with_caps_and_next_id() {
        let s = backend().await;
        let snap = AccountsSnapshot {
            records: vec![
                AccountRecord {
                    id: AccountId(1),
                    handle: "operator".into(),
                    caps: vec![],
                    is_su: true,
                },
                AccountRecord {
                    id: AccountId(2),
                    handle: "builder".into(),
                    caps: vec!["build".into(), "possess".into()],
                    is_su: false,
                },
            ],
            next_id: 3,
        };
        s.save(&snap).await.unwrap();
        // Caps load in a stable (alphabetical) order; this snapshot is built in it.
        assert_eq!(s.load().await.unwrap(), snap);
    }

    #[tokio::test]
    async fn a_save_replaces_the_whole_set() {
        let s = backend().await;
        let mut snap = AccountsSnapshot {
            records: vec![
                AccountRecord {
                    id: AccountId(1),
                    handle: "operator".into(),
                    caps: vec![],
                    is_su: true,
                },
                AccountRecord {
                    id: AccountId(2),
                    handle: "builder".into(),
                    caps: vec!["build".into()],
                    is_su: false,
                },
            ],
            next_id: 3,
        };
        s.save(&snap).await.unwrap();

        // The next save carries a revoke (and, one day, a delete): the stored set
        // must become exactly the new snapshot, leaving no stale rows behind.
        snap.records[1].caps.clear();
        s.save(&snap).await.unwrap();
        assert_eq!(s.load().await.unwrap(), snap);
    }

    #[tokio::test]
    async fn the_persisted_next_id_survives_a_shrinking_set() {
        let s = backend().await;
        // The high-water mark outlives the records that minted it: a store whose
        // accounts were removed must not reissue their ids.
        let snap = AccountsSnapshot {
            records: vec![AccountRecord {
                id: AccountId(1),
                handle: "operator".into(),
                caps: vec![],
                is_su: true,
            }],
            next_id: 9,
        };
        s.save(&snap).await.unwrap();
        assert_eq!(s.load().await.unwrap().next_id, 9);
    }

    #[tokio::test]
    async fn an_unknown_schema_version_refuses_to_load() {
        let s = sqlite_store().await;
        sqlx::query("INSERT INTO meta (key, value) VALUES ('schema_version', '999')")
            .execute(&s.pool)
            .await
            .unwrap();
        match s.load().await {
            Err(StoreError::SchemaVersion { stored: 999, .. }) => {}
            other => panic!("expected a schema-version refusal, got {other:?}"),
        }
    }
}
