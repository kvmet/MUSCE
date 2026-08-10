use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::component::{
    ComponentRegistry, Description, Id, Locus, Name, NamedComponent, RegistryError,
};
use crate::containment::Containment;
use crate::control::{Controls, Focus};
use crate::fact::{DestroyCause, Fact};
use crate::id::{EntityId, EntityIndex};
use crate::relation::{
    AcyclicRelation, Cascade, RelTarget, Relation, RelationError, RelationRole, Walk,
};
use crate::snapshot::LoadError;

type DespawnHandler = fn(&mut World, EntityId);
type ValidateHandler = fn(&World) -> Result<(), LoadError>;
type RebuildHandler = fn(&mut World);
type RelateFn = fn(&mut World, EntityId, EntityId) -> Result<(), RelationError>;
type UnrelateFn = fn(&mut World, EntityId);

mod read_query_sealed {
    pub trait Sealed {}
}

/// The queries [`World::query`] accepts: read-only ones. Implemented for a shared
/// component borrow `&T` and for tuples of read-only queries, and deliberately *not*
/// for `&mut T`. This is the bound that lets `World` expose archetypal iteration
/// without also handing out a write path that bypasses the mutator layer (and so the
/// dirty set, the index, and the reverse lists). An app names the components in a
/// query (`world.query::<(&Id, &Foo)>()`); it never names this trait. The private
/// supertrait seals the implementation set, so an app cannot opt a mutable hecs
/// query back into this surface.
///
/// ```compile_fail
/// use musce_core::world::ReadQuery;
///
/// struct AppComponent;
/// impl ReadQuery for &mut AppComponent {}
/// ```
pub trait ReadQuery: hecs::Query + read_query_sealed::Sealed {}

impl<T: hecs::Component> read_query_sealed::Sealed for &T {}
impl<T: hecs::Component> ReadQuery for &T {}

macro_rules! read_query_tuple {
    ($($name:ident),+) => {
        impl<$($name: ReadQuery),+> read_query_sealed::Sealed for ($($name,)+) {}
        impl<$($name: ReadQuery),+> ReadQuery for ($($name,)+) {}
    };
}
read_query_tuple!(A);
read_query_tuple!(A, B);
read_query_tuple!(A, B, C);
read_query_tuple!(A, B, C, D);
read_query_tuple!(A, B, C, D, E);
read_query_tuple!(A, B, C, D, E, F);

/// Type-erased per-relation cleanup hooks, populated by `register_relation`.
#[derive(Default, Clone)]
struct RelationRegistry {
    registered: HashSet<TypeId>,
    despawn: Vec<DespawnHandler>,
    validate: Vec<ValidateHandler>,
    rebuild: Vec<RebuildHandler>,
    relate: HashMap<&'static str, RelateFn>,
    unrelate: HashMap<&'static str, UnrelateFn>,
}

