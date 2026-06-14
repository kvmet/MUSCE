//! Core of the MUSCE MUD engine: the in-memory ECS world, the generic relation
//! layer, and the persistence-facing snapshot model. Pure: no networking, no DB.

pub mod component;
pub mod containment;
pub mod id;
pub mod relation;
pub mod snapshot;
pub mod world;

// Re-export hecs so dependents can build entities without depending on it directly.
pub use hecs;

pub use component::{
    Container, Creature, Description, Exit, Exits, Id, Item, NamedComponent, Player,
    RegistryError, Room,
};
pub use containment::Containment;
pub use id::{EntityId, EntityIndex};
pub use relation::{Cascade, RelSources, RelTarget, Relation, RelationError};
pub use snapshot::{EntityBlob, Snapshot};
pub use world::World;

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
