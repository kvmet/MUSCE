//! Durable storage for the MUSCE world. The DB is save/load only: the in-memory
//! World is authoritative, this is its persisted form. An entity is stored shredded
//! into one row per component, so the same save/load-only data is also amenable to
//! out-of-band analytics (a component census, cross-entity aggregation) run off the
//! runtime tick path, which itself never queries the DB. Cold payloads that should
//! not stay resident live in a separate content store ([`KvStore`]). SQLite and
//! Postgres both back these, one schema behind the traits, picked by the
//! connection URL's scheme. See `docs/architecture/persistence.md`.

use std::collections::HashMap;

use musce_core::{EntityBlob, EntityId, Id, Map, NamedComponent, Snapshot, Value};

mod postgres;
mod sqlite;

pub use postgres::PostgresStore;
pub use sqlite::SqliteStore;

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
#[derive(Debug)]
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

/// The world's table definitions, written once so the two backends cannot drift
/// apart structurally. Only the dialect's type words vary: `int_ty` spells the
/// 64-bit id columns (`INTEGER` on SQLite, `BIGINT` on Postgres). The `data`
/// column is JSON text on both. Returns the entities/components/meta trio the
/// `Persistence::init` contract creates; the cold `kv` table is [`kv_table_ddl`].
fn world_tables_ddl(int_ty: &str) -> [String; 3] {
    [
        format!(
            "CREATE TABLE IF NOT EXISTS entities (
                entity_id {int_ty} PRIMARY KEY,
                zone      {int_ty}
            )"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS components (
                entity_id {int_ty} NOT NULL REFERENCES entities(entity_id),
                tag       TEXT NOT NULL,
                data      TEXT NOT NULL,
                PRIMARY KEY (entity_id, tag)
            )"
        ),
        "CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )"
        .to_string(),
    ]
}

/// The cold content table, kept parallel to [`world_tables_ddl`]. Only the byte
/// column's type word varies: `BLOB` on SQLite, `BYTEA` on Postgres.
fn kv_table_ddl(bytes_ty: &str) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS kv (
            key   TEXT PRIMARY KEY,
            value {bytes_ty} NOT NULL
        )"
    )
}

