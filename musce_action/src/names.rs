//! Name resolution: turn a player's typed noun into an `EntityId`. For now the
//! `Description` doubles as the name (there is no `Name` component yet), matched
//! case-insensitively as a substring over what the actor can plausibly refer to:
//! the things in their hands and the things in their room. First match wins.

use musce_core::{Description, EntityId, World};

/// Where to look for a named thing, relative to the actor.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Things the actor is holding (e.g. for `drop`).
    Inventory,
    /// Things on the floor of the actor's room (e.g. for `take`).
    Room,
}

/// Resolve `query` to a single entity in the given scope, or `None` on no match.
/// The actor itself is never a match (you don't `take` yourself).
pub fn resolve(world: &World, actor: EntityId, scope: Scope, query: &str) -> Option<EntityId> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }

    let container = match scope {
        Scope::Inventory => Some(actor),
        Scope::Room => world.enclosing_room(actor),
    };

    container
        .into_iter()
        .flat_map(|c| world.contents(c))
        .filter(|&e| e != actor)
        .find(|&e| matches_name(world, e, &needle))
}

fn matches_name(world: &World, entity: EntityId, needle: &str) -> bool {
    world
        .entity(entity)
        .and_then(|er| er.get::<&Description>().map(|d| d.0.to_lowercase().contains(needle)))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Item, Player, Room};

    fn spawn(w: &mut World, builder: EntityBuilder) -> EntityId {
        w.spawn(builder)
    }

    fn described(marker: impl FnOnce(&mut EntityBuilder), name: &str) -> EntityBuilder {
        let mut b = EntityBuilder::new();
        marker(&mut b);
        b.add(Description(name.into()));
        b
    }

    fn world_with_actor() -> (World, EntityId, EntityId) {
        let mut w = World::new();
        let room = spawn(&mut w, described(|b| { b.add(Room); }, "a hall"));
        let actor = spawn(&mut w, described(|b| { b.add(Player); }, "an adventurer"));
        w.move_entity(actor, room).unwrap();
        (w, room, actor)
    }

    #[test]
    fn finds_item_in_room() {
        let (mut w, room, actor) = world_with_actor();
        let key = spawn(&mut w, described(|b| { b.add(Item); }, "a brass key"));
        w.move_entity(key, room).unwrap();

        assert_eq!(resolve(&w, actor, Scope::Room, "brass"), Some(key));
        assert_eq!(resolve(&w, actor, Scope::Room, "key"), Some(key));
    }

    #[test]
    fn finds_item_in_inventory() {
        let (mut w, _room, actor) = world_with_actor();
        let coin = spawn(&mut w, described(|b| { b.add(Item); }, "a gold coin"));
        w.move_entity(coin, actor).unwrap();

        assert_eq!(resolve(&w, actor, Scope::Inventory, "coin"), Some(coin));
        // Not in the room scope: it is held, not on the floor.
        assert_eq!(resolve(&w, actor, Scope::Room, "coin"), None);
    }

    #[test]
    fn miss_returns_none() {
        let (w, _room, actor) = world_with_actor();
        assert_eq!(resolve(&w, actor, Scope::Room, "dragon"), None);
    }
}
