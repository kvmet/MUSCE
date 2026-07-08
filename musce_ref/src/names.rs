//! Name resolution: turn a player's typed noun into an `EntityId`. This is
//! opinionated, English-leaning game policy over the world queries the engine
//! exposes, so it lives in the game, not the engine. A typed noun matches an
//! entity's `Name` first (exact, then a whole- or per-word prefix so `key`
//! resolves `a brass key`), then its `Aliases` (extra keywords a builder hangs on
//! a thing), then falls back to a case-insensitive `Description` substring, over
//! what the actor can plausibly refer to: the things in their hands, the things
//! in their room, or the exits leading out of it. A compass direction is just a
//! common name. First match wins, higher tiers before lower.

use musce_core::{Description, EntityId, NamedComponent, World};
use serde::{Deserialize, Serialize};

/// Extra keywords a player may type to refer to an entity, beyond its `Name`
/// (e.g. `light` for a torch). Purely a resolver convenience with no engine
/// consumer, so it lives game-side and registers through [`register`]. Never
/// shown; `Name` is the displayed handle.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Aliases(pub Vec<String>);

impl NamedComponent for Aliases {
    const TAG: &'static str = "aliases";
}

/// Register the game-side resolver components so they persist and reload. Called
/// from the game's `register` hook.
pub fn register(world: &mut World) {
    world.register_component::<Aliases>();
}

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

    match_query(world, actor, &candidates, &needle)
}

/// Resolve `query` against everything the actor can refer to right now: itself
/// (typed as `me`/`self`), the things it holds, the things in its room, and the
/// exits out of it, in that order so a held thing wins a tie. Used by `examine`,
/// where the target may be any nearby thing rather than one fixed scope.
pub fn resolve_nearby(world: &World, actor: EntityId, query: &str) -> Option<EntityId> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    if matches!(needle.as_str(), "me" | "self" | "myself") {
        return Some(actor);
    }
    resolve(world, actor, Scope::Inventory, query)
        .or_else(|| resolve(world, actor, Scope::Room, query))
        .or_else(|| resolve(world, actor, Scope::Exits, query))
}

/// A candidate's match tokens, read once and lowercased. The actor is filtered
/// out upstream.
struct Card {
    id: EntityId,
    name: Option<String>,
    aliases: Vec<String>,
    desc: Option<String>,
}

fn card(world: &World, e: EntityId) -> Card {
    let name = world.name_of(e).map(|n| n.to_lowercase());
    let (aliases, desc) = world
        .entity(e)
        .map(|er| {
            let aliases = er
                .get::<&Aliases>()
                .map(|a| a.0.iter().map(|s| s.to_lowercase()).collect())
                .unwrap_or_default();
            let desc = er.get::<&Description>().map(|d| d.0.to_lowercase());
            (aliases, desc)
        })
        .unwrap_or_default();
    Card {
        id: e,
        name,
        aliases,
        desc,
    }
}

/// Match `needle` (already trimmed and lowercased) against `candidates` in tiers,
/// returning the first entity that hits the highest tier any candidate reaches:
/// an exact name/alias, then a name/alias prefix, then a description substring.
fn match_query(
    world: &World,
    actor: EntityId,
    candidates: &[EntityId],
    needle: &str,
) -> Option<EntityId> {
    let cards: Vec<Card> = candidates
        .iter()
        .copied()
        .filter(|&e| e != actor)
        .map(|e| card(world, e))
        .collect();

    // Tier 1: an exact name or an exact alias.
    if let Some(c) = cards
        .iter()
        .find(|c| c.name.as_deref() == Some(needle) || c.aliases.iter().any(|a| a == needle))
    {
        return Some(c.id);
    }
    // Tier 2: the name as a whole- or per-word prefix, or an alias prefix.
    if let Some(c) = cards.iter().find(|c| {
        c.name
            .as_deref()
            .is_some_and(|n| name_prefix_match(n, needle))
            || c.aliases.iter().any(|a| a.starts_with(needle))
    }) {
        return Some(c.id);
    }
    // Tier 3: a description substring (covers un-named quick-create content).
    cards
        .iter()
        .find(|c| c.desc.as_deref().is_some_and(|d| d.contains(needle)))
        .map(|c| c.id)
}

/// The short handle for an entity: its `Name`, else its `Description` (quick-
/// create content carries only the latter). `None` if it has neither, so callers
/// can skip a thing with no way to name it. The single naming policy the verb,
/// system, and reaction layers all narrate through, so a thing reads the same
/// whoever mentions it.
pub(crate) fn short_name(world: &World, entity: EntityId) -> Option<String> {
    world.name_of(entity).or_else(|| {
        world
            .entity(entity)
            .and_then(|er| er.get::<&Description>().map(|d| d.0.clone()))
    })
}

