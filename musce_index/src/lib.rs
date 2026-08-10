//! `musce_index`: a generic, type-agnostic secondary index over a single
//! component. An app names a component and a key function; the index maintains a
//! `key -> entities` lookup so it can answer "which entities key to X" without
//! scanning the world every time. The default is a plain value hash; a custom key
//! function makes anything else (a spatial cell hash is the motivating case) fall
//! out for free, because the index never learns the key's meaning.
//!
//! The index is derived, in-memory state, never persisted. It is homed in a
//! [`World`] resource (transient, snapshot-excluded), rebuilt from the world at
//! boot and maintained incrementally thereafter by reacting to the engine's
//! `Fact::ComponentChanged` trigger (per `track_component`) plus `Fact::Destroyed`
//! for eviction. Nothing about it touches the database. See
//! `docs/architecture/indexes.md`.
//!
//! The crate is C-agnostic: at registration the source component type `C` is
//! erased into two closures (a per-entity key reader and a full-world enumerator),
//! leaving the key type `K` as the only generic parameter an [`Index`] carries.

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::hash::Hash;

use musce_core::{EntityId, Fact, Id, NamedComponent, World};

/// The number of entities an index expects per key. This is diagnostic metadata,
/// not write enforcement: the index is derived state and cannot intercept the
/// mutation that introduced a duplicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cardinality {
    Many,
    Unique,
}

type ReadKey<K> = Box<dyn Fn(&World, EntityId) -> Option<K> + Send + Sync>;
type Enumerate = Box<dyn Fn(&World) -> Vec<EntityId> + Send + Sync>;
#[derive(Clone, Copy)]
struct SourceHooks {
    type_id: TypeId,
    type_name: &'static str,
    is_registered: fn(&World) -> bool,
    track: fn(&mut World),
}

#[derive(Debug, PartialEq, Eq)]
pub enum IndexLookupError {
    UnknownName {
        name: String,
    },
    WrongKeyType {
        name: String,
        requested_type: &'static str,
        registered_type: &'static str,
    },
}

impl std::fmt::Display for IndexLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownName { name } => write!(f, "unknown index {name:?}"),
            Self::WrongKeyType {
                name,
                requested_type,
                registered_type,
            } => write!(
                f,
                "index {name:?} uses key type {registered_type}, not requested type {requested_type}"
            ),
        }
    }
}

impl std::error::Error for IndexLookupError {}

#[derive(Debug, PartialEq, Eq)]
pub enum IndexBuildError {
    UnregisteredSource { tag: &'static str },
}

impl std::fmt::Display for IndexBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnregisteredSource { tag } => {
                write!(
                    f,
                    "index source component {tag:?} is not registered in the world"
                )
            }
        }
    }
}

impl std::error::Error for IndexBuildError {}

/// One secondary index, generic over its key type `K`. The source component type
/// is erased into `read_key` and `enumerate`, so this type never names it.
pub struct Index<K> {
    cardinality: Cardinality,
    source_tag: &'static str,
    read_key: ReadKey<K>,
    enumerate: Enumerate,
    forward: HashMap<K, Vec<EntityId>>,
    reverse: HashMap<EntityId, K>,
}

impl<K: Eq + Hash + Clone> Index<K> {
    /// The entities currently keyed to `key`, in insertion order. Empty slice if
    /// none. This is the exact-match primitive; range or neighborhood queries are
    /// the caller's job, built by mapping a region onto the keys that cover it and
    /// unioning their `get`s (a sphere over a spatial cell hash, for example).
    pub fn get(&self, key: &K) -> &[EntityId] {
        self.forward.get(key).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The key an entity currently indexes under, if it is in this index.
    pub fn key_of(&self, entity: EntityId) -> Option<&K> {
        self.reverse.get(&entity)
    }

    pub fn cardinality(&self) -> Cardinality {
        self.cardinality
    }

    /// Every bucket that violates a [`Cardinality::Unique`] declaration, including
    /// both the colliding key and its entities. Borrowed and allocation-free; a
    /// many-valued index yields no conflicts because shared keys are intentional.
    pub fn conflicts(&self) -> impl Iterator<Item = (&K, &[EntityId])> {
        let unique = self.cardinality == Cardinality::Unique;
        self.forward
            .iter()
            .filter(move |(_, entities)| unique && entities.len() > 1)
            .map(|(key, entities)| (key, entities.as_slice()))
    }

    /// The tag of the component this index reads.
    pub fn source_tag(&self) -> &'static str {
        self.source_tag
    }

