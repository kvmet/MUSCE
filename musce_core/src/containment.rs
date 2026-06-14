use crate::component::Room;
use crate::id::EntityId;
use crate::relation::{Cascade, Relation, RelationError};
use crate::world::World;

/// The containment hierarchy: rooms, containers, and inventories are all the
/// same relation. A thing's target is the container it sits in.
pub struct Containment;

impl Relation for Containment {
    const ACYCLIC: bool = true;
    const ON_TARGET_DESPAWN: Cascade = Cascade::Reparent;
    const TARGET_TAG: &'static str = "contained_by";
}

impl World {
    /// Move an entity into a container. The single mutator for containment:
    /// rejects cycles and keeps both sides of the relation consistent.
    pub fn move_entity(&mut self, entity: EntityId, into: EntityId) -> Result<(), RelationError> {
        self.relate::<Containment>(entity, into)
    }

    /// Immediate contents of a container (one level).
    pub fn contents(&self, container: EntityId) -> Vec<EntityId> {
        self.sources_of::<Containment>(container)
    }

    /// The container an entity directly sits in, if any.
    pub fn container_of(&self, entity: EntityId) -> Option<EntityId> {
        self.target_of::<Containment>(entity)
    }

    /// Nearest enclosing room, walking up through any nested containers.
    pub fn enclosing_room(&self, entity: EntityId) -> Option<EntityId> {
        self.ancestors::<Containment>(entity)
            .into_iter()
            .find(|&a| self.has::<Room>(a))
    }
}