/// The authoritative in-memory app state: a hecs World plus the identity index
/// and the registries that drive persistence and relation bookkeeping.
pub struct World {
    pub(crate) ecs: hecs::World,
    index: EntityIndex,
    next_id: u64,
    relations: RelationRegistry,
    components: ComponentRegistry,
    /// EntityIds despawned but not yet confirmed durably deleted. A snapshot
    /// copies (does not drain) this; it clears only once persistence acks via
    /// `confirm_saved`, so a failed save can't lose a pending delete.
    despawned: Vec<EntityId>,
    /// Live EntityIds whose persisted state changed since the last snapshot: the
    /// dirty set a delta snapshot serializes instead of walking the whole world.
    /// Marked at every mutator chokepoint that touches a persisted component or
    /// forward relation link; a snapshot *drains* it (unlike `despawned`, which is
    /// copied), because a live entity re-mutated after the snapshot must re-enter
    /// the set for the next one, and a failed save restores the drained ids via
    /// `remark_dirty`. A raw in-crate `&mut` component write (via `entity_ref` or the
    /// `ecs` field) bypasses this, the same boundary `ComponentChanged` and
    /// raw-mutation hygiene guard polices; the public API has no such path, so outside
    /// the core the only way to change a persisted component is through `modify` and the
    /// other mutators. `load` does not mark (a loaded world already matches the store);
    /// only a schema migration re-dirties it, via `mark_all_dirty`.
    dirty: HashSet<EntityId>,
    /// Structural facts emitted since the last drain: a transient per-tick buffer
    /// the runtime drains via `take_facts` before running systems. Not persisted
    /// (a snapshot serializes only registered components); mirrors `despawned`.
    facts: Vec<Fact>,
    /// Component tags a consumer opted into via `track_component`. A
    /// `ComponentChanged` fact fires only for a tag in this set, keeping the trigger
    /// stream bounded to components someone actually maintains an index over.
    tracked: HashSet<&'static str>,
    /// Transient singleton state an app hangs off the world without persisting it:
    /// derived, rebuilt-on-boot data (a secondary index, a cache), keyed by type,
    /// at most one value per type. Like `facts` and `tracked` it lives beside the
    /// entity table and `snapshot` never serializes it, so it costs nothing at save
    /// time and starts empty every boot. The engine never reads a resource; it is
    /// opaque app state, homed here only because a `fn`-pointer system can reach no
    /// state but the world.
    resources: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Derived reverse index per relation: `R -> (target -> its sources)`. The
    /// forward link (`RelTarget<R>`) is the persisted source of truth on the source
    /// entity; this is the rebuilt-on-load reverse of it, maintained inline by the
    /// same mutators that write the forward link so `sources_of` is synchronously
    /// consistent (the despawn cascade reads it mid-tick). Homed here, beside the
    /// other derived indexes (`resources`, `musce_index`) rather than as a component,
    /// because it is only ever point-looked-up by target, never iterated
    /// archetypally: a component would fragment archetypes and force a raw `&mut` to
    /// maintain, for no columnar benefit. Keyed by `TypeId::of::<R>()`; the values
    /// are plain `Vec<EntityId>` regardless of `R`, so no type erasure is needed.
    /// Never persisted; `rebuild_relations` repopulates it from the forward links on
    /// load.
    reverse: HashMap<TypeId, HashMap<EntityId, Vec<EntityId>>>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        let mut w = World {
            ecs: hecs::World::new(),
            index: EntityIndex::default(),
            next_id: 1,
            relations: RelationRegistry::default(),
            components: ComponentRegistry::default(),
            despawned: Vec::new(),
            dirty: HashSet::new(),
            facts: Vec::new(),
            tracked: HashSet::new(),
            resources: HashMap::new(),
            reverse: HashMap::new(),
        };
        w.register_defaults();
        w
    }

    fn register_defaults(&mut self) {
        self.register_component::<Id>();
        self.register_component::<Description>();
        self.register_component::<Name>();
        self.register_component::<Locus>();
        self.register_relation::<Containment>();
        self.register_relation::<Controls>();
        self.register_relation::<Focus>();
    }

    // --- registration ----------------------------------------------------

    pub fn register_component<C: NamedComponent>(&mut self) {
        self.components.register::<C>();
    }

    /// Whether startup wiring registered component type `C` for persistence and
    /// type-erased mutation. Primarily lets derived-state consumers validate their
    /// own activation prerequisites without relying on a later panic.
    pub fn is_component_registered<C: 'static>(&self) -> bool {
        self.components.tag_of::<C>().is_some()
    }

    /// Opt a component into the `ComponentChanged` trigger stream. Until a component
    /// is tracked it emits nothing; this is the bound that keeps the trigger charter
    /// honest (see fact.rs). `C` must be registered, so every mutator path can
    /// resolve its tag. Startup wiring; tracking the same component twice is a
    /// harmless no-op.
    pub fn track_component<C: NamedComponent>(&mut self) {
        assert!(
            self.components.tag_of::<C>().is_some(),
            "cannot track unregistered component {:?}; register it first",
            C::TAG
        );
        self.tracked.insert(C::TAG);
    }

    // --- transient resources --------------------------------------------
    //
    // Type-keyed singleton state that is never persisted: an app's derived,
    // rebuilt-on-boot data (a secondary index, a cache). The engine stores and
    // hands these back but never interprets one; `snapshot` does not see them.

    /// Insert or replace the resource of type `T`, returning the previous value if
    /// one was set.
    pub fn insert_resource<T: Any + Send + Sync>(&mut self, value: T) -> Option<T> {
        self.resources
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|prev| prev.downcast::<T>().ok().map(|b| *b))
    }

    /// Borrow the resource of type `T`, if set.
    pub fn resource<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.resources
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    /// Remove and return the resource of type `T`, if set. The take-out that lets a
    /// maintainer own its state while it reads the rest of the world through
    /// `&World` (an index rereads component values as it applies its deltas), then
    /// reinsert it.
    pub fn take_resource<T: Any + Send + Sync>(&mut self) -> Option<T> {
        self.resources
            .remove(&TypeId::of::<T>())
            .and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    pub fn register_relation<R: Relation>(&mut self) {
        // The forward link is a persisted component; the reverse list is not.
        self.register_component::<RelTarget<R>>();
        // The live mutation paths must refuse forward-link tags; they bypass the
        // cycle check and reverse-index bookkeeping that `relate` owns.
        self.components.mark_relation_tag(R::TARGET_TAG);
        self.relations.registered.insert(TypeId::of::<R>());
        self.relations.despawn.push(despawn_relation::<R>);
        self.relations.validate.push(validate_relation::<R>);
        self.relations.rebuild.push(rebuild_relation::<R>);
        self.relations
            .relate
            .insert(R::TARGET_TAG, relate_by_tag::<R>);
        self.relations
            .unrelate
            .insert(R::TARGET_TAG, unrelate_by_tag::<R>);
    }

    /// Whether startup wiring registered relation type `R`. Lets an assembled
    /// affordance state vocabulary fail closed before evaluating a typed reader
    /// against a world that never registered the corresponding relation.
    pub fn is_relation_registered<R: Relation>(&self) -> bool {
        self.relations.registered.contains(&TypeId::of::<R>())
    }

    // --- identity / lifecycle -------------------------------------------

    fn alloc_id(&mut self) -> EntityId {
        let id = EntityId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    pub fn index(&self) -> &EntityIndex {
        &self.index
    }

    /// Spawn an entity from a builder, assigning it a fresh `EntityId`.
    pub fn spawn(&mut self, mut builder: hecs::EntityBuilder) -> EntityId {
        let id = self.alloc_id();
        builder.add(Id(id));
        let e = self.ecs.spawn(builder.build());
        self.index.insert(id, e);
        self.mark_dirty(id);
        id
    }

    /// Despawn an entity, running every relation's cascade first. A directly
    /// targeted despawn; cascade-removed entities go through `despawn_with_cause`
    /// with `Cascade`.
    pub fn despawn(&mut self, id: EntityId) {
        self.despawn_with_cause(id, DestroyCause::Direct);
    }

    /// The despawn body, tagged with why this entity is dying. `cause` rides into
    /// the `Destroyed` fact so a reaction can tell a directly destroyed entity from
    /// one swept up by a relation cascade.
    fn despawn_with_cause(&mut self, id: EntityId, cause: DestroyCause) {
        if self.index.get(id).is_none() {
            return;
        }
        // fn pointers are Copy; take a local list so we can call &mut self freely.
        let handlers = self.relations.despawn.clone();
        for h in handlers {
            h(self, id);
        }
        // Snapshot what a reaction needs before the entity leaves the world. It is
        // still live here: a cascade handler may have detached it from a target's
        // reverse list, but never strips its own forward `Containment` link, its
        // `Name`, or its `Description`, so `enclosing_locus` and the name still
        // resolve. After `index.remove` below they would not. The name is the
        // entity's `Name` handle, falling back to its `Description` for content
        // that carries only prose (a quick-create thing), mirroring how the app
        // displays it; `None` if it has neither.
        let last_locus = self.enclosing_locus(id);
        let name = self
            .name_of(id)
            .or_else(|| self.get::<Description>(id).map(|d| d.0.clone()));
        self.emit_fact(Fact::Destroyed {
            entity: id,
            last_locus,
            name,
            cause,
        });
        if let Some(e) = self.index.remove(id) {
            let _ = self.ecs.despawn(e);
        }
        // Terminal: the id is dead and carried by `despawned`, so it must not also
        // linger in the live dirty set (a snapshot would skip it anyway, but a
        // failed-save `remark_dirty` should never resurrect a dead id).
        self.dirty.remove(&id);
        // Drop its reverse-index entries (its role as a relation *target*). Its role
        // as a *source* was already detached from every target's list by the despawn
        // handlers above; this evicts the key a component drop used to reclaim for
        // free. Cheap: one pass over the handful of registered relations.
        for m in self.reverse.values_mut() {
            m.remove(&id);
        }
        self.despawned.push(id);
    }

    fn emit_fact(&mut self, fact: Fact) {
        self.facts.push(fact);
    }

    /// Flag an entity's persisted state as changed since the last snapshot. Called
    /// from every mutator path that writes a persisted component or forward relation
    /// link; a delta snapshot serializes exactly this set. Idempotent (a set), so
    /// marking the same entity twice in a tick costs nothing.
    fn mark_dirty(&mut self, id: EntityId) {
        self.dirty.insert(id);
    }

    /// Emit `ComponentChanged` for a tag-driven mutation (set/remove/create), gated
    /// on the tracked set. The runtime `tag` is resolved to its registered
    /// `&'static str` for the fact; an unregistered tag never reaches here (the
    /// mutation would have failed first) and would carry no static tag anyway.
    fn note_component_change(&mut self, entity: EntityId, tag: &str) {
        if self.tracked.contains(tag)
            && let Some(stag) = self.components.static_tag(tag)
        {
            self.emit_fact(Fact::ComponentChanged { entity, tag: stag });
        }
    }

    /// Emit `ComponentChanged` for a typed mutation (`insert`/`remove`/`modify`),
    /// gated on the tracked set. Resolves the tag from `C`'s runtime type; an
    /// unregistered type has none and emits nothing.
    fn note_component_change_typed<C: 'static>(&mut self, entity: EntityId) {
        if let Some(tag) = self.components.tag_of::<C>()
            && self.tracked.contains(tag)
        {
            self.emit_fact(Fact::ComponentChanged { entity, tag });
        }
    }

    /// Drain the structural-fact buffer. The runtime calls this once per tick
    /// before running systems; facts not drained leak into the next tick.
    pub fn take_facts(&mut self) -> Vec<Fact> {
        std::mem::take(&mut self.facts)
    }

    /// An entity's name token, if it has one. Reads the general `Name` component;
    /// the despawn snapshot above is one user, app name resolution is another.
    pub fn name_of(&self, entity: EntityId) -> Option<String> {
        self.get::<Name>(entity).map(|n| n.0.clone())
    }

    /// Whether an entity is live in this world.
    pub fn contains(&self, id: EntityId) -> bool {
        self.index.get(id).is_some()
    }

    pub fn has<C: hecs::Component>(&self, id: EntityId) -> bool {
        self.entity_ref(id).map(|er| er.has::<C>()).unwrap_or(false)
    }

    /// Read one component of a live entity by id, as a shared guard (deref to `&C`).
    /// The addressed-by-id read: there is deliberately no by-value entity handle to
    /// hold, and no `&mut` variant, so the only way to *change* a persisted component
    /// is through the mutator methods (`set_component`/`insert`/`modify`/…), which
    /// keep the dirty set, the index, and the reverse lists consistent. See the
    /// note on `entity_ref`.
    pub fn get<C: hecs::Component>(&self, id: EntityId) -> Option<hecs::Ref<'_, C>> {
        self.ecs.get::<&C>(self.index.get(id)?).ok()
    }

    /// Run a read-only archetypal query. The [`ReadQuery`] bound admits only shared
    /// borrows (`&T` and tuples of them), so a query cannot hand out `&mut C` and
    /// bypass the mutator layer. Structural mutation (spawn/despawn, relation links,
    /// component membership) and component writes go through `World`, so the identity
    /// index, the despawn cascade, the reverse lists, and the persistence dirty set
    /// stay consistent. Making the raw `hecs::World` unreachable is what enforces
    /// that: a raw `ecs.despawn` would bypass the cascade, a raw `ecs.spawn` would
    /// create an `Id`-less entity, and a raw `get::<&mut C>` would drop a persisted
    /// change from the next delta snapshot.
    pub fn query<Q: ReadQuery>(&self) -> hecs::QueryBorrow<'_, Q> {
        self.ecs.query::<Q>()
    }

    /// The raw `hecs::EntityRef` for an entity, for **trusted in-crate** use only
    /// (snapshot serialization, internal reads). It is `pub(crate)` precisely because
    /// `EntityRef::get::<&mut C>` reaches below the mutator layer; keeping it off the
    /// public surface is what makes the unmediated mutation the earlier methods forbid
    /// literally unwritable outside the engine core.
    pub(crate) fn entity_ref(&self, id: EntityId) -> Option<hecs::EntityRef<'_>> {
        self.ecs.entity(self.index.get(id)?).ok()
    }

    /// The one raw `&mut C` borrow in the crate: `modify`'s in-place write to a
    /// live component funnels through here, so the single hazard the raw-mutation
    /// guard polices sits at exactly one auditable site. Module-private on purpose:
    /// were it `pub(crate)`, any in-crate code could take an unbookkept `&mut`
    /// without tripping the `.get::<&mut` lint, reopening the very bypass this
    /// funnel closes. `modify` owns the bookkeeping the raw write skips: it marks
    /// the entity dirty and emits `ComponentChanged` afterward. (Reverse-index
    /// maintenance no longer reaches here; it lives in the `reverse` side map.)
    fn raw_get_mut<C: hecs::Component>(&self, e: hecs::Entity) -> Option<hecs::RefMut<'_, C>> {
        self.ecs.get::<&mut C>(e).ok() // hygiene:allow-raw-mut
    }

    // --- type-erased component mutation (the reflection layer) -----------
    //
    // These mirror how `move_entity` wraps `relate`: the work needs the private
    // registry and ecs, so it lives here. They are the live counterparts of the
    // load path's `deserialize_into`, which is exempt from the relation guard
    // because `rebuild_relations` runs after it; these have no rebuild pass.

    /// Build a root entity from a tag->value blob and spawn it. Location-less: it
    /// never places the entity (placement is a separate `Move`). Refuses any
    /// relation forward-link tag in the blob, which would need `Move`/`Relate`.
    pub fn create(&mut self, components: &Value) -> Result<EntityId, MutateError> {
        let obj = components.as_object().ok_or(RegistryError::NotObject)?;
        for tag in obj.keys() {
            if self.components.is_relation_tag(tag) {
                return Err(MutateError::RelationTag(tag.clone()));
            }
        }
        let mut b = hecs::EntityBuilder::new();
        self.components.deserialize_into(components, &mut b)?;
        let id = self.spawn(b);
        // Post-spawn: the entity is live, so a maintainer that rereads on the trigger
        // sees the new values. Only tracked tags emit; the identity tag `spawn` adds
        // is not in the blob and is never tracked.
        for tag in obj.keys() {
            self.note_component_change(id, tag);
        }
        Ok(id)
    }

    /// Deserialize one component from `value` and overwrite it on a live entity.
    /// Refuses relation forward-link tags (use `Move`/`Relate`) and the identity
    /// tag (`Id` must track the `EntityIndex`).
    pub fn set_component(
        &mut self,
        id: EntityId,
        tag: &str,
        value: Value,
    ) -> Result<(), MutateError> {
        let e = self.index.get(id).ok_or(MutateError::NoSuchEntity(id))?;
        self.guard_tag(tag)?;
        self.components
            .insert_component(&mut self.ecs, e, tag, value)?;
        self.mark_dirty(id);
        self.note_component_change(id, tag);
        Ok(())
    }

    /// Remove one component by tag from a live entity. Same guards as
    /// `set_component`.
    pub fn remove_component(&mut self, id: EntityId, tag: &str) -> Result<(), MutateError> {
        let e = self.index.get(id).ok_or(MutateError::NoSuchEntity(id))?;
        self.guard_tag(tag)?;
        self.components.remove_component(&mut self.ecs, e, tag)?;
        self.mark_dirty(id);
        self.note_component_change(id, tag);
        Ok(())
    }

    /// Insert or overwrite a typed component on a live entity. The typed counterpart
    /// to `set_component` (which takes a runtime tag and JSON), for app systems that
    /// mutate concrete component types on the hot path without a JSON round-trip.
    /// Identity and registered relation forward-link types are rejected before the
    /// write, so this path cannot bypass their structural bookkeeping.
    pub fn insert<C: hecs::Component>(
        &mut self,
        id: EntityId,
        component: C,
    ) -> Result<(), MutateError> {
        let e = self.index.get(id).ok_or(MutateError::NoSuchEntity(id))?;
        self.guard_component_type::<C>()?;
        let _ = self.ecs.insert_one(e, component);
        self.mark_dirty(id);
        self.note_component_change_typed::<C>(id);
        Ok(())
    }

    /// Remove a typed component from a live entity. Returns whether the component
    /// was present. An absent component is a true no-op: it emits no fact and does
    /// not dirty the entity.
    pub fn remove<C: hecs::Component>(&mut self, id: EntityId) -> Result<bool, MutateError> {
        let e = self.index.get(id).ok_or(MutateError::NoSuchEntity(id))?;
        self.guard_component_type::<C>()?;
        if self.ecs.remove_one::<C>(e).is_err() {
            return Ok(false);
        }
        self.mark_dirty(id);
        self.note_component_change_typed::<C>(id);
        Ok(true)
    }

    /// Mutate a component in place: mark the entity dirty and emit `ComponentChanged`.
    /// The sanctioned in-place write, and the only one available outside the core
    /// (there is no public `&mut` component borrow); a raw in-crate `&mut` write would
    /// mutate below the mutator layer, drop the change from the next delta snapshot,
    /// and silently desync a tracked index. Returns `false`
    /// (touching nothing) if the entity or the component is absent, so no trigger or
    /// dirty mark claims a change that did not happen. Typed and JSON-free, for the
    /// hot path.
    pub fn modify<C: hecs::Component>(
        &mut self,
        id: EntityId,
        f: impl FnOnce(&mut C),
    ) -> Result<bool, MutateError> {
        let e = self.index.get(id).ok_or(MutateError::NoSuchEntity(id))?;
        self.guard_component_type::<C>()?;
        {
            // This method IS the sanctioned in-place mutator: it marks dirty and
            // emits the trigger below, so reaching for the raw borrow is warranted.
            let Some(mut c) = self.raw_get_mut::<C>(e) else {
                return Ok(false);
            };
            f(&mut *c);
        }
        self.mark_dirty(id);
        self.note_component_change_typed::<C>(id);
        Ok(true)
    }

    /// Serialize just one named component back to JSON; `None` if absent. The read
    /// half of merge-patch; the engine implements neither the merge nor the verb.
    pub fn component_value(&self, id: EntityId, tag: &str) -> Option<Value> {
        let er = self.entity_ref(id)?;
        self.components.component_value(er, tag).ok().flatten()
    }

    /// Reject the identity tag and relation forward-link tags on the live
    /// set/remove paths.
    fn guard_tag(&self, tag: &str) -> Result<(), MutateError> {
        if tag == Id::TAG {
            return Err(MutateError::IdentityTag(tag.to_string()));
        }
        if self.components.is_relation_tag(tag) {
            return Err(MutateError::RelationTag(tag.to_string()));
        }
        Ok(())
    }

    /// Typed counterpart to [`World::guard_tag`]. Registered relation components
    /// resolve through the component registry populated by `register_relation`; the
    /// identity type is recognized directly so it remains protected even on a
    /// freshly constructed world.
    fn guard_component_type<C: 'static>(&self) -> Result<(), MutateError> {
        if TypeId::of::<C>() == TypeId::of::<Id>() {
            return Err(MutateError::IdentityTag(Id::TAG.to_string()));
        }
        if let Some(tag) = self.components.tag_of::<C>()
            && self.components.is_relation_tag(tag)
        {
            return Err(MutateError::RelationTag(tag.to_string()));
        }
        Ok(())
    }

    // --- generic relation ops -------------------------------------------

    pub fn relate<R: Relation>(
        &mut self,
        source: EntityId,
        target: EntityId,
    ) -> Result<(), RelationError> {
        if self.index.get(source).is_none() {
            return Err(RelationError::NoSuchEntity {
                kind: R::TARGET_TAG.to_string(),
                role: RelationRole::Source,
                entity: source,
            });
        }
        if self.index.get(target).is_none() {
            return Err(RelationError::NoSuchEntity {
                kind: R::TARGET_TAG.to_string(),
                role: RelationRole::Target,
                entity: target,
            });
        }
        if R::ACYCLIC && self.would_cycle::<R>(source, target) {
            return Err(RelationError::Cycle {
                kind: R::TARGET_TAG.to_string(),
                source,
                target,
            });
        }
        let from = self.target_of::<R>(source);
        if from == Some(target) {
            return Ok(());
        }
        // Capture the pre-move locus while the old link still stands; it is gone
        // once the link is rewritten (see `emit_movement`). Only the containment
        // relation reaches this branch.
        let from_locus = if R::EMITS_MOVEMENT {
            self.enclosing_locus(source)
        } else {
            None
        };
        if let Some(old) = from {
            self.remove_source::<R>(old, source);
        }
        let se = self.index.get(source).unwrap();
        let _ = self.ecs.insert_one(se, RelTarget::<R>::new(target));
        self.add_source::<R>(target, source);
        // The forward link is a persisted component on the source; the reverse list
        // on the target is derived (rebuilt on load), so only the source is dirtied.
        self.mark_dirty(source);
        if R::EMITS_MOVEMENT {
            self.emit_movement(source, from, Some(target), from_locus);
        }
        Ok(())
    }

    pub fn unrelate<R: Relation>(&mut self, source: EntityId) {
        self.clear_target::<R>(source);
    }

    /// Type-erased relate: dispatch to the relation registered under `tag` (its
    /// forward-link TARGET_TAG). The runtime face of relate, used by the Relate
    /// action so wiring rides the executor like every other mutation.
    pub fn relate_tag(
        &mut self,
        source: EntityId,
        target: EntityId,
        tag: &str,
    ) -> Result<(), RelationError> {
        let f = self
            .relations
            .relate
            .get(tag)
            .copied()
            .ok_or_else(|| RelationError::UnknownKind(tag.to_string()))?;
        f(self, source, target)
    }

    /// Type-erased unrelate: clear the forward link of the relation registered
    /// under `tag`. The runtime face of unrelate, used by the Unrelate action.
    /// Clearing a link that is not set is a no-op `Ok`, matching the typed
    /// `unrelate`. The type-erased executor-facing path still rejects a missing
    /// source so every action names a live subject.
    pub fn unrelate_tag(&mut self, source: EntityId, tag: &str) -> Result<(), RelationError> {
        let f = self
            .relations
            .unrelate
            .get(tag)
            .copied()
            .ok_or_else(|| RelationError::UnknownKind(tag.to_string()))?;
        if !self.contains(source) {
            return Err(RelationError::NoSuchEntity {
                kind: tag.to_string(),
                role: RelationRole::Source,
                entity: source,
            });
        }
        f(self, source);
        Ok(())
    }

    pub fn target_of<R: Relation>(&self, source: EntityId) -> Option<EntityId> {
        let e = self.index.get(source)?;
        self.ecs.entity(e).ok()?.get::<&RelTarget<R>>().map(|t| t.0)
    }

    /// A target's sources (its reverse list). **Unordered:** the order is
    /// unspecified and not stable across a save/load, because the reverse list is
    /// a derived index rebuilt from the forward links on load, not preserved live
    /// order. A caller that wants a stable display order (contents, exits,
    /// inventory) sorts at the display site by something meaningful to it; the
    /// engine promises membership, not order. Borrowed from the reverse index so
    /// read-only callers pay no allocation; a caller that mutates `World` while
    /// iterating must copy at that ownership boundary.
    pub fn sources_of<R: Relation>(&self, target: EntityId) -> &[EntityId] {
        self.reverse
            .get(&TypeId::of::<R>())
            .and_then(|m| m.get(&target))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Stream the ancestor chain (immediate target first) without allocating.
    /// Callers that need ownership across a world mutation collect explicitly.
    pub fn ancestors<R: AcyclicRelation>(
        &self,
        start: EntityId,
    ) -> impl Iterator<Item = EntityId> + '_ {
        assert!(
            R::ACYCLIC,
            "AcyclicRelation implementors must set Relation::ACYCLIC"
        );
        std::iter::successors(self.target_of::<R>(start), move |&current| {
            self.target_of::<R>(current)
        })
    }

    /// Walk every descendant of `root` in an acyclic relation. `visit` sees each
    /// descendant once and controls traversal with [`Walk`]: descend into that
    /// entity, prune its subtree, or stop the whole walk. The root itself is not
    /// visited. Order is unspecified because reverse relation lists are unordered.
    ///
    /// The traversal owns its DFS stack; the underlying [`World::sources_of`]
    /// query remains allocation-free for ordinary reads.
    pub fn walk_descendants<R: AcyclicRelation>(
        &self,
        root: EntityId,
        mut visit: impl FnMut(EntityId) -> Walk,
    ) {
        assert!(
            R::ACYCLIC,
            "AcyclicRelation implementors must set Relation::ACYCLIC"
        );
        let mut stack = self.sources_of::<R>(root).to_vec();
        while let Some(entity) = stack.pop() {
            match visit(entity) {
                Walk::Descend => stack.extend_from_slice(self.sources_of::<R>(entity)),
                Walk::Prune => {}
                Walk::Stop => break,
            }
        }
    }

    pub fn clear_target<R: Relation>(&mut self, source: EntityId) {
        let from = self.target_of::<R>(source);
        let from_locus = if R::EMITS_MOVEMENT {
            self.enclosing_locus(source)
        } else {
            None
        };
        if let Some(old) = from {
            self.remove_source::<R>(old, source);
        }
        if let Some(se) = self.index.get(source) {
            let _ = self.ecs.remove_one::<RelTarget<R>>(se);
        }
        // Only a link that was actually present changed the source's persisted
        // forward link; clearing an absent link is a no-op and dirties nothing.
        if from.is_some() {
            self.mark_dirty(source);
        }
        // A cleared containment link is a move to root (no container); nothing
        // moved if there was no link to begin with.
        if R::EMITS_MOVEMENT && from.is_some() {
            self.emit_movement(source, from, None, from_locus);
        }
    }

    /// Emit the movement facts for a containment change of `entity`: always
    /// `Moved`, plus `LocusChanged` when the enclosing locus actually differs.
    /// `from_locus` is captured by the caller *before* the change (it is
    /// unrecoverable afterward); `to_locus` is read here, after. Called only for the
    /// containment relation (`R::EMITS_MOVEMENT`), and only for the entity whose own
    /// link changed: a carried subtree keeps its links, so its locus change is
    /// derivable from this fact and is the consumer's to compute, not ours to emit
    /// (see `fact.rs` / facts.md).
    fn emit_movement(
        &mut self,
        entity: EntityId,
        from: Option<EntityId>,
        to: Option<EntityId>,
        from_locus: Option<EntityId>,
    ) {
        self.emit_fact(Fact::Moved { entity, from, to });
        let to_locus = self.enclosing_locus(entity);
        if from_locus != to_locus {
            self.emit_fact(Fact::LocusChanged {
                entity,
                from: from_locus,
                to: to_locus,
            });
        }
    }

    fn would_cycle<R: Relation>(&self, source: EntityId, target: EntityId) -> bool {
        let mut cur = Some(target);
        while let Some(c) = cur {
            if c == source {
                return true;
            }
            cur = self.target_of::<R>(c);
        }
        false
    }

    fn add_source<R: Relation>(&mut self, target: EntityId, source: EntityId) {
        // `relate` (the sole caller) has already verified `target` is live, so no
        // liveness guard here: the old guard existed only because a hecs component
        // could not be inserted on a dead entity, a constraint the side map lifts.
        let sources = self
            .reverse
            .entry(TypeId::of::<R>())
            .or_default()
            .entry(target)
            .or_default();
        if !sources.contains(&source) {
            sources.push(source);
        }
    }

    /// Overwrite a target's reverse list wholesale. Used by relation rebuild,
    /// where sources are unique by construction (no dedup needed).
    pub(crate) fn set_sources<R: Relation>(&mut self, target: EntityId, sources: Vec<EntityId>) {
        self.reverse
            .entry(TypeId::of::<R>())
            .or_default()
            .insert(target, sources);
    }

    fn remove_source<R: Relation>(&mut self, target: EntityId, source: EntityId) {
        // Derived reverse-index maintenance, not persisted (see the `reverse` field).
        if let Some(m) = self.reverse.get_mut(&TypeId::of::<R>())
            && let Some(s) = m.get_mut(&target)
        {
            s.retain(|&x| x != source);
        }
    }

    // --- persistence support (used by snapshot.rs) ----------------------

    pub(crate) fn components(&self) -> &ComponentRegistry {
        &self.components
    }

    /// Pending deletes to include in a snapshot. Does not clear them; see
    /// `confirm_saved`.
    pub(crate) fn pending_deletes(&self) -> Vec<EntityId> {
        self.despawned.clone()
    }

    /// Take the dirty set for a snapshot, clearing it. Drained (not copied like
    /// `pending_deletes`) because a live entity re-mutated after the snapshot must
    /// re-enter the set for the *next* one; a failed save restores the drained ids
    /// via `remark_dirty`. See the `dirty` field.
    pub(crate) fn drain_dirty(&mut self) -> Vec<EntityId> {
        std::mem::take(&mut self.dirty).into_iter().collect()
    }

    /// Return the drained ids of a failed save to the dirty set, so the next
    /// snapshot re-serializes them at their then-current state. A no-op for ids
    /// re-mutated (already re-dirtied) or despawned since the snapshot.
    pub fn remark_dirty(&mut self, ids: &[EntityId]) {
        for &id in ids {
            // A despawned id must not be resurrected into the live set; it rides
            // `despawned` instead and is retried as a delete.
            if self.index.get(id).is_some() {
                self.dirty.insert(id);
            }
        }
    }

    /// Mark every live entity dirty. The one place a load re-enters the dirty set:
    /// after a schema migration, the in-memory world holds the migrated form but the
    /// store still holds the old rows, so every entity must be re-serialized to
    /// persist the migration. An ordinary load leaves the set empty (the store
    /// already matches).
    pub fn mark_all_dirty(&mut self) {
        let ids: Vec<EntityId> = self.ecs.query::<&Id>().iter().map(|id| id.0).collect();
        self.dirty.extend(ids);
    }

    /// The zone an entity belongs to, extracted into the snapshot row for future
    /// shard-scoped loading. Unassigned until sharding exists, so every entity is
    /// `None` today. This is the **one place** a zone is derived, so when zones
    /// become real the choice is forced here: derive it (e.g. walk containment to a
    /// zone-root) or read it from a zone relation. It must **never** become a raw
    /// `EntityId` kept as authoritative component data in the blob, which would be a
    /// cross-reference the despawn cascade cannot see (see sharding.md).
    pub(crate) fn zone_of(&self, _entity: EntityId) -> Option<EntityId> {
        None
    }

    /// Drop the given deletes from the pending set once they're durably saved.
    /// Deletes accumulated since the snapshot are preserved.
    pub fn confirm_saved(&mut self, saved: &[EntityId]) {
        if saved.is_empty() {
            return;
        }
        let set: HashSet<EntityId> = saved.iter().copied().collect();
        self.despawned.retain(|id| !set.contains(id));
    }

    pub(crate) fn set_next_id(&mut self, next_id: u64) {
        self.next_id = next_id;
    }

    pub(crate) fn insert_loaded(&mut self, id: EntityId, built: hecs::BuiltEntity) {
        let e = self.ecs.spawn(built);
        self.index.insert(id, e);
    }

    pub(crate) fn rebuild_relations(&mut self) {
        let handlers = self.relations.rebuild.clone();
        for h in handlers {
            h(self);
        }
    }

    pub(crate) fn validate_relations(&self) -> Result<(), LoadError> {
        for validate in &self.relations.validate {
            validate(self)?;
        }
        Ok(())
    }

    pub(crate) fn is_fresh_for_load(&self) -> bool {
        self.index.is_empty()
            && self.next_id == 1
            && self.despawned.is_empty()
            && self.dirty.is_empty()
            && self.facts.is_empty()
            && self.reverse.is_empty()
    }

    /// Remove entity-derived load state while preserving startup registration,
    /// tracking choices, and transient app resources. This makes a failed load
    /// retryable on the same freshly configured world.
    pub(crate) fn clear_loaded_state(&mut self) {
        self.ecs = hecs::World::new();
        self.index.clear();
        self.next_id = 1;
        self.despawned.clear();
        self.dirty.clear();
        self.facts.clear();
        self.reverse.clear();
    }
}

