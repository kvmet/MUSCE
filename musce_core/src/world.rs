use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::component::{
    ComponentRegistry, Description, Id, Locus, Name, NamedComponent, RegistryError,
};
use crate::containment::Containment;
use crate::control::{Controls, Focus};
use crate::fact::{DestroyCause, Fact};
use crate::id::{EntityId, EntityIndex};
use crate::relation::{Cascade, RelSources, RelTarget, Relation, RelationError};

type DespawnHandler = fn(&mut World, EntityId);
type RebuildHandler = fn(&mut World);
type RelateFn = fn(&mut World, EntityId, EntityId) -> Result<(), RelationError>;
type UnrelateFn = fn(&mut World, EntityId);

/// Type-erased per-relation cleanup hooks, populated by `register_relation`.
#[derive(Default, Clone)]
struct RelationRegistry {
    despawn: Vec<DespawnHandler>,
    rebuild: Vec<RebuildHandler>,
    relate: HashMap<&'static str, RelateFn>,
    unrelate: HashMap<&'static str, UnrelateFn>,
}

/// The authoritative in-memory game state: a hecs World plus the identity index
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
    /// Structural facts emitted since the last drain: a transient per-tick buffer
    /// the runtime drains via `take_facts` before running systems. Not persisted
    /// (a snapshot serializes only registered components); mirrors `despawned`.
    facts: Vec<Fact>,
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
            facts: Vec::new(),
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

    pub fn register_relation<R: Relation>(&mut self) {
        // The forward link is a persisted component; the reverse list is not.
        self.register_component::<RelTarget<R>>();
        // The live mutation paths must refuse forward-link tags; they bypass the
        // cycle check and reverse-index bookkeeping that `relate` owns.
        self.components.mark_relation_tag(R::TARGET_TAG);
        self.relations.despawn.push(despawn_relation::<R>);
        self.relations.rebuild.push(rebuild_relation::<R>);
        self.relations
            .relate
            .insert(R::TARGET_TAG, relate_by_tag::<R>);
        self.relations
            .unrelate
            .insert(R::TARGET_TAG, unrelate_by_tag::<R>);
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
        // that carries only prose (a quick-create thing), mirroring how the game
        // displays it; `None` if it has neither.
        let last_locus = self.enclosing_locus(id);
        let name = self.name_of(id).or_else(|| {
            self.entity(id)
                .and_then(|er| er.get::<&Description>().map(|d| d.0.clone()))
        });
        self.emit_fact(Fact::Destroyed {
            entity: id,
            last_locus,
            name,
            cause,
        });
        if let Some(e) = self.index.remove(id) {
            let _ = self.ecs.despawn(e);
        }
        self.despawned.push(id);
    }

    fn emit_fact(&mut self, fact: Fact) {
        self.facts.push(fact);
    }

    /// Drain the structural-fact buffer. The runtime calls this once per tick
    /// before running systems; facts not drained leak into the next tick.
    pub fn take_facts(&mut self) -> Vec<Fact> {
        std::mem::take(&mut self.facts)
    }

    /// An entity's name token, if it has one. Reads the general `Name` component;
    /// the despawn snapshot above is one user, game name resolution is another.
    pub fn name_of(&self, entity: EntityId) -> Option<String> {
        self.entity(entity)?.get::<&Name>().map(|n| n.0.clone())
    }

    pub fn has<C: hecs::Component>(&self, id: EntityId) -> bool {
        self.index
            .get(id)
            .and_then(|e| self.ecs.entity(e).ok())
            .map(|er| er.has::<C>())
            .unwrap_or(false)
    }

    pub fn entity(&self, id: EntityId) -> Option<hecs::EntityRef<'_>> {
        self.ecs.entity(self.index.get(id)?).ok()
    }

    /// Read-only access to the underlying hecs world, for archetypal queries and
    /// component reads (`query`, `get`, including `get::<&mut C>` via hecs's
    /// interior borrow tracking). Structural mutation (spawn/despawn, relation
    /// links, component set membership) goes through `World` so the identity index,
    /// the cascade, and the reverse lists stay consistent, which is why this hands
    /// out `&self` and the field itself is not public: a raw `ecs.despawn` would
    /// bypass the cascade and resurrect the entity on the next load, and a raw
    /// `ecs.spawn` would create an entity with no `Id`, invisible to every lookup
    /// and a panic at the next snapshot.
    pub fn ecs(&self) -> &hecs::World {
        &self.ecs
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
        Ok(self.spawn(b))
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
        Ok(())
    }

    /// Remove one component by tag from a live entity. Same guards as
    /// `set_component`.
    pub fn remove_component(&mut self, id: EntityId, tag: &str) -> Result<(), MutateError> {
        let e = self.index.get(id).ok_or(MutateError::NoSuchEntity(id))?;
        self.guard_tag(tag)?;
        self.components.remove_component(&mut self.ecs, e, tag)?;
        Ok(())
    }

    /// Insert or overwrite a typed component on a live entity; no-op if the id is
    /// absent. The typed counterpart to `set_component` (which takes a runtime tag
    /// and JSON), for game systems that mutate concrete component types on the hot
    /// path without a JSON round-trip. Not for relation forward links (use
    /// `Move`/`relate`, which keep the reverse index and cycle check correct) or
    /// `Id` (identity must track the index); unlike the tag-driven path there is no
    /// runtime guard, because naming one of those types here is a mistake visible at
    /// the call site, not a runtime-string misroute.
    pub fn insert<C: hecs::Component>(&mut self, id: EntityId, component: C) {
        if let Some(e) = self.index.get(id) {
            let _ = self.ecs.insert_one(e, component);
        }
    }

    /// Remove a typed component from a live entity; no-op if the id or the component
    /// is absent. The typed counterpart to `remove_component`; same caveats as
    /// `insert`.
    pub fn remove<C: hecs::Component>(&mut self, id: EntityId) {
        if let Some(e) = self.index.get(id) {
            let _ = self.ecs.remove_one::<C>(e);
        }
    }

    /// Serialize just one named component back to JSON; `None` if absent. The read
    /// half of merge-patch; the engine implements neither the merge nor the verb.
    pub fn component_value(&self, id: EntityId, tag: &str) -> Option<Value> {
        let er = self.entity(id)?;
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

    // --- generic relation ops -------------------------------------------

    pub fn relate<R: Relation>(
        &mut self,
        source: EntityId,
        target: EntityId,
    ) -> Result<(), RelationError> {
        if self.index.get(source).is_none() {
            return Err(RelationError::NoSuchEntity(source));
        }
        if self.index.get(target).is_none() {
            return Err(RelationError::NoSuchEntity(target));
        }
        if R::ACYCLIC && self.would_cycle::<R>(source, target) {
            return Err(RelationError::Cycle);
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
    /// `unrelate`; only an unregistered `tag` is an error.
    pub fn unrelate_tag(&mut self, source: EntityId, tag: &str) -> Result<(), RelationError> {
        let f = self
            .relations
            .unrelate
            .get(tag)
            .copied()
            .ok_or_else(|| RelationError::UnknownKind(tag.to_string()))?;
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
    /// engine promises membership, not order.
    pub fn sources_of<R: Relation>(&self, target: EntityId) -> Vec<EntityId> {
        self.index
            .get(target)
            .and_then(|e| self.ecs.entity(e).ok())
            .and_then(|er| er.get::<&RelSources<R>>().map(|s| s.0.clone()))
            .unwrap_or_default()
    }

    /// Ancestor chain (immediate target first), following the relation upward.
    pub fn ancestors<R: Relation>(&self, start: EntityId) -> Vec<EntityId> {
        let mut out = Vec::new();
        let mut cur = self.target_of::<R>(start);
        while let Some(c) = cur {
            out.push(c);
            cur = self.target_of::<R>(c);
        }
        out
    }

    /// Walk all descendants of `root`. `descend` decides whether to recurse into
    /// a given node (game policy); `visit` is called for every descendant.
    pub fn descendants<R, D, V>(&self, root: EntityId, mut descend: D, mut visit: V)
    where
        R: Relation,
        D: FnMut(EntityId) -> bool,
        V: FnMut(EntityId),
    {
        let mut stack = self.sources_of::<R>(root);
        while let Some(n) = stack.pop() {
            visit(n);
            if descend(n) {
                stack.extend(self.sources_of::<R>(n));
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
        let Some(te) = self.index.get(target) else {
            return;
        };
        if let Ok(mut s) = self.ecs.get::<&mut RelSources<R>>(te) {
            if !s.0.contains(&source) {
                s.0.push(source);
            }
            return;
        }
        let _ = self.ecs.insert_one(te, RelSources::<R>::new(vec![source]));
    }

    /// Overwrite a target's reverse list wholesale. Used by relation rebuild,
    /// where sources are unique by construction (no dedup needed).
    pub(crate) fn set_sources<R: Relation>(&mut self, target: EntityId, sources: Vec<EntityId>) {
        if let Some(te) = self.index.get(target) {
            let _ = self.ecs.insert_one(te, RelSources::<R>::new(sources));
        }
    }

    fn remove_source<R: Relation>(&mut self, target: EntityId, source: EntityId) {
        if let Some(te) = self.index.get(target)
            && let Ok(mut s) = self.ecs.get::<&mut RelSources<R>>(te)
        {
            s.0.retain(|&x| x != source);
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
    let sources = world.sources_of::<R>(id);
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