    /// Reconcile one entity against the current world: reread its key and move it
    /// between buckets if it changed. A missing component (removed, or the entity
    /// despawned) reads as `None` and drops the entity. Idempotent, so a duplicate
    /// trigger is harmless.
    fn place(&mut self, world: &World, entity: EntityId) {
        let new = (self.read_key)(world, entity);
        let old = self.reverse.get(&entity).cloned();
        if old == new {
            return;
        }
        if let Some(old_key) = old {
            self.detach(&old_key, entity);
        }
        if let Some(new_key) = new {
            self.forward
                .entry(new_key.clone())
                .or_default()
                .push(entity);
            self.reverse.insert(entity, new_key);
        }
    }

    /// Drop an entity with no reread, recovering its key from the reverse map. The
    /// eviction path for a despawn, which emits only `Destroyed` (no per-component
    /// remove), so this is the sole signal that a gone entity must leave the index.
    fn evict(&mut self, entity: EntityId) {
        if let Some(old_key) = self.reverse.remove(&entity) {
            self.detach_bucket(&old_key, entity);
        }
    }

    fn detach(&mut self, key: &K, entity: EntityId) {
        self.detach_bucket(key, entity);
        self.reverse.remove(&entity);
    }

    fn detach_bucket(&mut self, key: &K, entity: EntityId) {
        if let Some(bucket) = self.forward.get_mut(key) {
            bucket.retain(|e| *e != entity);
            if bucket.is_empty() {
                self.forward.remove(key);
            }
        }
    }

    fn rebuild_all(&mut self, world: &World) {
        self.forward.clear();
        self.reverse.clear();
        for entity in (self.enumerate)(world) {
            self.place(world, entity);
        }
    }
}

/// Object-safe, key-erased view of an [`Index`], so the registry can hold many
/// indexes of different key types together and drive them uniformly.
trait AnyIndex: Any + Send + Sync {
    fn on_changed(&mut self, world: &World, entity: EntityId);
    fn on_removed(&mut self, entity: EntityId);
    fn rebuild(&mut self, world: &World);
    fn as_any(&self) -> &dyn Any;
    fn key_type_name(&self) -> &'static str;
}

impl<K: Eq + Hash + Clone + Send + Sync + 'static> AnyIndex for Index<K> {
    fn on_changed(&mut self, world: &World, entity: EntityId) {
        self.place(world, entity);
    }
    fn on_removed(&mut self, entity: EntityId) {
        self.evict(entity);
    }
    fn rebuild(&mut self, world: &World) {
        self.rebuild_all(world);
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn key_type_name(&self) -> &'static str {
        type_name::<K>()
    }
}

/// The set of named indexes, homed in a [`World`] resource. Registration is by
/// index name (unique); a component-tag -> names table fans one `ComponentChanged`
/// out to every index over that component, which is what lets many indexes read
/// one component with different keys at no extra cost. Registration is frozen by
/// the first successful baseline, and one source tag may denote only one Rust type.
#[derive(Default)]
pub struct IndexRegistry {
    by_name: HashMap<&'static str, Box<dyn AnyIndex>>,
    by_tag: HashMap<&'static str, Vec<&'static str>>,
    source_hooks: HashMap<&'static str, SourceHooks>,
    activated: bool,
}

impl IndexRegistry {
    /// Add a many-valued index named `name` over component `C`, keyed by `key`.
    pub fn register_many<C, K>(
        &mut self,
        name: &'static str,
        key: impl Fn(&C) -> K + Send + Sync + 'static,
    ) where
        C: NamedComponent,
        K: Eq + Hash + Clone + Send + Sync + 'static,
    {
        self.register_with(name, Cardinality::Many, key);
    }

