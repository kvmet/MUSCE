//! Exit entities: room connectivity carried by an intermediate entity whose two
//! endpoints each fit the one-to-many relation layer. An exit has one origin
//! (LeadsFrom) and one destination (LeadsTo); a room's exits are the reverse index
//! of LeadsFrom. Both cascade DespawnSources, so destroying a room takes its
//! outgoing and incoming exits with it, leaving nothing dangling. Sources (exits)
//! and targets (rooms) are disjoint kinds, so neither relation can cycle and the
//! room graph is free to loop. Modeled on containment.rs. See
//! docs/architecture/ecs-and-relations.md.

use crate::component::Label;
use crate::id::EntityId;
use crate::relation::{Cascade, Relation};
use crate::world::World;

/// An exit's origin: source = the exit, target = the room it leads out of. A
/// room's exit list is this relation's reverse index.
pub struct LeadsFrom;

impl Relation for LeadsFrom {
    const ACYCLIC: bool = false;
    const ON_TARGET_DESPAWN: Cascade = Cascade::DespawnSources;
    const TARGET_TAG: &'static str = "leads_from";
}

/// An exit's destination: source = the exit, target = the room it leads into.
pub struct LeadsTo;

impl Relation for LeadsTo {
    const ACYCLIC: bool = false;
    const ON_TARGET_DESPAWN: Cascade = Cascade::DespawnSources;
    const TARGET_TAG: &'static str = "leads_to";
}

impl World {
    /// The exits leading out of a room (the reverse index of LeadsFrom).
    pub fn exits_of(&self, room: EntityId) -> Vec<EntityId> {
        self.sources_of::<LeadsFrom>(room)
    }

    /// The room an exit leads to, if its destination is still set.
    pub fn exit_destination(&self, exit: EntityId) -> Option<EntityId> {
        self.target_of::<LeadsTo>(exit)
    }

    /// An entity's label token, if it has one. General (reads the Label
    /// component); exits are the first user.
    pub fn label_of(&self, entity: EntityId) -> Option<String> {
        self.entity(entity)?.get::<&Label>().map(|l| l.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{Exit, Label, Room};
    use hecs::EntityBuilder;

    fn room(w: &mut World) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Room);
        w.spawn(b)
    }

    /// Spawn an exit entity from `from` to `to`, wired both relations.
    fn exit(w: &mut World, from: EntityId, to: EntityId, label: &str) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Exit);
        b.add(Label(label.into()));
        let e = w.spawn(b);
        w.relate::<LeadsFrom>(e, from).unwrap();
        w.relate::<LeadsTo>(e, to).unwrap();
        e
    }

    #[test]
    fn exits_are_reverse_indexed_and_readable() {
        let mut w = World::new();
        let hall = room(&mut w);
        let garden = room(&mut w);
        let north = exit(&mut w, hall, garden, "north");

        assert_eq!(w.exits_of(hall), vec![north]);
        assert_eq!(w.exit_destination(north), Some(garden));
        assert_eq!(w.label_of(north), Some("north".to_string()));
    }

    #[test]
    fn destroying_a_room_takes_its_incoming_and_outgoing_exits() {
        let mut w = World::new();
        let hall = room(&mut w);
        let garden = room(&mut w);
        let out = exit(&mut w, hall, garden, "north"); // hall leads to garden
        let back = exit(&mut w, garden, hall, "south"); // garden leads to hall

        w.despawn(hall);

        // The outgoing exit (LeadsFrom hall) and the incoming exit (LeadsTo hall)
        // are both gone; the surviving room has no dangling exits left.
        assert!(w.entity(out).is_none());
        assert!(w.entity(back).is_none());
        assert!(w.entity(garden).is_some());
        assert_eq!(w.exits_of(garden), Vec::<EntityId>::new());
    }
}
