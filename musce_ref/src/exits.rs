//! Exit connectivity: the reference game's room graph. An exit is an intermediate
//! entity with one origin (`LeadsFrom`) and one destination (`LeadsTo`); a locus's
//! exits are the reverse index of `LeadsFrom`. Both relations cascade
//! `DespawnSources`, so destroying a room takes its outgoing and incoming exits
//! with it, leaving nothing dangling.
//!
//! This is game vocabulary, not engine machinery: the engine owns containment, the
//! generic relation layer, and the `Locus` scope boundary, but never reads exit
//! connectivity. So the relations are defined here over the public `Relation` API
//! and registered through the game's `register` hook, exactly like the kind
//! markers. See `docs/architecture/ecs-and-relations.md`.
//!
//! Define AND register the relations together (here). `World::relate` succeeds on
//! an *unregistered* relation, but registration is what wires serialization, the
//! despawn cascade, and rebuild-on-load; splitting the two would silently drop
//! exits from saves and leave dangling exits on room despawn. Keeping both in this
//! one module makes that impossible to get wrong.

use musce::world::{Cascade, EntityId, Relation, World};

/// An exit's origin: source = the exit, target = the locus it leads out of. A
/// locus's exit list is this relation's reverse index.
pub(crate) struct LeadsFrom;

impl Relation for LeadsFrom {
    const ACYCLIC: bool = false;
    const ON_TARGET_DESPAWN: Cascade = Cascade::DespawnSources;
    const TARGET_TAG: &'static str = "leads_from";
}

/// An exit's destination: source = the exit, target = the locus it leads into.
pub(crate) struct LeadsTo;

impl Relation for LeadsTo {
    const ACYCLIC: bool = false;
    const ON_TARGET_DESPAWN: Cascade = Cascade::DespawnSources;
    const TARGET_TAG: &'static str = "leads_to";
}

/// Register the exit relations so they serialize, cascade, and rebuild on load.
/// Called from the game's `register` hook, before any world loads or seeds.
pub(crate) fn register(world: &mut World) {
    world.register_relation::<LeadsFrom>();
    world.register_relation::<LeadsTo>();
}

/// Exit-graph queries over the world the engine already exposes. An extension
/// trait so call sites keep reading `world.exits_of(..)`.
pub(crate) trait ExitQueries {
    /// The exits leading out of a locus (the reverse index of `LeadsFrom`).
    fn exits_of(&self, locus: EntityId) -> Vec<EntityId>;
    /// The locus an exit leads to, if its destination is still set.
    fn exit_destination(&self, exit: EntityId) -> Option<EntityId>;
}

impl ExitQueries for World {
    fn exits_of(&self, locus: EntityId) -> Vec<EntityId> {
        self.sources_of::<LeadsFrom>(locus)
    }

    fn exit_destination(&self, exit: EntityId) -> Option<EntityId> {
        self.target_of::<LeadsTo>(exit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::Exit;
    use musce::world::hecs::EntityBuilder;
    use musce::world::{DestroyCause, Fact, Locus, Name};

    fn room(w: &mut World) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Locus);
        w.spawn(b)
    }

    /// Spawn an exit entity from `from` to `to`, wired both relations.
    fn exit(w: &mut World, from: EntityId, to: EntityId, name: &str) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Exit);
        b.add(Name(name.into()));
        let e = w.spawn(b);
        w.relate::<LeadsFrom>(e, from).unwrap();
        w.relate::<LeadsTo>(e, to).unwrap();
        e
    }

    #[test]
    fn exits_are_reverse_indexed_and_readable() {
        let mut w = World::new();
        register(&mut w);
        let hall = {
            let mut b = EntityBuilder::new();
            b.add(Locus);
            w.spawn(b)
        };
        let garden = {
            let mut b = EntityBuilder::new();
            b.add(Locus);
            w.spawn(b)
        };
        let north = exit(&mut w, hall, garden, "north");

        assert_eq!(w.exits_of(hall), vec![north]);
        assert_eq!(w.exit_destination(north), Some(garden));
        assert_eq!(w.name_of(north), Some("north".to_string()));
    }

    #[test]
    fn destroying_a_room_takes_its_incoming_and_outgoing_exits() {
        let mut w = World::new();
        register(&mut w);
        let hall = {
            let mut b = EntityBuilder::new();
            b.add(Locus);
            w.spawn(b)
        };
        let garden = {
            let mut b = EntityBuilder::new();
            b.add(Locus);
            w.spawn(b)
        };
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

    #[test]
    fn despawn_room_with_exits_emits_direct_and_cascade_facts() {
        let mut w = World::new();
        register(&mut w);
        let hall = room(&mut w);
        let garden = room(&mut w);
        let north = exit(&mut w, hall, garden, "north");

        w.despawn(hall);
        let facts = w.take_facts();

        // Order rests on reverse-index order and is not guaranteed; assert by set.
        let room_fact = facts
            .iter()
            .find(|f| matches!(f, Fact::Destroyed { entity, .. } if *entity == hall))
            .expect("a fact for the room");
        assert!(matches!(
            room_fact,
            Fact::Destroyed {
                cause: DestroyCause::Direct,
                ..
            }
        ));
        let exit_fact = facts
            .iter()
            .find(|f| matches!(f, Fact::Destroyed { entity, .. } if *entity == north))
            .expect("a fact for the cascaded exit");
        assert!(matches!(
            exit_fact,
            Fact::Destroyed {
                cause: DestroyCause::Cascade,
                ..
            }
        ));
    }
}
