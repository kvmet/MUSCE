//! The durable account store: a small relational SQLite database of its own,
//! separate from the world DB (which a dev reseed deletes). Accounts are
//! first-class queryable rows, unlike the world's opaque entity blobs, because
//! consumers beyond the sim host (a web or oauth frontend, admin tooling) read the
//! same account set, and the account id is the foreign-key target for provenance
//! and a later characters table. Runtime reads never come here: the sim reads the
//! in-memory authority, and an async writer task feeds full-snapshot saves.

use std::str::FromStr;

use sqlx::Row;
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
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS accounts (
                id     INTEGER PRIMARY KEY,
                handle TEXT UNIQUE NOT NULL,
                is_su  INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS account_caps (
                account_id INTEGER NOT NULL REFERENCES accounts(id),
                cap_name   TEXT NOT NULL,
                PRIMARY KEY (account_id, cap_name)
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;
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
        // A fresh store has no marker and reads as current; a real mismatch means
        // this build cannot interpret the rows, so it refuses rather than guesses.
        let stored: u32 = sqlx::query("SELECT value FROM meta WHERE key = ?")
            .bind(SCHEMA_VERSION_KEY)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| r.get::<String, _>("value"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(ACCOUNTS_SCHEMA_VERSION);
        if stored != ACCOUNTS_SCHEMA_VERSION {
            return Err(StoreError::SchemaVersion {
                stored,
                current: ACCOUNTS_SCHEMA_VERSION,
            });
        }

        let mut records = Vec::new();
        let rows = sqlx::query("SELECT id, handle, is_su FROM accounts ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        for row in rows {
            let id: i64 = row.get("id");
            records.push(AccountRecord {
                id: AccountId(id as u64),
                handle: row.get("handle"),
                caps: Vec::new(),
                is_su: row.get("is_su"),
            });
        }
        let caps = sqlx::query(
            "SELECT account_id, cap_name FROM account_caps ORDER BY account_id, cap_name",
        )
        .fetch_all(&self.pool)
        .await?;
        for row in caps {
            let id = AccountId(row.get::<i64, _>("account_id") as u64);
            if let Some(rec) = records.iter_mut().find(|r| r.id == id) {
                rec.caps.push(row.get("cap_name"));
            }
        }

        let stored_next: u64 = sqlx::query("SELECT value FROM meta WHERE key = ?")
            .bind(NEXT_ID_KEY)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| r.get::<String, _>("value"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        // The persisted high-water mark is authoritative (it survives deletes and
        // must never let an id be reissued); the max-row floor only defends
        // against a hand-edited store whose meta row went missing.
        let floor = records.iter().map(|r| r.id.0 + 1).max().unwrap_or(1);
        let next_id = stored_next.max(floor);

        Ok(AccountsSnapshot { records, next_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> AccountStore {
        let s = AccountStore::connect("sqlite::memory:").await.unwrap();
        s.init().await.unwrap();
        s
    }

    #[tokio::test]
    async fn a_fresh_store_loads_as_the_empty_snapshot() {
        let s = store().await;
        assert_eq!(s.load().await.unwrap(), AccountsSnapshot::empty());
    }

    #[tokio::test]
    async fn a_snapshot_round_trips_with_caps_and_next_id() {
        let s = store().await;
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
        let s = store().await;
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
        let s = store().await;
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
        let s = store().await;
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