/// Reassemble the rows a backend read into the world's `Loaded` form, enforcing
/// the load invariants. Pure and backend-free: each store extracts primitives
/// from its own driver, then hands them here, so both inherit exactly the same
/// checks (every entity carries an `Id`, no orphan component rows, `next_id`
/// clears the live max). Unit-tested without a database.
///
/// `marker`/`schema_version` are the parsed `meta` values, `None` when the row is
/// missing or unparseable; `max_id` is the live id high-water from the roster.
fn assemble(
    roster: Vec<(i64, Option<i64>)>,
    comp_rows: Vec<(i64, String, String)>,
    max_id: Option<i64>,
    marker: Option<u64>,
    schema_version: Option<u32>,
) -> Result<Loaded> {
    // Group every component row by entity, rebuilding each `{tag: value}` object.
    let mut groups: HashMap<i64, Map<String, Value>> = HashMap::new();
    for (id, tag, data) in comp_rows {
        groups
            .entry(id)
            .or_default()
            .insert(tag, serde_json::from_str(&data)?);
    }

    let mut entities = Vec::with_capacity(roster.len());
    for (id, zone) in &roster {
        let map = groups.remove(id).unwrap_or_default();
        // Every entity carries an Id; an Id-less reassembly means its component
        // rows are missing (corrupt or partial write). Surface it rather than
        // load an Id-less entity that only detonates at the next snapshot.
        if !map.contains_key(Id::TAG) {
            return Err(Error::IdlessEntity(EntityId(*id as u64)));
        }
        entities.push(EntityBlob {
            id: EntityId(*id as u64),
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
    let floor = max_id.map(|m| m as u64 + 1).unwrap_or(1);
    let marker = marker.unwrap_or(1);
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
    let schema_version = schema_version.unwrap_or(SCHEMA_VERSION);

    Ok(Loaded {
        entities,
        next_id,
        schema_version,
    })
}

/// The world store as the runtime holds it: one of the concrete backends, chosen
/// at connect time by the URL scheme. Forwards the `Persistence`/`KvStore`
/// contract to the variant, so the runtime and a game program against the store
/// without naming a backend. Adding a backend is a variant here plus its impl;
/// no call site changes.
#[derive(Clone)]
pub enum WorldStore {
    Sqlite(SqliteStore),
    Postgres(PostgresStore),
}

impl WorldStore {
    /// Connect to whichever backend the URL scheme names: `postgres://` or
    /// `postgresql://` to Postgres, anything else (`sqlite://…`, `sqlite::memory:`)
    /// to SQLite.
    pub async fn connect(url: &str) -> Result<Self> {
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Ok(WorldStore::Postgres(PostgresStore::connect(url).await?))
        } else {
            Ok(WorldStore::Sqlite(SqliteStore::connect(url).await?))
        }
    }
}

impl Persistence for WorldStore {
    async fn init(&self) -> Result<()> {
        match self {
            WorldStore::Sqlite(s) => s.init().await,
            WorldStore::Postgres(p) => p.init().await,
        }
    }

    async fn save(&self, snapshot: &Snapshot) -> Result<()> {
        match self {
            WorldStore::Sqlite(s) => s.save(snapshot).await,
            WorldStore::Postgres(p) => p.save(snapshot).await,
        }
    }

    async fn load(&self) -> Result<Loaded> {
        match self {
            WorldStore::Sqlite(s) => s.load().await,
            WorldStore::Postgres(p) => p.load().await,
        }
    }
}

impl KvStore for WorldStore {
    async fn kv_init(&self) -> Result<()> {
        match self {
            WorldStore::Sqlite(s) => s.kv_init().await,
            WorldStore::Postgres(p) => p.kv_init().await,
        }
    }

    async fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match self {
            WorldStore::Sqlite(s) => s.kv_get(key).await,
            WorldStore::Postgres(p) => p.kv_get(key).await,
        }
    }

    async fn kv_put(&self, key: &str, value: &[u8]) -> Result<()> {
        match self {
            WorldStore::Sqlite(s) => s.kv_put(key, value).await,
            WorldStore::Postgres(p) => p.kv_put(key, value).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Description, Locus, Name, World};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The store under test. `MUSCE_TEST_DB` unset → a private in-memory SQLite
    /// (the local default); set → that URL's backend, so CI reruns the same
    /// black-box assertions against Postgres. On Postgres each test gets its own
    /// schema for parallel isolation, the equivalent of SQLite's per-connection
    /// `:memory:` database.
    async fn test_world_store() -> WorldStore {
        match std::env::var("MUSCE_TEST_DB") {
            Ok(base) => {
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let schema = format!("musce_test_{}", NEXT.fetch_add(1, Ordering::Relaxed));
                // Create the schema on a plain connection, then pin the store's
                // search_path to it via the connection `options`, so every query
                // this test runs lands in its own schema.
                let admin = PostgresStore::connect(&base).await.unwrap();
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "CREATE SCHEMA IF NOT EXISTS {schema}"
                )))
                .execute(&admin.pool)
                .await
                .unwrap();
                let sep = if base.contains('?') { '&' } else { '?' };
                let url = format!("{base}{sep}options=-c%20search_path%3D{schema}");
                WorldStore::Postgres(PostgresStore::connect(&url).await.unwrap())
            }
            Err(_) => WorldStore::connect("sqlite::memory:").await.unwrap(),
        }
    }

    /// A component row carrying the mandatory `Id` tag, the minimum that lets a
    /// roster entity pass `assemble`'s Id-less check.
    fn id_row(id: i64) -> (i64, String, String) {
        (id, Id::TAG.to_string(), id.to_string())
    }

    // `assemble` is the backend-free heart of every `load`: both stores hand it
    // the same primitives, so testing it here covers the risky invariants (Id-less
    // rejection, orphan detection, the next_id floor) for SQLite and Postgres at
    // once, with no database.

    #[test]
    fn assemble_reassembles_entities_with_zone() {
        let loaded = assemble(
            vec![(1, Some(2))],
            vec![id_row(1)],
            Some(1),
            Some(2),
            Some(1),
        )
        .unwrap();
        assert_eq!(loaded.entities.len(), 1);
        assert_eq!(loaded.entities[0].id, EntityId(1));
        assert_eq!(loaded.entities[0].zone, Some(EntityId(2)));
    }

    #[test]
    fn assemble_rejects_an_idless_entity() {
        // A roster row whose component rows are missing reassembles Id-less.
        let err = assemble(vec![(1, None)], vec![], None, Some(1), Some(1)).unwrap_err();
        assert!(matches!(err, Error::IdlessEntity(EntityId(1))));
    }

    #[test]
    fn assemble_rejects_orphan_components() {
        // A component row whose entity has no roster row is an orphan.
        let err = assemble(vec![], vec![id_row(99)], None, Some(1), Some(1)).unwrap_err();
        assert!(matches!(err, Error::OrphanComponents(ids) if ids == vec![EntityId(99)]));
    }

    #[test]
    fn assemble_next_id_clears_the_live_max() {
        // A marker at or below the live max would reissue a stored id; the floor
        // (live_max + 1) wins.
        let loaded = assemble(vec![(5, None)], vec![id_row(5)], Some(5), Some(3), Some(1)).unwrap();
        assert_eq!(loaded.next_id, 6);
    }

    #[test]
    fn assemble_next_id_honors_a_marker_above_the_floor() {
        // A marker past the live max is authoritative: ids were minted for
        // entities since removed, and must not be reissued.
        let loaded =
            assemble(vec![(5, None)], vec![id_row(5)], Some(5), Some(10), Some(1)).unwrap();
        assert_eq!(loaded.next_id, 10);
    }

    #[test]
    fn assemble_missing_marker_and_version_default() {
        // A restored dump without meta: next_id falls back to the floor, and the
        // absent schema version reads as current (not a spurious migration).
        let loaded = assemble(vec![(2, None)], vec![id_row(2)], Some(2), None, None).unwrap();
        assert_eq!(loaded.next_id, 3);
        assert_eq!(loaded.schema_version, SCHEMA_VERSION);
    }

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

        let store = test_world_store().await;
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

        let store = test_world_store().await;
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

        let store = test_world_store().await;
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

        let store = test_world_store().await;
        store.init().await.unwrap();
        store.save(&w.snapshot()).await.unwrap();
        assert_eq!(store.load().await.unwrap().entities.len(), 1);

        w.despawn(thing);
        store.save(&w.snapshot()).await.unwrap();
        assert_eq!(store.load().await.unwrap().entities.len(), 0);
    }

    #[tokio::test]
    async fn kv_put_get_roundtrip() {
        let store = test_world_store().await;
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
        let store = test_world_store().await;
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
