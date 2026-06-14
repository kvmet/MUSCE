//! Durable storage for the MUSCE world. The DB is save/load only: the in-memory
//! World is authoritative, this is its persisted form. One blob per entity plus
//! a small meta table; SQLite now, Postgres to follow with the same shape.

use std::str::FromStr;

use musce_core::{EntityBlob, EntityId, Snapshot};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error("malformed entity blob: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// What `load` returns: the persisted entities and the id high-water mark.
pub struct Loaded {
    pub entities: Vec<EntityBlob>,
    pub next_id: u64,
}

/// Backend-agnostic save/load contract. Implemented per database engine.
pub trait Persistence {
    /// Create tables if absent.
    fn init(&self) -> impl std::future::Future<Output = Result<()>> + Send;
    /// Apply a snapshot: upsert live entities, delete despawned, record next_id.
    fn save(&self, snapshot: &Snapshot) -> impl std::future::Future<Output = Result<()>> + Send;
    /// Load the full world.
    fn load(&self) -> impl std::future::Future<Output = Result<Loaded>> + Send;
}

const NEXT_ID_KEY: &str = "next_id";

pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Connect (creating the file if missing). Use `"sqlite::memory:"` for an
    /// in-memory database. A single connection keeps the writer serialized and
    /// keeps in-memory databases consistent across queries.
    pub async fn connect(url: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::from_str(url)?.create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        Ok(Self { pool })
    }
}

impl Persistence for SqliteStore {
    async fn init(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS entities (
                entity_id  INTEGER PRIMARY KEY,
                zone       INTEGER,
                data       TEXT NOT NULL,
                updated_at INTEGER NOT NULL
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

    async fn save(&self, snapshot: &Snapshot) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let mut tx = self.pool.begin().await?;

        for blob in &snapshot.entities {
            let data = serde_json::to_string(&blob.data)?;
            sqlx::query(
                "INSERT INTO entities (entity_id, zone, data, updated_at)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT(entity_id) DO UPDATE SET
                    zone = excluded.zone,
                    data = excluded.data,
                    updated_at = excluded.updated_at",
            )
            .bind(blob.id.0 as i64)
            .bind(blob.zone.map(|z| z.0 as i64))
            .bind(data)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        for id in &snapshot.deletes {
            sqlx::query("DELETE FROM entities WHERE entity_id = ?")
                .bind(id.0 as i64)
                .execute(&mut *tx)
                .await?;
        }

        sqlx::query(
            "INSERT INTO meta (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(NEXT_ID_KEY)
        .bind(snapshot.next_id.to_string())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn load(&self) -> Result<Loaded> {
        let rows = sqlx::query("SELECT entity_id, zone, data FROM entities")
            .fetch_all(&self.pool)
            .await?;

        let mut entities = Vec::with_capacity(rows.len());
        for row in rows {
            let id: i64 = row.get("entity_id");
            let zone: Option<i64> = row.get("zone");
            let data: String = row.get("data");
            entities.push(EntityBlob {
                id: EntityId(id as u64),
                zone: zone.map(|z| EntityId(z as u64)),
                data: serde_json::from_str(&data)?,
            });
        }

        let next_id: u64 = sqlx::query("SELECT value FROM meta WHERE key = ?")
            .bind(NEXT_ID_KEY)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| r.get::<String, _>("value"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        Ok(Loaded { entities, next_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Container, Description, Item, Room, World};

    #[tokio::test]
    async fn save_load_roundtrip() {
        // Build a world: hall contains bag contains coin.
        let mut w = World::new();
        let mut b = EntityBuilder::new();
        b.add(Room);
        b.add(Description("hall".into()));
        let hall = w.spawn(b);

        let mut b = EntityBuilder::new();
        b.add(Container);
        b.add(Description("bag".into()));
        let bag = w.spawn(b);

        let mut b = EntityBuilder::new();
        b.add(Item);
        b.add(Description("coin".into()));
        let coin = w.spawn(b);

        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();

        let snap = w.snapshot();

        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();
        store.save(&snap).await.unwrap();

        let loaded = store.load().await.unwrap();
        assert_eq!(loaded.next_id, snap.next_id);

        let mut w2 = World::new();
        w2.load(&loaded.entities, loaded.next_id).unwrap();

        // Structure and reverse lists survive the DB round-trip.
        assert_eq!(w2.container_of(coin), Some(bag));
        assert_eq!(w2.container_of(bag), Some(hall));
        assert_eq!(w2.enclosing_room(coin), Some(hall));
        assert_eq!(w2.contents(bag), vec![coin]);
        assert!(w2.has::<Room>(hall));
        assert!(w2.has::<Container>(bag));
        assert!(w2.has::<Item>(coin));
    }

    #[tokio::test]
    async fn deletes_are_applied() {
        let mut w = World::new();
        let mut b = EntityBuilder::new();
        b.add(Item);
        let thing = w.spawn(b);

        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();
        store.save(&w.snapshot()).await.unwrap();
        assert_eq!(store.load().await.unwrap().entities.len(), 1);

        w.despawn(thing);
        store.save(&w.snapshot()).await.unwrap();
        assert_eq!(store.load().await.unwrap().entities.len(), 0);
    }
}
