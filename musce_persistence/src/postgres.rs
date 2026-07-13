use std::str::FromStr;

use musce_core::Snapshot;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::{
    Error, KvStore, Loaded, NEXT_ID_KEY, Persistence, Result, SCHEMA_VERSION, SCHEMA_VERSION_KEY,
    assemble, kv_table_ddl, world_tables_ddl,
};

#[derive(Clone)]
pub struct PostgresStore {
    pub(crate) pool: PgPool,
}

impl PostgresStore {
    /// Connect to an existing Postgres database. Unlike SQLite there is no
    /// create-if-missing: the database must already exist (the deployment or CI
    /// provisions it); `init` only creates tables within it. A single connection
    /// keeps the writer serialized, mirroring the SQLite store.
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
        Ok(Self { pool })
    }
}

impl Persistence for PostgresStore {
    async fn init(&self) -> Result<()> {
        for ddl in world_tables_ddl("BIGINT") {
            sqlx::query(sqlx::AssertSqlSafe(ddl))
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn save(&self, snapshot: &Snapshot) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for blob in &snapshot.entities {
            let obj = blob.data.as_object().ok_or(Error::NotAnObject(blob.id))?;

            sqlx::query(
                "INSERT INTO entities (entity_id, zone) VALUES ($1, $2)
                 ON CONFLICT(entity_id) DO UPDATE SET zone = excluded.zone",
            )
            .bind(blob.id.0 as i64)
            .bind(blob.zone.map(|z| z.0 as i64))
            .execute(&mut *tx)
            .await?;

            // Replace the whole component set: delete then insert, so a tag
            // dropped since the last save does not resurrect on reload.
            sqlx::query("DELETE FROM components WHERE entity_id = $1")
                .bind(blob.id.0 as i64)
                .execute(&mut *tx)
                .await?;

            for (tag, value) in obj {
                sqlx::query("INSERT INTO components (entity_id, tag, data) VALUES ($1, $2, $3)")
                    .bind(blob.id.0 as i64)
                    .bind(tag.as_str())
                    .bind(serde_json::to_string(value)?)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        for id in &snapshot.deletes {
            sqlx::query("DELETE FROM components WHERE entity_id = $1")
                .bind(id.0 as i64)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM entities WHERE entity_id = $1")
                .bind(id.0 as i64)
                .execute(&mut *tx)
                .await?;
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
        .bind(SCHEMA_VERSION.to_string())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn load(&self) -> Result<Loaded> {
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
        let marker = read_meta_pg(&self.pool, NEXT_ID_KEY).await?;
        let schema_version = read_meta_pg(&self.pool, SCHEMA_VERSION_KEY).await?;

        assemble(roster, comp_rows, max_id, marker, schema_version)
    }
}

/// The Postgres twin of `read_meta`: a `$1`-placeholder read of a `meta` value.
async fn read_meta_pg<T: FromStr>(pool: &PgPool, key: &str) -> Result<Option<T>> {
    Ok(sqlx::query("SELECT value FROM meta WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await?
        .map(|r| r.get::<String, _>("value"))
        .and_then(|s| s.parse().ok()))
}

impl KvStore for PostgresStore {
    async fn kv_init(&self) -> Result<()> {
        sqlx::query(sqlx::AssertSqlSafe(kv_table_ddl("BYTEA")))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let row = sqlx::query("SELECT value FROM kv WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get::<Vec<u8>, _>("value")))
    }

    async fn kv_put(&self, key: &str, value: &[u8]) -> Result<()> {
        sqlx::query(
            "INSERT INTO kv (key, value) VALUES ($1, $2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
