use std::marker::PhantomData;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::component::NamedComponent;
use crate::id::EntityId;

/// What happens to a target's sources when the target is despawned. Fixed per
/// relation kind (a `const`); a despawn-site override can be added later if a
/// relation ever needs context-dependent behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cascade {
    /// Destroy the sources too (recursively).
    DespawnSources,
    /// Move sources up to the dying target's own target; roots if none.
    Reparent,
    /// Detach sources (clear the relation); they become roots.
    Detach,
}

/// A relationship kind. One-to-many: a source has at most one target; a target
/// has many sources. One marker type per relationship (e.g. `Containment`).
pub trait Relation: 'static + Send + Sync {
    /// Tree-shaped? If so, `relate` rejects edges that would create a cycle.
    const ACYCLIC: bool;
    /// Despawn behavior for this relation's sources when their target dies.
    const ON_TARGET_DESPAWN: Cascade;
    /// Serialization tag for the forward link of this relation.
    const TARGET_TAG: &'static str;
}

/// Forward link, stored on the source side: which target this source points to.
/// This is the source of truth and is persisted.
pub struct RelTarget<R: Relation>(pub EntityId, PhantomData<R>);

impl<R: Relation> RelTarget<R> {
    pub fn new(target: EntityId) -> Self {
        Self(target, PhantomData)
    }
}

/// Reverse list, stored on the target side: which sources point at this target.
/// A derived index, rebuilt from `RelTarget` on load and never persisted.
pub struct RelSources<R: Relation>(pub Vec<EntityId>, PhantomData<R>);

impl<R: Relation> RelSources<R> {
    pub fn new(sources: Vec<EntityId>) -> Self {
        Self(sources, PhantomData)
    }

    pub fn ids(&self) -> &[EntityId] {
        &self.0
    }
}

// Transparent serde for the forward link: serialize only the inner EntityId,
// independent of the marker type R.
impl<R: Relation> Serialize for RelTarget<R> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, R: Relation> Deserialize<'de> for RelTarget<R> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(RelTarget(EntityId::deserialize(d)?, PhantomData))
    }
}

impl<R: Relation> NamedComponent for RelTarget<R> {
    const TAG: &'static str = R::TARGET_TAG;
}

#[derive(Debug, thiserror::Error)]
pub enum RelationError {
    #[error("no such entity: {0:?}")]
    NoSuchEntity(EntityId),
    #[error("relation would create a cycle")]
    Cycle,
}
