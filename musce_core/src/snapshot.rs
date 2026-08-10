use std::collections::HashSet;

use serde_json::Value;

use crate::component::{Id, NamedComponent, RegistryError};
use crate::id::EntityId;
use crate::world::World;

/// A failure to reconstruct a complete, internally consistent world.
#[derive(Debug)]
pub enum LoadError {
    WorldNotFresh,
    DuplicateEntity(EntityId),
    NonObjectBlob {
        blob_id: EntityId,
    },
    InvalidIdComponent {
        blob_id: EntityId,
        source: serde_json::Error,
    },
    IdMismatch {
        blob_id: EntityId,
        component_id: Option<EntityId>,
    },
    IdSpaceExhausted {
        highest_id: EntityId,
    },
    DanglingRelation {
        kind: String,
        source: EntityId,
        target: EntityId,
    },
    RelationCycle {
        kind: String,
        cycle: Vec<EntityId>,
    },
    Registry(RegistryError),
}

impl From<RegistryError> for LoadError {
    fn from(error: RegistryError) -> Self {
        Self::Registry(error)
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WorldNotFresh => {
                f.write_str("World::load requires a fresh world with no prior entity-derived state")
            }
            Self::DuplicateEntity(entity) => {
                write!(f, "snapshot contains duplicate entity id {entity:?}")
            }
            Self::NonObjectBlob { blob_id } => {
                write!(f, "blob {blob_id:?} component data is not an object")
            }
            Self::InvalidIdComponent { blob_id, source } => {
                write!(f, "blob {blob_id:?} has a malformed Id component: {source}")
            }
            Self::IdMismatch {
                blob_id,
                component_id,
            } => write!(
                f,
                "blob id {blob_id:?} disagrees with Id component {component_id:?}"
            ),
            Self::IdSpaceExhausted { highest_id } => write!(
                f,
                "loaded entity id {highest_id:?} leaves no successor for identity allocation"
            ),
            Self::DanglingRelation {
                kind,
                source,
                target,
            } => write!(
                f,
                "relation {kind} from {source:?} points to missing target {target:?}"
            ),
            Self::RelationCycle { kind, cycle } => {
                write!(f, "relation {kind} contains cycle {cycle:?}")
            }
            Self::Registry(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidIdComponent { source, .. } => Some(source),
            Self::Registry(error) => Some(error),
            _ => None,
        }
    }
}

/// One persisted entity: its id, an optional zone (extracted for shard-scoped
/// loading; unused for now), and its components as a JSON object.
#[derive(Debug, Clone)]
pub struct EntityBlob {
    pub id: EntityId,
    pub zone: Option<EntityId>,
    pub data: Value,
}

/// A point-in-time save payload, produced on the sim thread and handed to the
/// persistence layer. `deletes` covers entities despawned since the last save.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub entities: Vec<EntityBlob>,
    pub deletes: Vec<EntityId>,
    pub next_id: u64,
}

impl World {
    /// Serialize the entities changed since the last snapshot (the drained dirty
    /// set), not the whole world: a delta save costs O(changed), not O(world), so it
    /// carries no periodic tick-time spike and can run at a high cadence. Forward
    /// relation links are included; reverse lists and the index are derived and
    /// omitted. A dirtied id since despawned is skipped here and carried by
    /// `deletes` instead.
    ///
    /// The dirty set is *drained*: a save writes this delta, and anything re-mutated
    /// after the snapshot re-enters the set for the next one. The confirm contract
    /// is therefore asymmetric with deletes: a failed save must restore the drained
    /// ids (`remark_dirty`), whereas deletes are copied and cleared on ack
    /// (`confirm_saved`). The very first snapshot of a freshly seeded world is a full
    /// save (every spawn dirtied it); a loaded world starts clean (the store already
    /// matches).
    pub fn snapshot(&mut self) -> Snapshot {
        let ids = self.drain_dirty();

        let mut entities = Vec::with_capacity(ids.len());
        for id in ids {
            // A dirtied entity may have been despawned before this snapshot; it is
            // covered by `deletes`, so it is skipped rather than serialized dead.
            let Some(er) = self.entity_ref(id) else {
                continue;
            };
            let data = self.components().serialize_entity(er);
            entities.push(EntityBlob {
                id,
                zone: self.zone_of(id),
                data,
            });
        }

        Snapshot {
            entities,
            deletes: self.pending_deletes(),
            next_id: self.next_id(),
        }
    }

    /// Load a complete snapshot into a fresh world with no prior entity-derived
    /// state. Startup registrations, tracking choices, and resources may already
    /// be installed.
    /// All blobs deserialize and all identities/relations validate before reverse
    /// indexes are rebuilt. On any error after the freshness precondition passes,
    /// the receiver has no live entities and can be reused for another load attempt
    /// without repeating startup wiring.
    pub fn load(&mut self, blobs: &[EntityBlob], next_id: u64) -> Result<(), LoadError> {
        if !self.is_fresh_for_load() {
            return Err(LoadError::WorldNotFresh);
        }

        let mut seen = HashSet::with_capacity(blobs.len());
        let mut staged = Vec::with_capacity(blobs.len());
        let mut highest_id = None;
        for blob in blobs {
            if !seen.insert(blob.id) {
                return Err(LoadError::DuplicateEntity(blob.id));
            }
            highest_id = Some(highest_id.map_or(blob.id, |highest: EntityId| highest.max(blob.id)));
            let components = blob
                .data
                .as_object()
                .ok_or(LoadError::NonObjectBlob { blob_id: blob.id })?;
            let component_id = match components.get(Id::TAG) {
                Some(value) => Some(
                    serde_json::from_value::<Id>(value.clone())
                        .map_err(|source| LoadError::InvalidIdComponent {
                            blob_id: blob.id,
                            source,
                        })?
                        .0,
                ),
                None => None,
            };
            if component_id != Some(blob.id) {
                return Err(LoadError::IdMismatch {
                    blob_id: blob.id,
                    component_id,
                });
            }
            let mut b = hecs::EntityBuilder::new();
            self.components().deserialize_into(&blob.data, &mut b)?;
            staged.push((blob.id, b));
        }

        let minimum_next_id = match highest_id {
            Some(highest_id) => highest_id
                .0
                .checked_add(1)
                .ok_or(LoadError::IdSpaceExhausted { highest_id })?,
            None => 1,
        };
        let next_id = next_id.max(minimum_next_id);

        for (id, mut builder) in staged {
            self.insert_loaded(id, builder.build());
        }
        if let Err(error) = self.validate_relations() {
            self.clear_loaded_state();
            return Err(error);
        }
        self.set_next_id(next_id);
        self.rebuild_relations();
        Ok(())
    }
}
