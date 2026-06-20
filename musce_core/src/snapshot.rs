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
    /// Serialize every live entity. Forward relation links are included; reverse
    /// lists and the index are derived and omitted.
    ///
    /// TODO(perf): this is a full serialize of the whole world, run synchronously
    /// on the sim thread, so it costs O(entities) of allocation and JSON work per
    /// save and surfaces as a periodic tick-time spike that grows with world size.
    /// Dirty-tracked / incremental snapshots are the fix (deferred; see the README
    /// roadmap and persistence.md). Fine at the current scale.
    pub fn snapshot(&mut self) -> Snapshot {
        let entities_h: Vec<hecs::Entity> = self.ecs.query::<hecs::Entity>().iter().collect();

        let mut entities = Vec::with_capacity(entities_h.len());
        for e in entities_h {
            let er = self.ecs.entity(e).expect("entity from query still exists");
            let id = er.get::<&Id>().expect("every entity has Id").0;
            let data = self.components().serialize_entity(er);
            entities.push(EntityBlob {
                id,
                zone: None,
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
                self.entity(blob.id)
                    .and_then(|er| er.get::<&Id>().map(|i| i.0)),
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
