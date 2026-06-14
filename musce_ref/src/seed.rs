//! The reference game's starting world and its `@play` actor policy. The seed is
//! spawned into an empty database on first boot so there is ground truth to play
//! and test against before any builder tools exist; `choose_actor` is the policy
//! the runtime injects for `@play`, choosing which actor a connection comes to
//! drive. Both are game content over the world API the engine exposes. See
//! `docs/architecture/engine-and-game.md`.

use musce_core::hecs::EntityBuilder;
use musce_core::{Description, EntityId, Exit, Exits, Item, Player, Room, World};

/// Build the starter map into an empty world: a hall, a garden to its north, and
/// a cellar below it; a takeable key in the garden; and a player avatar standing
/// in the hall. Matches the `fn(&mut World)` shape the runtime's `Game.seed`
/// expects.
pub fn seed(world: &mut World) {
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
}

/// The `@play` policy: choose which actor a connection comes to drive. The floor
/// records the attachment; this only selects. For now that is the seeded player
/// avatar; the real flow will resolve the account's chosen character.
pub fn choose_actor(world: &World) -> Option<EntityId> {
    find_player(world)
}

/// Find the player avatar in the world. Returns the first `Player` entity, of
/// which the seed makes exactly one. The real flow will instead resolve the
/// account's chosen character.
fn find_player(world: &World) -> Option<EntityId> {
    world
        .ecs
        .query::<(&musce_core::Id, &Player)>()
        .iter()
        .next()
        .map(|(id, _)| id.0)
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
        .map(|(dir, to)| Exit {
            direction: (*dir).into(),
            to: *to,
        })
        .collect();
    let e = world.index().get(room).expect("seed: room just spawned");
    world
        .ecs
        .insert_one(e, Exits(exits))
        .expect("seed: set exits");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_links_rooms_and_places_things() {
        let mut w = World::new();
        seed(&mut w);

        // The seed makes exactly one player avatar, standing in a room.
        let avatar = find_player(&w).expect("seed places a player");
        let start = w.enclosing_room(avatar).expect("avatar is in a room");

        // North out of the start room reaches a room that leads back south.
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

    #[test]
    fn choose_actor_selects_the_seeded_avatar() {
        let mut w = World::new();
        seed(&mut w);
        assert_eq!(choose_actor(&w), find_player(&w));
        assert!(choose_actor(&w).is_some());
    }
}
