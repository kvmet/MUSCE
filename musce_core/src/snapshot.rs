use serde_json::Value;

use crate::component::{Id, RegistryError};
use crate::id::EntityId;
use crate::world::World;

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

    /// Load entities into a fresh World: spawn each (forward links and all),
    /// then rebuild reverse relation lists. Order-independent because links are
    /// EntityIds resolved through the index, not raw handles.
    pub fn load(&mut self, blobs: &[EntityBlob], next_id: u64) -> Result<(), RegistryError> {
        for blob in blobs {
            let mut b = hecs::EntityBuilder::new();
            self.components().deserialize_into(&blob.data, &mut b)?;
            self.insert_loaded(blob.id, b.build());
            // The DB primary key and the entity's own Id component must agree;
            // a mismatch means a corrupt or wrongly-keyed blob.
            debug_assert_eq!(
                self.get::<Id>(blob.id).map(|i| i.0),
                Some(blob.id),
                "blob.id {:?} disagrees with its Id component",
                blob.id,
            );
        }
        self.set_next_id(next_id);
        self.rebuild_relations();
        Ok(())
    }
}
