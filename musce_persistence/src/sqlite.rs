use std::str::FromStr;

use musce_auth::Account;
use musce_core::Snapshot;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::{QueryBuilder, Row};

use crate::{
    AccountStore, Error, KvStore, Loaded, NEXT_ID_KEY, Persistence, Result, SCHEMA_VERSION,
    SCHEMA_VERSION_KEY, accounts_table_ddl, assemble, assemble_account, kv_table_ddl,
    world_tables_ddl,
};

/// The most bound parameters allowed in one statement. SQLite's default
/// `SQLITE_MAX_VARIABLE_NUMBER` is 999 on builds before 3.32; staying under it
/// keeps a batch valid regardless of which library version sqlx was compiled
/// against. Row chunks divide this by their param count (2 for a roster row, 3 for
/// a component row, 1 for a delete id).
const MAX_VARS: usize = 999;

#[derive(Clone)]
pub struct SqliteStore {
    pub(crate) pool: SqlitePool,
}

impl SqliteStore {
    /// Connect (creating the file if missing). Use `"sqlite::memory:"` for an
    /// in-memory database. A single connection keeps the writer serialized and
    /// keeps in-memory databases consistent across queries. `foreign_keys` is
    /// enabled here (SQLite defaults it off per connection) so the component ->
    /// entity reference is enforced on every pooled connection.
    pub async fn connect(url: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        Ok(Self { pool })
    }
}

impl Persistence for SqliteStore {
    async fn init(&self) -> Result<()> {
        for ddl in world_tables_ddl("INTEGER") {
            sqlx::query(sqlx::AssertSqlSafe(ddl))
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn save(&self, snapshot: &Snapshot) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // Flatten the snapshot into row sets once, so the writes below are plain
        // batched inserts. The blob is always a `{tag: value}` object (the
        // registry's `serialize_entity` produces one); a non-object is a producer
        // bug, surfaced rather than written as a component-less entity. Component
        // `data` is the JSON text of the value; a marker's `null` becomes the text
        // `"null"` (satisfying NOT NULL), never a bound SQL NULL.
        let mut entity_rows: Vec<(i64, Option<i64>)> = Vec::with_capacity(snapshot.entities.len());
        let mut comp_rows: Vec<(i64, &str, String)> = Vec::new();
        for blob in &snapshot.entities {
            let obj = blob.data.as_object().ok_or(Error::NotAnObject(blob.id))?;
            let id = blob.id.0 as i64;
            entity_rows.push((id, blob.zone.map(|z| z.0 as i64)));
            for (tag, value) in obj {
                comp_rows.push((id, tag.as_str(), serde_json::to_string(value)?));
            }
        }

        // Upsert the roster rows first: a component row's FK requires its entity to
        // exist. Two bound params per row.
        for chunk in entity_rows.chunks(MAX_VARS / 2) {
            let mut qb = QueryBuilder::new("INSERT INTO entities (entity_id, zone) ");
            qb.push_values(chunk, |mut row, (id, zone)| {
                row.push_bind(*id).push_bind(*zone);
            });
            qb.push(" ON CONFLICT(entity_id) DO UPDATE SET zone = excluded.zone");
            qb.build().execute(&mut *tx).await?;
        }

        // Replace each live entity's whole component set: clear its old rows, then
        // insert the current ones. An upsert would leave rows for tags dropped since
        // the last save (e.g. a `RelTarget` removed by `clear_target`), which would
        // resurrect on reload. One bound param per id.
        for chunk in entity_rows.chunks(MAX_VARS) {
            let mut qb = QueryBuilder::new("DELETE FROM components WHERE entity_id IN (");
            let mut sep = qb.separated(", ");
            for (id, _) in chunk {
                sep.push_bind(*id);
            }
            qb.push(")");
            qb.build().execute(&mut *tx).await?;
        }

        // Three bound params per component row.
        for chunk in comp_rows.chunks(MAX_VARS / 3) {
            let mut qb = QueryBuilder::new("INSERT INTO components (entity_id, tag, data) ");
            qb.push_values(chunk, |mut row, (id, tag, data)| {
                row.push_bind(*id).push_bind(*tag).push_bind(data.as_str());
            });
            qb.build().execute(&mut *tx).await?;
        }

        // Despawned entities: drop children before the parent (the FK is RESTRICT,
        // not CASCADE, so correctness never depends on the pragma being on).
        for chunk in snapshot.deletes.chunks(MAX_VARS) {
            let mut qb = QueryBuilder::new("DELETE FROM components WHERE entity_id IN (");
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id.0 as i64);
            }
            qb.push(")");
            qb.build().execute(&mut *tx).await?;

            let mut qb = QueryBuilder::new("DELETE FROM entities WHERE entity_id IN (");
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id.0 as i64);
            }
            qb.push(")");
            qb.build().execute(&mut *tx).await?;
        }

        sqlx::query(
            "INSERT INTO meta (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(NEXT_ID_KEY)
        .bind(snapshot.next_id.to_string())
        .execute(&mut *tx)
        .await?;

        // Stamp the schema version every world is written at, so a later load can
        // tell whether the data needs migrating up to the current vocabulary.
        sqlx::query(
            "INSERT INTO meta (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(SCHEMA_VERSION_KEY)
        .bind(SCHEMA_VERSION.to_string())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn load(&self) -> Result<Loaded> {
        // Extract primitives from the driver, then hand the backend-free
        // `assemble` the grouping, invariant checks, and next_id floor.
        let roster = sqlx::query("SELECT entity_id, zone FROM entities")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|r| {
                (
                    r.get::<i64, _>("entity_id"),
                    r.get::<Option<i64>, _>("zone"),
                )
            })
            .collect();
        let comp_rows = sqlx::query("SELECT entity_id, tag, data FROM components")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|r| {
                (
                    r.get::<i64, _>("entity_id"),
                    r.get::<String, _>("tag"),
                    r.get::<String, _>("data"),
                )
            })
            .collect();
        let max_id: Option<i64> = sqlx::query("SELECT MAX(entity_id) AS m FROM entities")
            .fetch_one(&self.pool)
            .await?
            .get("m");
        let marker = read_meta(&self.pool, NEXT_ID_KEY).await?;
        let schema_version = read_meta(&self.pool, SCHEMA_VERSION_KEY).await?;

        assemble(roster, comp_rows, max_id, marker, schema_version)
    }
}

