use crate::component::Locus;
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
    // The one spatial relation: a containment change is a move, and crossing a
    // `Locus` is a perception-scope change. See the movement facts in `fact.rs`.
    const EMITS_MOVEMENT: bool = true;
}

impl World {
    /// Move an entity into a container. The single mutator for containment:
    /// rejects cycles and keeps both sides of the relation consistent.
    pub fn move_entity(&mut self, entity: EntityId, into: EntityId) -> Result<(), RelationError> {
        self.relate::<Containment>(entity, into)
    }

    /// Immediate contents of a container (one level). **Unordered** (see
    /// [`World::sources_of`]): order is not stable across a save/load, so a caller
    /// wanting a stable display order sorts at the display site.
    pub fn contents(&self, container: EntityId) -> Vec<EntityId> {
        self.sources_of::<Containment>(container)
    }

    /// The container an entity directly sits in, if any.
    pub fn container_of(&self, entity: EntityId) -> Option<EntityId> {
        self.target_of::<Containment>(entity)
    }

    /// Nearest enclosing [`Locus`], walking up through any nested containers. The
    /// generic scope-boundary query: no room semantics, just the closest ancestor
    /// carrying the boundary marker.
    pub fn enclosing_locus(&self, entity: EntityId) -> Option<EntityId> {
        self.ancestors::<Containment>(entity)
            .into_iter()
            .find(|&a| self.has::<Locus>(a))
    }
}
