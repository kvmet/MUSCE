//! `musce_index`: a generic, type-agnostic secondary index over a single
//! component. A game names a component and a key function; the index maintains a
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

use std::any::Any;
use std::collections::HashMap;
use std::hash::Hash;

use musce_core::{EntityId, Fact, Id, NamedComponent, World};

/// Whether an index expects at most one entity per key. `Multi` is the default
/// (many entities may share a key). `Unique` records the intent that a key
/// identifies one entity; it does not enforce it (a rebuilt read model cannot
/// intercept writes), so a violation is detected on request via
/// [`Index::conflicts`], never acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Multi,
    Unique,
}

type ReadKey<K> = Box<dyn Fn(&World, EntityId) -> Option<K> + Send + Sync>;
type Enumerate = Box<dyn Fn(&World) -> Vec<EntityId> + Send + Sync>;

/// One secondary index, generic over its key type `K`. The source component type
/// is erased into `read_key` and `enumerate`, so this type never names it.
pub struct Index<K> {
    policy: Policy,
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

    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// The tag of the component this index reads.
    pub fn source_tag(&self) -> &'static str {
        self.source_tag
    }

    /// Entities that share a key under a `Unique` policy, computed on request (not
    /// maintained). Empty for a `Multi` index, where shared keys are expected.
    pub fn conflicts(&self) -> Vec<EntityId> {
        if self.policy != Policy::Unique {
            return Vec::new();
        }
        self.forward
            .values()
            .filter(|bucket| bucket.len() > 1)
            .flatten()
            .copied()
            .collect()
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
}

/// The set of named indexes, homed in a [`World`] resource. Registration is by
/// index name (unique); a component-tag -> names table fans one `ComponentChanged`
/// out to every index over that component, which is what lets many indexes read
/// one component with different keys at no extra cost.
#[derive(Default)]
pub struct IndexRegistry {
    by_name: HashMap<&'static str, Box<dyn AnyIndex>>,
    by_tag: HashMap<&'static str, Vec<&'static str>>,
}

impl IndexRegistry {
    /// Add an index named `name` over component `C`, keyed by `key`. Panics on a
    /// duplicate name. `C` must be tracked (`world.track_component::<C>()`) for the
    /// index to receive incremental updates; registration here does not track it,
    /// because tracking is startup wiring that must precede any write.
    pub fn register<C, K>(
        &mut self,
        name: &'static str,
        policy: Policy,
        key: impl Fn(&C) -> K + Send + Sync + 'static,
    ) where
        C: NamedComponent,
        K: Eq + Hash + Clone + Send + Sync + 'static,
    {
        assert!(
            !self.by_name.contains_key(name),
            "duplicate index name {name:?}"
        );
        let read_key: ReadKey<K> = Box::new(move |world, entity| {
            world
                .entity(entity)
                .and_then(|er| er.get::<&C>().map(|c| key(&c)))
        });
        let enumerate: Enumerate = Box::new(|world| {
            world
                .ecs()
                .query::<(&Id, &C)>()
                .iter()
                .map(|(id, _)| id.0)
                .collect()
        });
        let index = Index {
            policy,
            source_tag: C::TAG,
            read_key,
            enumerate,
            forward: HashMap::new(),
            reverse: HashMap::new(),
        };
        self.by_tag.entry(C::TAG).or_default().push(name);
        self.by_name.insert(name, Box::new(index));
    }

