//! Durable storage for the MUSCE world. The DB is save/load only: the in-memory
//! World is authoritative, this is its persisted form. An entity is stored shredded
//! into one row per component, so the same save/load-only data is also amenable to
//! out-of-band analytics (a component census, cross-entity aggregation) run off the
//! runtime tick path, which itself never queries the DB. Cold payloads that should
//! not stay resident live in a separate content store ([`KvStore`]). SQLite now,
//! Postgres to follow with the same shape. See `docs/architecture/persistence.md`.

use std::collections::HashMap;
use std::str::FromStr;

use musce_core::{EntityBlob, EntityId, Id, Map, NamedComponent, Snapshot, Value};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error("malformed component value: {0}")]
    Json(#[from] serde_json::Error),
    #[error("entity {0:?} blob is not a JSON object")]
    NotAnObject(EntityId),
    #[error("entity {0:?} loaded with no Id component; its component rows are missing")]
    IdlessEntity(EntityId),
    #[error("component rows reference entities absent from the roster: {0:?}")]
    OrphanComponents(Vec<EntityId>),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The schema version a freshly written world carries. Bump this whenever a
/// persisted component's tag or value shape changes in a way old rows cannot be
/// read as; the load path compares the stored version against this and runs the
/// migration seam to bring older data up to it. It versions the logical component
/// vocabulary, *not* the physical table layout: a storage-layout change (like the
/// shift to per-component rows) is not expressible through this marker and is
/// handled operationally, not through the seam. No migrations exist yet (the schema
/// has only ever been at version 1); this marker is what makes the first one
/// possible without retrofitting versioning onto already-written worlds. See
/// `docs/architecture/persistence.md`.
pub const SCHEMA_VERSION: u32 = 1;

/// What `load` returns: the persisted entities, the id high-water mark, and the
/// schema version the entities were written at (for the migration seam).
pub struct Loaded {
    pub entities: Vec<EntityBlob>,
    pub next_id: u64,
    pub schema_version: u32,
}

/// Backend-agnostic save/load contract for the world. Implemented per database
/// engine. Cold content is a separate concern; see [`KvStore`].
pub trait Persistence {
    /// Create tables if absent.
    fn init(&self) -> impl std::future::Future<Output = Result<()>> + Send;
    /// Apply a snapshot: upsert live entities, delete despawned, record next_id.
    fn save(&self, snapshot: &Snapshot) -> impl std::future::Future<Output = Result<()>> + Send;
    /// Load the full world.
    fn load(&self) -> impl std::future::Future<Output = Result<Loaded>> + Send;
}

/// A content store for cold data: large, rarely-read payloads (a book's text, a
/// mail archive, PC notes) kept on disk instead of resident in the world. Separate
/// from [`Persistence`] (the whole-world save/load contract) because it is a
/// different concern with a different access pattern, so cold storage can be backed
/// differently (object storage) later without disturbing the world-save contract.
/// Keys are a flat, game-owned namespace, exactly like an object store: the game
/// namespaces by key prefix (`book:<hash>`, `notes:<id>`), and a shared key is how
/// many-to-one dedup falls out (every copy of a book points at one row). Values are
/// engine-opaque bytes the game encodes and decodes. See
/// `docs/architecture/persistence.md`.
pub trait KvStore {
    /// Create the content table if absent.
    fn kv_init(&self) -> impl std::future::Future<Output = Result<()>> + Send;
    /// Fetch a key's bytes, or `None` if the key is absent.
    fn kv_get(
        &self,
        key: &str,
    ) -> impl std::future::Future<Output = Result<Option<Vec<u8>>>> + Send;
    /// Store bytes under a key, overwriting any existing value.
    fn kv_put(
        &self,
        key: &str,
        value: &[u8],
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

const NEXT_ID_KEY: &str = "next_id";
const SCHEMA_VERSION_KEY: &str = "schema_version";

#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
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
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS entities (
                entity_id INTEGER PRIMARY KEY,
                zone      INTEGER
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS components (
                entity_id INTEGER NOT NULL REFERENCES entities(entity_id),
                tag       TEXT    NOT NULL,
                data      TEXT    NOT NULL,
                PRIMARY KEY (entity_id, tag)
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
        let mut tx = self.pool.begin().await?;

        for blob in &snapshot.entities {
            // The blob is always a `{tag: value}` object (the registry's
            // `serialize_entity` produces one); a non-object is a producer bug,
            // surfaced rather than written as a component-less entity.
            let obj = blob.data.as_object().ok_or(Error::NotAnObject(blob.id))?;

            sqlx::query(
                "INSERT INTO entities (entity_id, zone) VALUES (?, ?)
                 ON CONFLICT(entity_id) DO UPDATE SET zone = excluded.zone",
            )
            .bind(blob.id.0 as i64)
            .bind(blob.zone.map(|z| z.0 as i64))
            .execute(&mut *tx)
            .await?;

            // Replace the whole component set: delete then insert. An upsert would
            // leave rows for tags dropped since the last save (e.g. a `RelTarget`
            // removed by `clear_target`), which would resurrect on reload.
            sqlx::query("DELETE FROM components WHERE entity_id = ?")
                .bind(blob.id.0 as i64)
                .execute(&mut *tx)
                .await?;

            for (tag, value) in obj {
                // Store the JSON text of the value; a marker's `null` becomes the
                // text `"null"` (satisfying NOT NULL), never a bound SQL NULL.
                sqlx::query("INSERT INTO components (entity_id, tag, data) VALUES (?, ?, ?)")
                    .bind(blob.id.0 as i64)
                    .bind(tag.as_str())
                    .bind(serde_json::to_string(value)?)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        // Despawned entities: drop children before the parent (the FK is RESTRICT,
        // not CASCADE, so correctness never depends on the pragma being on).
        for id in &snapshot.deletes {
            sqlx::query("DELETE FROM components WHERE entity_id = ?")
                .bind(id.0 as i64)
                .execute(&mut *tx)
                .await?;
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
        let roster = sqlx::query("SELECT entity_id, zone FROM entities")
            .fetch_all(&self.pool)
            .await?;

        // Gather every component row once and group by entity, rebuilding each
        // entity's `{tag: value}` object. Two queries, O(n), order-independent.
        let comp_rows = sqlx::query("SELECT entity_id, tag, data FROM components")
            .fetch_all(&self.pool)
            .await?;
        let mut groups: HashMap<i64, Map<String, Value>> = HashMap::new();
        for row in &comp_rows {
            let id: i64 = row.get("entity_id");
            let tag: String = row.get("tag");
            let data: String = row.get("data");
            groups
                .entry(id)
                .or_default()
                .insert(tag, serde_json::from_str(&data)?);
        }

        let mut entities = Vec::with_capacity(roster.len());
        for row in &roster {
            let id: i64 = row.get("entity_id");
            let zone: Option<i64> = row.get("zone");
            let map = groups.remove(&id).unwrap_or_default();
            // Every entity carries an Id; an empty or Id-less reassembly means the
            // component rows are missing (corrupt or partial write). Surface it
            // rather than load an Id-less entity that only detonates later at the
            // next snapshot. The downstream check is a release-disabled debug_assert.
            if !map.contains_key(Id::TAG) {
                return Err(Error::IdlessEntity(EntityId(id as u64)));
            }
            entities.push(EntityBlob {
                id: EntityId(id as u64),
                zone: zone.map(|z| EntityId(z as u64)),
                data: Value::Object(map),
            });
        }

        // Component rows left ungrouped reference entities with no roster row:
        // orphans the roster-driven assembly would otherwise drop silently.
        if !groups.is_empty() {
            let mut orphans: Vec<EntityId> = groups.keys().map(|&i| EntityId(i as u64)).collect();
            orphans.sort();
            return Err(Error::OrphanComponents(orphans));
        }

        // next_id can never fall to or below a live id, or the next save would
        // reissue it over a stored entity. Enforce max(marker, live_max + 1); a
        // missing or stale-low marker (a restored dump without meta) is corrected
        // and warned, not silently trusted.
        let max_id: Option<i64> = sqlx::query("SELECT MAX(entity_id) AS m FROM entities")
            .fetch_one(&self.pool)
            .await?
            .get("m");
        let floor = max_id.map(|m| m as u64 + 1).unwrap_or(1);
        let marker: u64 = sqlx::query("SELECT value FROM meta WHERE key = ?")
            .bind(NEXT_ID_KEY)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| r.get::<String, _>("value"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        if marker < floor {
            tracing::warn!(
                marker,
                floor,
                "next_id marker below the live max; using the floor to avoid reissuing a live id"
            );
        }
        let next_id = marker.max(floor);

        // A world written before versioning existed has no marker; treat it as the
        // current version, since those are dev-only worlds carrying today's schema.
        // A real older version triggers the migration seam at load.
        let schema_version: u32 = sqlx::query("SELECT value FROM meta WHERE key = ?")
            .bind(SCHEMA_VERSION_KEY)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| r.get::<String, _>("value"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(SCHEMA_VERSION);

        Ok(Loaded {
            entities,
            next_id,
            schema_version,
        })
    }
}

impl KvStore for SqliteStore {
    async fn kv_init(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS kv (
                key   TEXT PRIMARY KEY,
                value BLOB NOT NULL
            )",
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Description, Locus, Name, World};

    #[tokio::test]
    async fn save_load_roundtrip() {
        // Build a world: hall contains bag contains coin.
        let mut w = World::new();
        let mut b = EntityBuilder::new();
        b.add(Locus);
        b.add(Description("hall".into()));
        let hall = w.spawn(b);

        // bag/coin are just described entities: container/item are game kinds and
        // this test exercises the kind-agnostic DB round-trip.
        let mut b = EntityBuilder::new();
        b.add(Description("bag".into()));
        let bag = w.spawn(b);

        let mut b = EntityBuilder::new();
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
        assert_eq!(w2.enclosing_locus(coin), Some(hall));
        assert_eq!(w2.contents(bag), vec![coin]);
        assert!(w2.has::<Locus>(hall));
        assert_eq!(
            w2.entity(bag).unwrap().get::<&Description>().unwrap().0,
            "bag"
        );
        assert_eq!(
            w2.entity(coin).unwrap().get::<&Description>().unwrap().0,
            "coin"
        );
    }

    #[tokio::test]
    async fn removed_component_does_not_survive_reload() {
        // A component dropped between two saves must not resurrect on reload: the
        // save rewrites the whole component set, so the stale row is gone.
        let mut w = World::new();
        let mut b = EntityBuilder::new();
        b.add(Description("a sign".into()));
        b.add(Name("north".into()));
        let sign = w.spawn(b);

        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();
        store.save(&w.snapshot()).await.unwrap();

        // Drop the Name component and save again.
        w.remove::<Name>(sign);
        store.save(&w.snapshot()).await.unwrap();

        let loaded = store.load().await.unwrap();
        let mut w2 = World::new();
        w2.load(&loaded.entities, loaded.next_id).unwrap();

        assert!(
            w2.entity(sign).unwrap().get::<&Name>().is_none(),
            "the removed Name should not survive the reload"
        );
        assert_eq!(
            w2.entity(sign).unwrap().get::<&Description>().unwrap().0,
            "a sign"
        );
    }

    #[tokio::test]
    async fn save_stamps_the_schema_version() {
        let mut w = World::new();
        let mut b = EntityBuilder::new();
        b.add(Description("a thing".into()));
        w.spawn(b);

        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();
        store.save(&w.snapshot()).await.unwrap();

        assert_eq!(store.load().await.unwrap().schema_version, SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn unversioned_world_reads_as_current() {
        // A world written before versioning existed has entity rows but no
        // schema_version marker; it is read as the current version, not migrated.
        let mut w = World::new();
        let mut b = EntityBuilder::new();
        b.add(Description("a thing".into()));
        w.spawn(b);

        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();
        store.save(&w.snapshot()).await.unwrap();
        // Simulate the pre-versioning world: drop the marker.
        sqlx::query("DELETE FROM meta WHERE key = ?")
            .bind(SCHEMA_VERSION_KEY)
            .execute(&store.pool)
            .await
            .unwrap();

        assert_eq!(store.load().await.unwrap().schema_version, SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn next_id_never_reissues_a_live_id() {
        // Entities present but the marker missing (a restored dump without meta):
        // next_id must land above the live max, never reset to 1 and reissue.
        let mut w = World::new();
        for _ in 0..3 {
            let mut b = EntityBuilder::new();
            b.add(Description("x".into()));
            w.spawn(b);
        }

        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();
        let snap = w.snapshot();
        let expected = snap.next_id;
        store.save(&snap).await.unwrap();

        // Drop the marker to simulate a partial restore.
        sqlx::query("DELETE FROM meta WHERE key = ?")
            .bind(NEXT_ID_KEY)
            .execute(&store.pool)
            .await
            .unwrap();

        let next_id = store.load().await.unwrap().next_id;
        assert_eq!(
            next_id, expected,
            "recovered next_id must clear the live max"
        );
        assert!(next_id > 1, "a missing marker must not reset to 1");
    }

    #[tokio::test]
    async fn idless_entity_is_rejected() {
        // A roster row with no component rows reassembles to an Id-less entity.
        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();
        sqlx::query("INSERT INTO entities (entity_id, zone) VALUES (1, NULL)")
            .execute(&store.pool)
            .await
            .unwrap();

        assert!(matches!(store.load().await, Err(Error::IdlessEntity(_))));
    }

    #[tokio::test]
    async fn orphan_component_rows_are_rejected() {
        // A component row whose entity has no roster row. The FK would reject this
        // insert, so disable it for this connection to seed the corrupt state a
        // pragma-off writer could leave; load must surface it, not drop it.
        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO components (entity_id, tag, data) VALUES (99, 'id', '99')")
            .execute(&store.pool)
            .await
            .unwrap();

        assert!(matches!(
            store.load().await,
            Err(Error::OrphanComponents(_))
        ));
    }

    #[tokio::test]
    async fn deletes_are_applied() {
        let mut w = World::new();
        let mut b = EntityBuilder::new();
        b.add(Description("a thing".into()));
        let thing = w.spawn(b);

        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.init().await.unwrap();
        store.save(&w.snapshot()).await.unwrap();
        assert_eq!(store.load().await.unwrap().entities.len(), 1);

        w.despawn(thing);
        store.save(&w.snapshot()).await.unwrap();
        assert_eq!(store.load().await.unwrap().entities.len(), 0);
    }

    #[tokio::test]
    async fn kv_put_get_roundtrip() {
        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.kv_init().await.unwrap();

        assert_eq!(store.kv_get("book:abc").await.unwrap(), None);
        store.kv_put("book:abc", b"once upon a time").await.unwrap();
        assert_eq!(
            store.kv_get("book:abc").await.unwrap().as_deref(),
            Some(&b"once upon a time"[..])
        );
        // Overwrite in place.
        store.kv_put("book:abc", b"a new tale").await.unwrap();
        assert_eq!(
            store.kv_get("book:abc").await.unwrap().as_deref(),
            Some(&b"a new tale"[..])
        );
    }

    #[tokio::test]
    async fn kv_shared_key_and_prefix_namespacing() {
        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        store.kv_init().await.unwrap();

        // Many referents, one row: the shared-key (book-copy) case.
        store.kv_put("book:tome", b"shared text").await.unwrap();
        // Distinct prefixes are distinct keys: no collision across subsystems.
        store.kv_put("notes:1", b"private note").await.unwrap();

        assert_eq!(
            store.kv_get("book:tome").await.unwrap().as_deref(),
            Some(&b"shared text"[..])
        );
        assert_eq!(
            store.kv_get("notes:1").await.unwrap().as_deref(),
            Some(&b"private note"[..])
        );
    }
}
