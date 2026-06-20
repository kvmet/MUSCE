//! Name resolution: turn a player's typed noun into an `EntityId`. This is
//! opinionated, English-leaning game policy over the world queries the engine
//! exposes, so it lives in the game, not the engine. A typed noun matches a
//! `Label` first (exact, then prefix), then falls back to a case-insensitive
//! `Description` substring, over what the actor can plausibly refer to: the
//! things in their hands, the things in their room, or the exits leading out of
//! it. A compass direction is just a common label. First match wins.

use musce_core::{Description, EntityId, World};

/// Where to look for a named thing, relative to the actor.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Things the actor is holding (e.g. for `drop`).
    Inventory,
    /// Things on the floor of the actor's room (e.g. for `take`).
    Room,
    /// The exits leading out of the actor's room (e.g. for `go`).
    Exits,
}

/// Resolve `query` to a single entity in the given scope, or `None` on no match.
/// The actor itself is never a match (you don't `take` yourself).
pub fn resolve(world: &World, actor: EntityId, scope: Scope, query: &str) -> Option<EntityId> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }

    let candidates: Vec<EntityId> = match scope {
        Scope::Inventory => world.contents(actor),
        Scope::Room => world
            .enclosing_room(actor)
            .map(|r| world.contents(r))
            .unwrap_or_default(),
        Scope::Exits => world
            .enclosing_room(actor)
            .map(|r| world.exits_of(r))
            .unwrap_or_default(),
    };

    // Read each candidate's label once (lowercased); the actor is never a match.
    let labeled: Vec<(EntityId, Option<String>)> = candidates
        .into_iter()
        .filter(|&e| e != actor)
        .map(|e| (e, world.label_of(e).map(|l| l.to_lowercase())))
        .collect();

    // Tier 1: exact label. Tier 2: label prefix. Tier 3: description substring.
    let by_label = labeled
        .iter()
        .find(|(_, l)| l.as_deref() == Some(needle.as_str()))
        .or_else(|| {
            labeled
                .iter()
                .find(|(_, l)| l.as_deref().is_some_and(|l| l.starts_with(needle.as_str())))
        });
    if let Some(&(e, _)) = by_label {
        return Some(e);
    }
    labeled
        .iter()
        .find(|(e, _)| description_contains(world, *e, &needle))
        .map(|&(e, _)| e)
}

fn description_contains(world: &World, entity: EntityId, needle: &str) -> bool {
    world
        .entity(entity)
        .and_then(|er| {
            er.get::<&Description>()
                .map(|d| d.0.to_lowercase().contains(needle))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Exit, Item, Label, LeadsFrom, LeadsTo, Player, Room};

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
        let room = spawn(
            &mut w,
            described(
                |b| {
                    b.add(Room);
                },
                "a hall",
            ),
        );
        let actor = spawn(
            &mut w,
            described(
                |b| {
                    b.add(Player);
                },
                "an adventurer",
            ),
        );
        w.move_entity(actor, room).unwrap();
        (w, room, actor)
    }

    #[test]
    fn finds_item_in_room() {
        let (mut w, room, actor) = world_with_actor();
        let key = spawn(
            &mut w,
            described(
                |b| {
                    b.add(Item);
                },
                "a brass key",
            ),
        );
        w.move_entity(key, room).unwrap();

        assert_eq!(resolve(&w, actor, Scope::Room, "brass"), Some(key));
        assert_eq!(resolve(&w, actor, Scope::Room, "key"), Some(key));
    }

    #[test]
    fn finds_item_in_inventory() {
        let (mut w, _room, actor) = world_with_actor();
        let coin = spawn(
            &mut w,
            described(
                |b| {
                    b.add(Item);
                },
                "a gold coin",
            ),
        );
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

    #[test]
    fn resolves_exits_by_label_and_items_by_description() {
        let (mut w, room, actor) = world_with_actor();

        // An exit labeled "north" leading out of the actor's room.
        let dest = spawn(
            &mut w,
            described(
                |b| {
                    b.add(Room);
                },
                "a garden",
            ),
        );
        let exit = {
            let mut b = EntityBuilder::new();
            b.add(Exit);
            b.add(Label("north".into()));
            spawn(&mut w, b)
        };
        w.relate::<LeadsFrom>(exit, room).unwrap();
        w.relate::<LeadsTo>(exit, dest).unwrap();

        // Exact label and a unique prefix both resolve the exit.
        assert_eq!(resolve(&w, actor, Scope::Exits, "north"), Some(exit));
        assert_eq!(resolve(&w, actor, Scope::Exits, "n"), Some(exit));

        // An item with only a description still resolves by substring in the room.
        let key = spawn(
            &mut w,
            described(
                |b| {
                    b.add(Item);
                },
                "a brass key",
            ),
        );
        w.move_entity(key, room).unwrap();
        assert_eq!(resolve(&w, actor, Scope::Room, "brass"), Some(key));
    }
}
