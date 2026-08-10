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
    /// Whether a change to this relation is a spatial *move* that emits `Moved`
    /// (and `LocusChanged` when the enclosing `Locus` differs). True only for
    /// `Containment`, the engine's one spatial relation: `enclosing_locus` is
    /// defined over it, so it is the only relation whose reparenting changes an
    /// entity's perception scope. Default `false`; the emit code is compiled out
    /// for every other relation.
    const EMITS_MOVEMENT: bool = false;
}

/// A relation whose forward links are guaranteed acyclic. Tree-walking queries
/// require this marker so a cyclic relation cannot accidentally enter an
/// unbounded ancestor or descendant walk. Implementors must also set
/// [`Relation::ACYCLIC`] to `true`; `World::relate` enforces that promise on every
/// write.
pub trait AcyclicRelation: Relation {}

/// What a descendant visitor wants to do after seeing one entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walk {
    /// Continue into this entity's sources.
    Descend,
    /// Keep walking elsewhere, but skip this entity's subtree.
    Prune,
    /// Stop the whole traversal immediately.
    Stop,
}

/// Forward link, stored on the source side: which target this source points to.
/// This is the source of truth and is persisted.
pub struct RelTarget<R: Relation>(pub EntityId, PhantomData<R>);

impl<R: Relation> RelTarget<R> {
    pub fn new(target: EntityId) -> Self {
        Self(target, PhantomData)
    }
}

// The reverse of this link (a target's sources) is a derived index rebuilt from
// the forward links on load; it lives in a side map on `World`, not as a
// component, since it is only ever point-looked-up by target. See `World::reverse`.

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

#[derive(Debug)]
pub enum RelationError {
    NoSuchEntity {
        kind: String,
        role: RelationRole,
        entity: EntityId,
    },
    Cycle {
        kind: String,
        source: EntityId,
        target: EntityId,
    },
    UnknownKind(String),
}

impl std::fmt::Display for RelationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchEntity { kind, role, entity } => {
                write!(f, "relation {kind} has no {role} entity {entity:?}")
            }
            Self::Cycle {
                kind,
                source,
                target,
            } => write!(
                f,
                "relation {kind} from {source:?} to {target:?} would create a cycle"
            ),
            Self::UnknownKind(kind) => write!(f, "unknown relation kind: {kind}"),
        }
    }
}

impl std::error::Error for RelationError {}

/// Which endpoint of a requested relation operation was missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationRole {
    Source,
    Target,
}

impl std::fmt::Display for RelationRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source => f.write_str("source"),
            Self::Target => f.write_str("target"),
        }
    }
}