    /// Add an index whose keys are expected to identify at most one entity.
    /// Duplicates remain present in the bucket and are reported by
    /// [`Index::conflicts`]; enforcement belongs at the app's write boundary.
    pub fn register_unique<C, K>(
        &mut self,
        name: &'static str,
        key: impl Fn(&C) -> K + Send + Sync + 'static,
    ) where
        C: NamedComponent,
        K: Eq + Hash + Clone + Send + Sync + 'static,
    {
        self.register_with(name, Cardinality::Unique, key);
    }

    /// Shared registration path. Activation via [`IndexRegistry::baseline`] tracks
    /// `C` before scanning: the scan absorbs all earlier writes and tracking covers
    /// every later one, so index intent and its trigger prerequisite cannot drift.
    /// Registration rejects a source tag already bound to another Rust type and is
    /// unavailable after activation.
    fn register_with<C, K>(
        &mut self,
        name: &'static str,
        cardinality: Cardinality,
        key: impl Fn(&C) -> K + Send + Sync + 'static,
    ) where
        C: NamedComponent,
        K: Eq + Hash + Clone + Send + Sync + 'static,
    {
        assert!(
            !self.activated,
            "cannot register index {name:?} after index activation"
        );
        assert!(
            !self.by_name.contains_key(name),
            "duplicate index name {name:?}"
        );
        if let Some(existing) = self.source_hooks.get(C::TAG) {
            assert_eq!(
                existing.type_id,
                TypeId::of::<C>(),
                "index source tag {:?} is already bound to {}, not {}",
                C::TAG,
                existing.type_name,
                type_name::<C>()
            );
        }
        let read_key: ReadKey<K> =
            Box::new(move |world, entity| world.get::<C>(entity).map(|c| key(&c)));
        let enumerate: Enumerate = Box::new(|world| {
            world
                .query::<(&Id, &C)>()
                .iter()
                .map(|(id, _)| id.0)
                .collect()
        });
        let index = Index {
            cardinality,
            source_tag: C::TAG,
            read_key,
            enumerate,
            forward: HashMap::new(),
            reverse: HashMap::new(),
        };
        self.by_tag.entry(C::TAG).or_default().push(name);
        self.source_hooks.entry(C::TAG).or_insert(SourceHooks {
            type_id: TypeId::of::<C>(),
            type_name: type_name::<C>(),
            is_registered: is_source_registered::<C>,
            track: track_source::<C>,
        });
        self.by_name.insert(name, Box::new(index));
    }