/// A structural failure from the type-erased mutation paths (`create`,
/// `set_component`, `remove_component`). Thin: it wraps the registry's existing
/// failures and adds the two guards these paths enforce.
#[derive(Debug, thiserror::Error)]
pub enum MutateError {
    #[error("no such entity: {0:?}")]
    NoSuchEntity(EntityId),
    /// A relation forward-link tag was passed to a live mutation path; it must go
    /// through `Move`/`Relate` so the cycle check and reverse index stay correct.
    #[error("relation tag {0} cannot be set directly; use Move/Relate")]
    RelationTag(String),
    /// The identity tag was passed to `set`/`remove`; `Id` must track the index.
    #[error("the identity tag {0} cannot be mutated")]
    IdentityTag(String),
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

// --- per-relation handlers (monomorphized into fn pointers) --------------

fn despawn_relation<R: Relation>(world: &mut World, id: EntityId) {
    // As a source: detach from its current target's reverse list.
    if let Some(t) = world.target_of::<R>(id) {
        world.remove_source::<R>(t, id);
    }
    // As a target: apply the cascade to its sources.
    // The cascade mutates the same reverse index, so take ownership at this
    // mutation boundary rather than making every read-only caller clone.
    let sources = world.sources_of::<R>(id).to_vec();
    if sources.is_empty() {
        return;
    }
    match R::ON_TARGET_DESPAWN {
        Cascade::DespawnSources => {
            for s in sources {
                // Cascade-removed, not directly targeted: this is the single edit
                // that makes `cause` meaningful (a `@destroy <room>` skips its
                // collateral exits; a `@purge` reacts to each Direct removal).
                world.despawn_with_cause(s, DestroyCause::Cascade);
            }
        }
        Cascade::Detach => {
            for s in sources {
                world.clear_target::<R>(s);
            }
        }
        Cascade::Reparent => {
            let up = world.target_of::<R>(id);
            for s in sources {
                match up {
                    Some(u) => {
                        let _ = world.relate::<R>(s, u);
                    }
                    None => world.clear_target::<R>(s),
                }
            }
        }
    }
}

fn relate_by_tag<R: Relation>(
    world: &mut World,
    source: EntityId,
    target: EntityId,
) -> Result<(), RelationError> {
    world.relate::<R>(source, target)
}

fn unrelate_by_tag<R: Relation>(world: &mut World, source: EntityId) {
    world.unrelate::<R>(source);
}

fn validate_relation<R: Relation>(world: &World) -> Result<(), LoadError> {
    let mut edges = HashMap::new();
    let mut q = world.ecs.query::<(&Id, &RelTarget<R>)>();
    for (id, target) in q.iter() {
        if !world.contains(target.0) {
            return Err(LoadError::DanglingRelation {
                kind: R::TARGET_TAG.to_string(),
                source: id.0,
                target: target.0,
            });
        }
        edges.insert(id.0, target.0);
    }
    drop(q);

    if !R::ACYCLIC {
        return Ok(());
    }

    // A relation has at most one outgoing edge, so a color/path walk visits every
    // source at most once overall. `positions` identifies the exact cycle suffix.
    let mut finished = HashSet::new();
    for start in edges.keys().copied() {
        if finished.contains(&start) {
            continue;
        }
        let mut path = Vec::new();
        let mut positions = HashMap::new();
        let mut current = start;
        while edges.contains_key(&current) && !finished.contains(&current) {
            if let Some(&cycle_start) = positions.get(&current) {
                return Err(LoadError::RelationCycle {
                    kind: R::TARGET_TAG.to_string(),
                    cycle: path[cycle_start..].to_vec(),
                });
            }
            positions.insert(current, path.len());
            path.push(current);
            current = edges[&current];
        }
        finished.extend(path);
    }
    Ok(())
}

fn rebuild_relation<R: Relation>(world: &mut World) {
    // Group sources by target, then write each list once. O(n) overall: a
    // source has exactly one RelTarget, so it appears exactly once.
    let mut by_target: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
    {
        let mut q = world.ecs.query::<(&Id, &RelTarget<R>)>();
        for (id, t) in q.iter() {
            by_target.entry(t.0).or_default().push(id.0);
        }
    }
    for (target, sources) in by_target {
        world.set_sources::<R>(target, sources);
    }
}
