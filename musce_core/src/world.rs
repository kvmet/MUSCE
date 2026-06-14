use crate::component::{
    ComponentRegistry, Container, Creature, Description, Exits, Id, Item, NamedComponent, Player,
    Room,
};
use crate::containment::Containment;
use crate::id::{EntityId, EntityIndex};
use crate::relation::{Cascade, RelSources, RelTarget, Relation, RelationError};

type DespawnHandler = fn(&mut World, EntityId);
type RebuildHandler = fn(&mut World);

/// Type-erased per-relation cleanup hooks, populated by `register_relation`.
#[derive(Default, Clone)]
struct RelationRegistry {
    despawn: Vec<DespawnHandler>,
    rebuild: Vec<RebuildHandler>,
}

/// The authoritative in-memory game state: a hecs World plus the identity index
/// and the registries that drive persistence and relation bookkeeping.
pub struct World {
    pub ecs: hecs::World,
    index: EntityIndex,
    next_id: u64,
    relations: RelationRegistry,
    components: ComponentRegistry,
    /// EntityIds despawned since the last snapshot, for the persistence delete set.
    despawned: Vec<EntityId>,
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
        };
        w.register_defaults();
        w
    }

    fn register_defaults(&mut self) {
        self.register_component::<Id>();
        self.register_component::<Description>();
        self.register_component::<Exits>();
        self.register_component::<Room>();
        self.register_component::<Item>();
        self.register_component::<Creature>();
        self.register_component::<Container>();
        self.register_component::<Player>();
        self.register_relation::<Containment>();
    }

    // --- registration ----------------------------------------------------

    pub fn register_component<C: NamedComponent>(&mut self) {
        self.components.register::<C>();
    }

    pub fn register_relation<R: Relation>(&mut self) {
        // The forward link is a persisted component; the reverse list is not.
        self.register_component::<RelTarget<R>>();
        self.relations.despawn.push(despawn_relation::<R>);
        self.relations.rebuild.push(rebuild_relation::<R>);
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

    /// Despawn an entity, running every relation's cascade first.
    pub fn despawn(&mut self, id: EntityId) {
        if self.index.get(id).is_none() {
            return;
        }
        // fn pointers are Copy; take a local list so we can call &mut self freely.
        let handlers = self.relations.despawn.clone();
        for h in handlers {
            h(self, id);
        }
        if let Some(e) = self.index.remove(id) {
            let _ = self.ecs.despawn(e);
        }
        self.despawned.push(id);
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
        if let Some(old) = self.target_of::<R>(source) {
            if old == target {
                return Ok(());
            }
            self.remove_source::<R>(old, source);
        }
        let se = self.index.get(source).unwrap();
        let _ = self.ecs.insert_one(se, RelTarget::<R>::new(target));
        self.add_source::<R>(target, source);
        Ok(())
    }

    pub fn unrelate<R: Relation>(&mut self, source: EntityId) {
        self.clear_target::<R>(source);
    }

    pub fn target_of<R: Relation>(&self, source: EntityId) -> Option<EntityId> {
        let e = self.index.get(source)?;
        self.ecs.entity(e).ok()?.get::<&RelTarget<R>>().map(|t| t.0)
    }

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
        if let Some(old) = self.target_of::<R>(source) {
            self.remove_source::<R>(old, source);
        }
        if let Some(se) = self.index.get(source) {
            let _ = self.ecs.remove_one::<RelTarget<R>>(se);
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

    pub(crate) fn take_despawned(&mut self) -> Vec<EntityId> {
        std::mem::take(&mut self.despawned)
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
                world.despawn(s);
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

fn rebuild_relation<R: Relation>(world: &mut World) {
    let pairs: Vec<(EntityId, EntityId)> = world
        .ecs
        .query::<(&Id, &RelTarget<R>)>()
        .iter()
        .map(|(id, t)| (id.0, t.0))
        .collect();
    for (s, t) in pairs {
        world.add_source::<R>(t, s);
    }
}