    /// Borrow a named index at its concrete key type, retaining enough context to
    /// distinguish missing startup wiring from a caller asking for the wrong key.
    pub fn index<K: 'static>(&self, name: &str) -> Result<&Index<K>, IndexLookupError> {
        let idx = self
            .by_name
            .get(name)
            .ok_or_else(|| IndexLookupError::UnknownName {
                name: name.to_owned(),
            })?;
        idx.as_any()
            .downcast_ref::<Index<K>>()
            .ok_or_else(|| IndexLookupError::WrongKeyType {
                name: name.to_owned(),
                requested_type: type_name::<K>(),
                registered_type: idx.key_type_name(),
            })
    }

    /// Activate each source component's change stream, then rebuild every index
    /// from a full scan. Run once at boot after the world is materialized. The scan
    /// establishes the current baseline; trigger tracking keeps later writes live.
    pub fn baseline(&mut self, world: &mut World) -> Result<(), IndexBuildError> {
        for (&tag, hooks) in &self.source_hooks {
            if !(hooks.is_registered)(world) {
                return Err(IndexBuildError::UnregisteredSource { tag });
            }
        }
        for hooks in self.source_hooks.values() {
            (hooks.track)(world);
        }
        for idx in self.by_name.values_mut() {
            idx.rebuild(world);
        }
        self.activated = true;
        Ok(())
    }

    /// Apply a tick's fact batch: fan each `ComponentChanged` to every index over
    /// that component, and evict on every `Destroyed`. Other facts are ignored.
    /// Order within a batch is irrelevant: reread reconciles a change against the
    /// live world, and eviction against the reverse map, so a `ComponentChanged`
    /// and a `Destroyed` for one entity converge either way.
    pub fn apply(&mut self, world: &World, facts: &[Fact]) {
        for fact in facts {
            match fact {
                Fact::ComponentChanged { entity, tag } => {
                    if let Some(names) = self.by_tag.get(tag) {
                        for &name in names {
                            if let Some(idx) = self.by_name.get_mut(name) {
                                idx.on_changed(world, *entity);
                            }
                        }
                    }
                }
                Fact::Destroyed { entity, .. } => {
                    for idx in self.by_name.values_mut() {
                        idx.on_removed(*entity);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Drive the index singleton for one tick. On the first call this run it builds
/// the registry via `init` and does the baseline scan, homing it in a `World`
/// resource; every later call applies the tick's `facts` incrementally. An app's
/// maintainer system is a one-liner over this, registered first among its systems
/// so later systems in the same tick read the updated index.
///
/// The registry is taken out of the resource for the apply, so it owns itself
/// while it rereads component values through `&World`, then reinserted.
pub fn maintain(world: &mut World, facts: &[Fact], init: impl FnOnce(&mut IndexRegistry)) {
    match world.take_resource::<IndexRegistry>() {
        Some(mut registry) => {
            registry.apply(world, facts);
            world.insert_resource(registry);
        }
        None => {
            let mut registry = IndexRegistry::default();
            init(&mut registry);
            registry
                .baseline(world)
                .expect("registered indexes must name registered source components");
            world.insert_resource(registry);
        }
    }
}

fn is_source_registered<C: NamedComponent>(world: &World) -> bool {
    world.is_component_registered::<C>()
}

fn track_source<C: NamedComponent>(world: &mut World) {
    world.track_component::<C>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::DestroyCause;
    use musce_core::hecs::EntityBuilder;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    struct Cell(i64);

    impl NamedComponent for Cell {
        const TAG: &'static str = "cell";
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    struct Level(i64);

    impl NamedComponent for Level {
        const TAG: &'static str = "level";
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    struct AliasCell(i64);

    impl NamedComponent for AliasCell {
        const TAG: &'static str = Cell::TAG;
    }

    fn test_world() -> World {
        let mut world = World::new();
        world.register_component::<Cell>();
        world.register_component::<Level>();
        world
    }

    fn spawn_cell(world: &mut World, c: i64) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Cell(c));
        world.spawn(b)
    }

    fn changed(entity: EntityId, tag: &'static str) -> Fact {
        Fact::ComponentChanged { entity, tag }
    }

    fn destroyed(entity: EntityId) -> Fact {
        Fact::Destroyed {
            entity,
            last_locus: None,
            name: None,
            cause: DestroyCause::Direct,
        }
    }

    fn cell_index() -> IndexRegistry {
        let mut reg = IndexRegistry::default();
        reg.register_many::<Cell, i64>("cell", |c| c.0);
        reg
    }

    #[test]
    fn baseline_indexes_existing_entities() {
        let mut world = test_world();
        let a = spawn_cell(&mut world, 1);
        let b = spawn_cell(&mut world, 1);
        let c = spawn_cell(&mut world, 2);

        let mut reg = cell_index();
        reg.baseline(&mut world).unwrap();

        let idx = reg.index::<i64>("cell").unwrap();
        assert_eq!(idx.get(&1), &[a, b]);
        assert_eq!(idx.get(&2), &[c]);
        assert_eq!(idx.get(&3), &[] as &[EntityId]);
    }

    #[test]
    fn lookup_distinguishes_unknown_name_from_wrong_key_type() {
        let reg = cell_index();
        assert_eq!(
            reg.index::<i64>("missing").err().unwrap(),
            IndexLookupError::UnknownName {
                name: "missing".into()
            }
        );
        assert_eq!(
            reg.index::<String>("cell").err().unwrap(),
            IndexLookupError::WrongKeyType {
                name: "cell".into(),
                requested_type: type_name::<String>(),
                registered_type: type_name::<i64>(),
            }
        );
    }

    #[test]
    fn baseline_rejects_an_unregistered_source_by_tag() {
        let mut world = World::new();
        let mut reg = cell_index();
        assert_eq!(
            reg.baseline(&mut world),
            Err(IndexBuildError::UnregisteredSource { tag: Cell::TAG })
        );
    }

    #[test]
    #[should_panic(expected = "index source tag \"cell\" is already bound")]
    fn registration_rejects_two_component_types_with_one_tag() {
        let mut reg = cell_index();
        reg.register_many::<AliasCell, i64>("alias_cell", |cell| cell.0);
    }

    #[test]
    #[should_panic(expected = "after index activation")]
    fn registration_rejects_a_new_index_after_baseline() {
        let mut world = test_world();
        let mut reg = cell_index();
        reg.baseline(&mut world).unwrap();

        reg.register_many::<Level, i64>("late_level", |level| level.0);
    }

    #[test]
    fn an_activated_registry_can_repeat_its_baseline() {
        let mut world = test_world();
        let entity = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&mut world).unwrap();
        world.insert(entity, Cell(2)).unwrap();

        reg.baseline(&mut world).unwrap();

        assert_eq!(reg.index::<i64>("cell").unwrap().get(&2), &[entity]);
    }

    #[test]
    fn baseline_automatically_tracks_future_real_mutator_facts() {
        let mut world = test_world();
        let entity = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&mut world).unwrap();
        let _ = world.take_facts();

        world.insert(entity, Cell(2)).unwrap();
        let facts = world.take_facts();
        assert!(matches!(
            facts.as_slice(),
            [Fact::ComponentChanged { entity: changed, tag: "cell" }] if *changed == entity
        ));
        reg.apply(&world, &facts);
        assert_eq!(reg.index::<i64>("cell").unwrap().get(&2), &[entity]);
    }

    #[test]
    fn changed_moves_between_buckets() {
        let mut world = test_world();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&mut world).unwrap();

        world.insert(a, Cell(2)).unwrap();
        reg.apply(&world, &[changed(a, "cell")]);

        let idx = reg.index::<i64>("cell").unwrap();
        assert_eq!(idx.get(&1), &[] as &[EntityId]);
        assert_eq!(idx.get(&2), &[a]);
        assert_eq!(idx.key_of(a), Some(&2));
    }

    #[test]
    fn duplicate_triggers_are_idempotent() {
        let mut world = test_world();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&mut world).unwrap();

        world.insert(a, Cell(2)).unwrap();
        reg.apply(&world, &[changed(a, "cell"), changed(a, "cell")]);

        let idx = reg.index::<i64>("cell").unwrap();
        assert_eq!(idx.get(&2), &[a]);
        assert_eq!(idx.get(&1), &[] as &[EntityId]);
    }

    #[test]
    fn removed_component_drops_entity() {
        let mut world = test_world();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&mut world).unwrap();

        world.remove::<Cell>(a).unwrap();
        reg.apply(&world, &[changed(a, "cell")]);

        assert_eq!(
            reg.index::<i64>("cell").unwrap().get(&1),
            &[] as &[EntityId]
        );
    }

    #[test]
    fn destroyed_evicts_from_index() {
        let mut world = test_world();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&mut world).unwrap();

        reg.apply(&world, &[destroyed(a)]);

        assert_eq!(
            reg.index::<i64>("cell").unwrap().get(&1),
            &[] as &[EntityId]
        );
    }

    #[test]
    fn change_then_destroy_same_batch_converges() {
        let mut world = test_world();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&mut world).unwrap();

        // The change fact precedes the despawn that produced the destroy fact.
        world.despawn(a);
        reg.apply(&world, &[changed(a, "cell"), destroyed(a)]);

        assert_eq!(
            reg.index::<i64>("cell").unwrap().get(&1),
            &[] as &[EntityId]
        );
        assert_eq!(reg.index::<i64>("cell").unwrap().key_of(a), None);
    }

    #[test]
    fn destroy_then_change_same_batch_converges() {
        let mut world = test_world();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&mut world).unwrap();

        world.despawn(a);
        reg.apply(&world, &[destroyed(a), changed(a, "cell")]);

        assert_eq!(
            reg.index::<i64>("cell").unwrap().get(&1),
            &[] as &[EntityId]
        );
        assert_eq!(reg.index::<i64>("cell").unwrap().key_of(a), None);
    }

    #[test]
    fn one_change_fans_out_to_every_index_over_the_component() {
        let mut world = test_world();
        let a = spawn_cell(&mut world, 1);

        let mut reg = IndexRegistry::default();
        // Two indexes over the same component, different keys.
        reg.register_many::<Cell, i64>("cell_exact", |c| c.0);
        reg.register_many::<Cell, i64>("cell_band", |c| c.0 / 10);
        reg.baseline(&mut world).unwrap();

        world.insert(a, Cell(25)).unwrap();
        reg.apply(&world, &[changed(a, "cell")]);

        assert_eq!(reg.index::<i64>("cell_exact").unwrap().get(&25), &[a]);
        assert_eq!(reg.index::<i64>("cell_band").unwrap().get(&2), &[a]);
    }

    #[test]
    fn indexes_over_distinct_components_do_not_cross_react() {
        let mut world = test_world();
        let mut b = EntityBuilder::new();
        b.add(Cell(1));
        b.add(Level(7));
        let a = world.spawn(b);

        let mut reg = IndexRegistry::default();
        reg.register_many::<Cell, i64>("cell", |c| c.0);
        reg.register_many::<Level, i64>("level", |l| l.0);
        reg.baseline(&mut world).unwrap();

        // A "cell" change must not disturb the "level" index.
        world.insert(a, Cell(9)).unwrap();
        reg.apply(&world, &[changed(a, "cell")]);

        assert_eq!(reg.index::<i64>("cell").unwrap().get(&9), &[a]);
        assert_eq!(reg.index::<i64>("level").unwrap().get(&7), &[a]);
    }

    #[test]
    fn unique_conflicts_borrow_the_key_and_complete_bucket() {
        let mut world = test_world();
        let a = spawn_cell(&mut world, 1);
        let b = spawn_cell(&mut world, 1);
        spawn_cell(&mut world, 2);

        let mut reg = IndexRegistry::default();
        reg.register_unique::<Cell, i64>("cell", |c| c.0);
        reg.baseline(&mut world).unwrap();

        let idx = reg.index::<i64>("cell").unwrap();
        assert_eq!(idx.cardinality(), Cardinality::Unique);
        let conflicts: Vec<_> = idx.conflicts().collect();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(*conflicts[0].0, 1);
        assert_eq!(conflicts[0].1, &[a, b]);
    }

    #[test]
    fn many_valued_shared_buckets_are_not_conflicts() {
        let mut world = test_world();
        spawn_cell(&mut world, 1);
        spawn_cell(&mut world, 1);

        let mut reg = cell_index();
        reg.baseline(&mut world).unwrap();

        let idx = reg.index::<i64>("cell").unwrap();
        assert_eq!(idx.cardinality(), Cardinality::Many);
        assert_eq!(idx.get(&1).len(), 2);
        assert_eq!(idx.conflicts().count(), 0);
    }

    #[test]
    fn maintain_bootstraps_then_applies_via_resource() {
        let mut world = test_world();
        let a = spawn_cell(&mut world, 5);

        let init = |reg: &mut IndexRegistry| {
            reg.register_many::<Cell, i64>("cell", |c| c.0);
        };

        // First call: builds the registry, baselines, homes it in the resource.
        maintain(&mut world, &[], init);
        assert!(world.resource::<IndexRegistry>().is_some());

        // A change plus its trigger, applied on the next call.
        world.insert(a, Cell(6)).unwrap();
        maintain(&mut world, &[changed(a, "cell")], init);

        let idx_reg = world.resource::<IndexRegistry>().unwrap();
        let idx = idx_reg.index::<i64>("cell").unwrap();
        assert_eq!(idx.get(&5), &[] as &[EntityId]);
        assert_eq!(idx.get(&6), &[a]);
    }
}