/// A name for narration: [`short_name`], falling back to a neutral noun for a
/// thing carrying neither a `Name` nor a `Description`.
pub(crate) fn display_name(world: &World, entity: EntityId) -> String {
    short_name(world, entity).unwrap_or_else(|| "something".to_string())
}

/// Whether `needle` is a prefix of the whole name or of any of its words, with
/// leading articles ignored so `key` matches `a brass key`.
fn name_prefix_match(name: &str, needle: &str) -> bool {
    name.starts_with(needle)
        || name
            .split_whitespace()
            .filter(|w| !matches!(*w, "a" | "an" | "the"))
            .any(|w| w.starts_with(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::{Exit, Item, Player};
    use musce_core::hecs::EntityBuilder;
    use musce_core::{LeadsFrom, LeadsTo, Name, Room};

    fn spawn(w: &mut World, builder: EntityBuilder) -> EntityId {
        w.spawn(builder)
    }

    fn described(marker: impl FnOnce(&mut EntityBuilder), desc: &str) -> EntityBuilder {
        let mut b = EntityBuilder::new();
        marker(&mut b);
        b.add(Description(desc.into()));
        b
    }

    /// A marker plus a `Name` (and no `Description`), for exercising the name
    /// tiers directly rather than the description-substring fallback.
    fn named(marker: impl FnOnce(&mut EntityBuilder), name: &str) -> EntityBuilder {
        let mut b = EntityBuilder::new();
        marker(&mut b);
        b.add(Name(name.into()));
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
    fn resolves_exits_by_name_and_items_by_description() {
        let (mut w, room, actor) = world_with_actor();

        // An exit named "north" leading out of the actor's room.
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
            b.add(Name("north".into()));
            spawn(&mut w, b)
        };
        w.relate::<LeadsFrom>(exit, room).unwrap();
        w.relate::<LeadsTo>(exit, dest).unwrap();

        // Exact name and a unique prefix both resolve the exit.
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

    #[test]
    fn matches_a_multi_word_name_by_any_word() {
        let (mut w, room, actor) = world_with_actor();
        // A Name, no Description: only the name tiers can resolve it.
        let key = spawn(
            &mut w,
            named(
                |b| {
                    b.add(Item);
                },
                "a brass key",
            ),
        );
        w.move_entity(key, room).unwrap();

        // Both words resolve it, and the leading article does not swallow the room.
        assert_eq!(resolve(&w, actor, Scope::Room, "key"), Some(key));
        assert_eq!(resolve(&w, actor, Scope::Room, "brass"), Some(key));
        assert_eq!(resolve(&w, actor, Scope::Room, "a brass key"), Some(key));
    }

    #[test]
    fn matches_an_alias_the_name_does_not_contain() {
        let (mut w, room, actor) = world_with_actor();
        let torch = {
            let mut b = named(
                |b| {
                    b.add(Item);
                },
                "a guttering torch",
            );
            b.add(Aliases(vec!["light".into(), "lamp".into()]));
            spawn(&mut w, b)
        };
        w.move_entity(torch, room).unwrap();

        // "light" appears in neither the name nor a description, only the aliases.
        assert_eq!(resolve(&w, actor, Scope::Room, "light"), Some(torch));
        assert_eq!(resolve(&w, actor, Scope::Room, "torch"), Some(torch));
    }

    #[test]
    fn resolve_nearby_finds_self_held_and_room() {
        let (mut w, room, actor) = world_with_actor();
        let held = spawn(
            &mut w,
            named(
                |b| {
                    b.add(Item);
                },
                "a worn map",
            ),
        );
        w.move_entity(held, actor).unwrap();
        let floor = spawn(
            &mut w,
            named(
                |b| {
                    b.add(Item);
                },
                "a mossy stone",
            ),
        );
        w.move_entity(floor, room).unwrap();

        assert_eq!(resolve_nearby(&w, actor, "me"), Some(actor));
        assert_eq!(resolve_nearby(&w, actor, "self"), Some(actor));
        assert_eq!(resolve_nearby(&w, actor, "map"), Some(held));
        assert_eq!(resolve_nearby(&w, actor, "stone"), Some(floor));
        assert_eq!(resolve_nearby(&w, actor, "dragon"), None);
    }
}
