use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Global, location-independent entity identity. Distinct from `hecs::Entity`
/// (which is local to one World) so it can survive persistence and, later,
/// crossing shard boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntityId(pub u64);

/// Per-world map from global `EntityId` to the local `hecs::Entity` handle.
/// Rebuilt on load; never persisted.
#[derive(Default)]
pub struct EntityIndex {
    fwd: HashMap<EntityId, hecs::Entity>,
}

impl EntityIndex {
    pub fn insert(&mut self, id: EntityId, e: hecs::Entity) {
        self.fwd.insert(id, e);
    }

    pub fn remove(&mut self, id: EntityId) -> Option<hecs::Entity> {
        self.fwd.remove(&id)
    }

    pub fn get(&self, id: EntityId) -> Option<hecs::Entity> {
        self.fwd.get(&id).copied()
    }

    pub fn len(&self) -> usize {
        self.fwd.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fwd.is_empty()
    }

    pub fn clear(&mut self) {
        self.fwd.clear();
    }
}
