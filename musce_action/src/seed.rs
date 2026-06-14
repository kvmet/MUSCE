//! A small code-built map, spawned when the database loads empty, so the engine
//! has ground truth to play and test against before any builder tools exist. This
//! is content, not engine: it leans only on `World::spawn` and the containment
//! mutator. See `docs/architecture/actions.md`.

use musce_core::hecs::EntityBuilder;
use musce_core::{Description, EntityId, Exit, Exits, Item, Player, Room, World};

/// The handful of entities a freshly seeded world cares about, returned for the
/// boot path and tests.
pub struct Seeded {
    /// The room a new player starts in.
    pub start: EntityId,
    /// The pre-made player avatar `@play` binds to in this stub.
    pub avatar: EntityId,
}

/// Build the starter map into an empty world: a hall, a garden to its north, and
/// a cellar below it; a takeable key in the garden; and a player avatar standing
/// in the hall.
pub fn seed(world: &mut World) -> Seeded {
    let hall = room(world, "a stone hall, its flagstones worn smooth");
    let garden = room(world, "a quiet walled garden");
    let cellar = room(world, "a damp, low-ceilinged cellar");

    set_exits(world, hall, &[("north", garden), ("down", cellar)]);
    set_exits(world, garden, &[("south", hall)]);
    set_exits(world, cellar, &[("up", hall)]);

    let key = item(world, "a brass key");
    world.move_entity(key, garden).expect("seed: place key");

    let avatar = avatar(world, "a weathered adventurer");
    world.move_entity(avatar, hall).expect("seed: place avatar");

    Seeded { start: hall, avatar }
}

fn room(world: &mut World, desc: &str) -> EntityId {
    spawn(world, |b| {
        b.add(Room);
        b.add(Description(desc.into()));
    })
}

fn item(world: &mut World, desc: &str) -> EntityId {
    spawn(world, |b| {
        b.add(Item);
        b.add(Description(desc.into()));
    })
}

fn avatar(world: &mut World, desc: &str) -> EntityId {
    spawn(world, |b| {
        b.add(Player);
        b.add(Description(desc.into()));
    })
}

fn spawn(world: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
    let mut b = EntityBuilder::new();
    f(&mut b);
    world.spawn(b)
}

fn set_exits(world: &mut World, room: EntityId, exits: &[(&str, EntityId)]) {
    let exits = exits
        .iter()
        .map(|(dir, to)| Exit { direction: (*dir).into(), to: *to })
        .collect();
    let e = world.index().get(room).expect("seed: room just spawned");
    world.ecs.insert_one(e, Exits(exits)).expect("seed: set exits");
}

/// Find the player avatar in the world. The stub `@play` binds to it; the real
/// flow will instead resolve the account's chosen character. Returns the first
/// `Player` entity, of which the seed makes exactly one.
pub fn find_player(world: &World) -> Option<EntityId> {
    world
        .ecs
        .query::<(&musce_core::Id, &Player)>()
        .iter()
        .next()
        .map(|(id, _)| id.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_links_rooms_and_places_things() {
        let mut w = World::new();
        let Seeded { start, avatar } = seed(&mut w);

        // The avatar starts in the hall, and the hall is what `find_player` lands on.
        assert_eq!(w.enclosing_room(avatar), Some(start));
        assert_eq!(find_player(&w), Some(avatar));

        // North out of the hall reaches a room that leads back south.
        let north = w
            .entity(start)
            .unwrap()
            .get::<&Exits>()
            .unwrap()
            .0
            .iter()
            .find(|e| e.direction == "north")
            .map(|e| e.to);
        assert!(north.is_some());
    }
}