    /// Borrow a named index at its concrete key type, for querying. `None` if the
    /// name is unknown or `K` does not match the index's key type.
    pub fn index<K: 'static>(&self, name: &str) -> Option<&Index<K>> {
        self.by_name
            .get(name)
            .and_then(|idx| idx.as_any().downcast_ref::<Index<K>>())
    }

    /// Rebuild every index from a full scan of the world. Run once at boot, after
    /// the world is materialized.
    pub fn baseline(&mut self, world: &World) {
        for idx in self.by_name.values_mut() {
            idx.rebuild(world);
        }
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
/// resource; every later call applies the tick's `facts` incrementally. A game's
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
            registry.baseline(world);
            world.insert_resource(registry);
        }
    }
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
        reg.register::<Cell, i64>("cell", Policy::Multi, |c| c.0);
        reg
    }

    #[test]
    fn baseline_indexes_existing_entities() {
        let mut world = World::new();
        let a = spawn_cell(&mut world, 1);
        let b = spawn_cell(&mut world, 1);
        let c = spawn_cell(&mut world, 2);

        let mut reg = cell_index();
        reg.baseline(&world);

        let idx = reg.index::<i64>("cell").unwrap();
        assert_eq!(idx.get(&1), &[a, b]);
        assert_eq!(idx.get(&2), &[c]);
        assert_eq!(idx.get(&3), &[] as &[EntityId]);
    }

    #[test]
    fn changed_moves_between_buckets() {
        let mut world = World::new();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&world);

        world.insert(a, Cell(2));
        reg.apply(&world, &[changed(a, "cell")]);

        let idx = reg.index::<i64>("cell").unwrap();
        assert_eq!(idx.get(&1), &[] as &[EntityId]);
        assert_eq!(idx.get(&2), &[a]);
        assert_eq!(idx.key_of(a), Some(&2));
    }

    #[test]
    fn duplicate_triggers_are_idempotent() {
        let mut world = World::new();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&world);

        world.insert(a, Cell(2));
        reg.apply(&world, &[changed(a, "cell"), changed(a, "cell")]);

        let idx = reg.index::<i64>("cell").unwrap();
        assert_eq!(idx.get(&2), &[a]);
        assert_eq!(idx.get(&1), &[] as &[EntityId]);
    }

    #[test]
    fn removed_component_drops_entity() {
        let mut world = World::new();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&world);

        world.remove::<Cell>(a);
        reg.apply(&world, &[changed(a, "cell")]);

        assert_eq!(
            reg.index::<i64>("cell").unwrap().get(&1),
            &[] as &[EntityId]
        );
    }

    #[test]
    fn destroyed_evicts_from_index() {
        let mut world = World::new();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&world);

        reg.apply(&world, &[destroyed(a)]);

        assert_eq!(
            reg.index::<i64>("cell").unwrap().get(&1),
            &[] as &[EntityId]
        );
    }

    #[test]
    fn change_then_destroy_same_batch_converges() {
        let mut world = World::new();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&world);

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
        let mut world = World::new();
        let a = spawn_cell(&mut world, 1);
        let mut reg = cell_index();
        reg.baseline(&world);

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
        let mut world = World::new();
        let a = spawn_cell(&mut world, 1);

        let mut reg = IndexRegistry::default();
        // Two indexes over the same component, different keys.
        reg.register::<Cell, i64>("cell_exact", Policy::Multi, |c| c.0);
        reg.register::<Cell, i64>("cell_band", Policy::Multi, |c| c.0 / 10);
        reg.baseline(&world);

        world.insert(a, Cell(25));
        reg.apply(&world, &[changed(a, "cell")]);

        assert_eq!(reg.index::<i64>("cell_exact").unwrap().get(&25), &[a]);
        assert_eq!(reg.index::<i64>("cell_band").unwrap().get(&2), &[a]);
    }

    #[test]
    fn indexes_over_distinct_components_do_not_cross_react() {
        let mut world = World::new();
        let mut b = EntityBuilder::new();
        b.add(Cell(1));
        b.add(Level(7));
        let a = world.spawn(b);

        let mut reg = IndexRegistry::default();
        reg.register::<Cell, i64>("cell", Policy::Multi, |c| c.0);
        reg.register::<Level, i64>("level", Policy::Multi, |l| l.0);
        reg.baseline(&world);

        // A "cell" change must not disturb the "level" index.
        world.insert(a, Cell(9));
        reg.apply(&world, &[changed(a, "cell")]);

        assert_eq!(reg.index::<i64>("cell").unwrap().get(&9), &[a]);
        assert_eq!(reg.index::<i64>("level").unwrap().get(&7), &[a]);
    }

    #[test]
    fn unique_reports_conflicts_on_request() {
        let mut world = World::new();
        let a = spawn_cell(&mut world, 1);
        let b = spawn_cell(&mut world, 1);
        let mut reg = IndexRegistry::default();
        reg.register::<Cell, i64>("cell", Policy::Unique, |c| c.0);
        reg.baseline(&world);

        let mut conflicts = reg.index::<i64>("cell").unwrap().conflicts();
        conflicts.sort();
        let mut expected = vec![a, b];
        expected.sort();
        assert_eq!(conflicts, expected);
    }

    #[test]
    fn maintain_bootstraps_then_applies_via_resource() {
        let mut world = World::new();
        let a = spawn_cell(&mut world, 5);

        let init = |reg: &mut IndexRegistry| {
            reg.register::<Cell, i64>("cell", Policy::Multi, |c| c.0);
        };

        // First call: builds the registry, baselines, homes it in the resource.
        maintain(&mut world, &[], init);
        assert!(world.resource::<IndexRegistry>().is_some());

        // A change plus its trigger, applied on the next call.
        world.insert(a, Cell(6));
        maintain(&mut world, &[changed(a, "cell")], init);

        let idx_reg = world.resource::<IndexRegistry>().unwrap();
        let idx = idx_reg.index::<i64>("cell").unwrap();
        assert_eq!(idx.get(&5), &[] as &[EntityId]);
        assert_eq!(idx.get(&6), &[a]);
    }
}