/// Read a `meta` value and parse it, `None` when the row is missing or does not
/// parse (a restored dump without meta, a hand-edited store). SQLite-side; the
/// Postgres store has its own `$1`-placeholder twin.
async fn read_meta<T: FromStr>(pool: &SqlitePool, key: &str) -> Result<Option<T>> {
    Ok(sqlx::query("SELECT value FROM meta WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?
        .map(|r| r.get::<String, _>("value"))
        .and_then(|s| s.parse().ok()))
}

impl KvStore for SqliteStore {
    async fn kv_init(&self) -> Result<()> {
        sqlx::query(sqlx::AssertSqlSafe(kv_table_ddl("BLOB")))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let row = sqlx::query("SELECT value FROM kv WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get::<Vec<u8>, _>("value")))
    }

    async fn kv_put(&self, key: &str, value: &[u8]) -> Result<()> {
        sqlx::query(
            "INSERT INTO kv (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

impl AccountStore for SqliteStore {
    async fn accounts_init(&self) -> Result<()> {
        sqlx::query(sqlx::AssertSqlSafe(accounts_table_ddl("INTEGER")))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn account_by_username(&self, username: &str) -> Result<Option<Account>> {
        let row = sqlx::query(
            "SELECT id, username, credential, caps, su, status, app_data
             FROM accounts WHERE username = ?",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        row.map(account_from_row).transpose()
    }

    async fn account_upsert(&self, account: &Account) -> Result<()> {
        sqlx::query(
            "INSERT INTO accounts (id, username, credential, caps, su, status, app_data)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 username   = excluded.username,
                 credential = excluded.credential,
                 caps       = excluded.caps,
                 su         = excluded.su,
                 status     = excluded.status,
                 app_data   = excluded.app_data",
        )
        .bind(account.id().to_string())
        .bind(account.username())
        .bind(account.credential())
        .bind(serde_json::to_string(account.caps())?)
        .bind(account.is_su())
        .bind(account.status().as_str())
        .bind(serde_json::to_string(account.app_data())?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn any_superuser(&self) -> Result<bool> {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE su = ?)")
            .bind(true)
            .fetch_one(&self.pool)
            .await?;
        Ok(exists)
    }
}

/// Reassemble one `SELECT`ed row into an [`Account`] through the shared, backend-free
/// [`assemble_account`], so both stores enforce the same parse checks.
fn account_from_row(row: sqlx::sqlite::SqliteRow) -> Result<Account> {
    assemble_account(
        row.get("id"),
        row.get("username"),
        row.get("credential"),
        row.get("caps"),
        row.get("su"),
        row.get("status"),
        row.get("app_data"),
    )
}
