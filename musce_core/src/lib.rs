//! Core of the MUSCE MUD engine: the in-memory ECS world, the generic relation
//! layer, and the persistence-facing snapshot model. Pure: no networking, no DB.

pub mod component;
pub mod containment;
pub mod control;
pub mod id;
pub mod relation;
pub mod snapshot;
pub mod world;

// Re-export hecs so dependents can build entities without depending on it directly.
pub use hecs;
// Re-export serde_json's JSON types so the action layer names them without its own
// serde_json dependency.
pub use serde_json::{Map, Value};

pub use component::{
    Container, Creature, Description, Exit, Exits, Id, Item, NamedComponent, Player, RegistryError,
    Room,
};
pub use containment::Containment;
pub use control::{Controls, Focus};
pub use id::{EntityId, EntityIndex};
pub use relation::{Cascade, RelSources, RelTarget, Relation, RelationError};
pub use snapshot::{EntityBlob, Snapshot};
pub use world::{MutateError, World};

#[cfg(test)]
mod tests {
    use super::*;
    use hecs::EntityBuilder;

    fn room(w: &mut World, name: &str) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Room);
        b.add(Description(name.into()));
        w.spawn(b)
    }

    fn item(w: &mut World, name: &str) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Item);
        b.add(Description(name.into()));
        w.spawn(b)
    }

    fn container(w: &mut World, name: &str) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Container);
        b.add(Description(name.into()));
        w.spawn(b)
    }

    #[test]
    fn containment_basic() {
        let mut w = World::new();
        let hall = room(&mut w, "hall");
        let sword = item(&mut w, "sword");
        w.move_entity(sword, hall).unwrap();

        assert_eq!(w.container_of(sword), Some(hall));
        assert_eq!(w.contents(hall), vec![sword]);
    }

    #[test]
    fn enclosing_room_walks_up() {
        let mut w = World::new();
        let hall = room(&mut w, "hall");
        let bag = container(&mut w, "bag");
        let coin = item(&mut w, "coin");
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();

        assert_eq!(w.container_of(coin), Some(bag));
        assert_eq!(w.enclosing_room(coin), Some(hall));
    }

    #[test]
    fn moving_reparents() {
        let mut w = World::new();
        let hall = room(&mut w, "hall");
        let chest = container(&mut w, "chest");
        let gem = item(&mut w, "gem");
        w.move_entity(gem, hall).unwrap();
        w.move_entity(gem, chest).unwrap();

        assert_eq!(w.container_of(gem), Some(chest));
        assert_eq!(w.contents(hall), Vec::<EntityId>::new());
        assert_eq!(w.contents(chest), vec![gem]);
    }

    #[test]
    fn cycles_rejected() {
        let mut w = World::new();
        let a = container(&mut w, "a");
        let b = container(&mut w, "b");
        w.move_entity(b, a).unwrap();
        assert!(matches!(w.move_entity(a, b), Err(RelationError::Cycle)));
        assert!(matches!(w.move_entity(a, a), Err(RelationError::Cycle)));
    }

    #[test]
    fn despawn_reparents_contents() {
        let mut w = World::new();
        let hall = room(&mut w, "hall");
        let bag = container(&mut w, "bag");
        let coin = item(&mut w, "coin");
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();

        w.despawn(bag);

        // bag's contents spill up to the hall; bag is gone.
        assert_eq!(w.container_of(coin), Some(hall));
        assert_eq!(w.enclosing_room(coin), Some(hall));
        assert!(w.entity(bag).is_none());
        let mut contents = w.contents(hall);
        contents.sort();
        assert_eq!(contents, vec![coin]);
    }

    #[test]
    fn descendants_predicate_stops() {
        let mut w = World::new();
        let hall = room(&mut w, "hall");
        let bag = container(&mut w, "bag");
        let coin = item(&mut w, "coin");
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();

        // descend everywhere: see bag and coin
        let mut all = Vec::new();
        w.descendants::<Containment, _, _>(hall, |_| true, |e| all.push(e));
        all.sort();
        let mut expect = vec![bag, coin];
        expect.sort();
        assert_eq!(all, expect);

        // never descend: see only direct contents (bag), not coin
        let mut shallow = Vec::new();
        w.descendants::<Containment, _, _>(hall, |_| false, |e| shallow.push(e));
        assert_eq!(shallow, vec![bag]);
    }

    #[test]
    fn deletes_persist_until_confirmed() {
        let mut w = World::new();
        let a = item(&mut w, "a");
        w.despawn(a);

        // Snapshot copies the delete but does not drop it.
        let s1 = w.snapshot();
        assert_eq!(s1.deletes, vec![a]);

        // A second snapshot (e.g. after the first save failed) still has it.
        assert_eq!(w.snapshot().deletes, vec![a]);

        // Only an explicit confirm clears it.
        w.confirm_saved(&s1.deletes);
        assert!(w.snapshot().deletes.is_empty());
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "disagrees with its Id component")]
    fn load_rejects_mismatched_id() {
        let mut w = World::new();
        let _ = item(&mut w, "x");
        let mut snap = w.snapshot();
        snap.entities[0].id = EntityId(99_999); // disagree with the Id in data

        let mut w2 = World::new();
        let _ = w2.load(&snap.entities, snap.next_id);
    }

    // --- type-erased component mutation ----------------------------------

    #[test]
    fn create_from_blob_spawns_with_components_and_id() {
        let mut w = World::new();
        let before = w.index().len();
        let id = w
            .create(&serde_json::json!({
                "item": null,
                "description": "a brass lamp",
            }))
            .unwrap();

        // The components landed, a fresh Id was assigned, and the index grew.
        assert!(w.has::<Item>(id));
        assert_eq!(w.index().len(), before + 1);
        let er = w.entity(id).unwrap();
        assert_eq!(er.get::<&Description>().unwrap().0, "a brass lamp");
        assert_eq!(er.get::<&Id>().unwrap().0, id);
        // Location-less: create never places it.
        assert_eq!(w.container_of(id), None);
    }

    #[test]
    fn create_rejects_relation_tag_and_spawns_nothing() {
        let mut w = World::new();
        let hall = room(&mut w, "hall");
        let before = w.index().len();

        let err = w.create(&serde_json::json!({
            "item": null,
            "contained_by": hall.0,
        }));
        assert!(matches!(err, Err(MutateError::RelationTag(_))));
        assert_eq!(w.index().len(), before, "nothing should have spawned");
    }

    #[test]
    fn set_component_round_trips() {
        let mut w = World::new();
        let it = item(&mut w, "plain");
        w.set_component(it, "description", serde_json::json!("a shiny coin"))
            .unwrap();
        assert_eq!(
            w.component_value(it, "description"),
            Some(serde_json::json!("a shiny coin"))
        );
    }

    #[test]
    fn set_component_unknown_tag_errors() {
        let mut w = World::new();
        let it = item(&mut w, "x");
        let err = w.set_component(it, "nonesuch", serde_json::json!(1));
        assert!(matches!(
            err,
            Err(MutateError::Registry(RegistryError::UnknownComponent(_)))
        ));
    }

    #[test]
    fn set_component_relation_tag_refused_and_containment_unchanged() {
        let mut w = World::new();
        let hall = room(&mut w, "hall");
        let chest = container(&mut w, "chest");
        let coin = item(&mut w, "coin");
        w.move_entity(coin, hall).unwrap();

        // Trying to retarget containment via the generic setter must fail and
        // leave the existing containment intact.
        let err = w.set_component(coin, "contained_by", serde_json::json!(chest.0));
        assert!(matches!(err, Err(MutateError::RelationTag(_))));
        assert_eq!(w.container_of(coin), Some(hall));
        assert_eq!(w.contents(chest), Vec::<EntityId>::new());
    }

    #[test]
    fn set_component_identity_tag_refused() {
        let mut w = World::new();
        let it = item(&mut w, "x");
        let err = w.set_component(it, "id", serde_json::json!(42));
        assert!(matches!(err, Err(MutateError::IdentityTag(_))));
    }

    #[test]
    fn remove_component_removes_present() {
        let mut w = World::new();
        let it = item(&mut w, "x");
        assert!(w.component_value(it, "description").is_some());
        w.remove_component(it, "description").unwrap();
        assert_eq!(w.component_value(it, "description"), None);
    }

    #[test]
    fn remove_component_refuses_id_and_relation_tags() {
        let mut w = World::new();
        let it = item(&mut w, "x");
        assert!(matches!(
            w.remove_component(it, "id"),
            Err(MutateError::IdentityTag(_))
        ));
        assert!(matches!(
            w.remove_component(it, "contained_by"),
            Err(MutateError::RelationTag(_))
        ));
    }

    #[test]
    fn component_value_absent_is_none() {
        let mut w = World::new();
        let mut b = EntityBuilder::new();
        b.add(Item);
        let bare = w.spawn(b);
        assert_eq!(w.component_value(bare, "description"), None);
    }

    #[test]
    fn mutate_missing_entity_errors() {
        let mut w = World::new();
        let ghost = EntityId(9999);
        assert!(matches!(
            w.set_component(ghost, "description", serde_json::json!("x")),
            Err(MutateError::NoSuchEntity(_))
        ));
    }

    #[test]
    fn snapshot_roundtrip() {
        let mut w = World::new();
        let hall = room(&mut w, "hall");
        let bag = container(&mut w, "bag");
        let coin = item(&mut w, "coin");
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();

        let snap = w.snapshot();

        let mut w2 = World::new();
        w2.load(&snap.entities, snap.next_id).unwrap();

        // structure survives, reverse lists rebuilt
        assert_eq!(w2.container_of(coin), Some(bag));
        assert_eq!(w2.container_of(bag), Some(hall));
        assert_eq!(w2.enclosing_room(coin), Some(hall));
        assert_eq!(w2.contents(bag), vec![coin]);
        assert!(w2.has::<Room>(hall));
        assert!(w2.has::<Container>(bag));
        assert!(w2.has::<Item>(coin));
        assert_eq!(w2.next_id(), snap.next_id);
    }
}
