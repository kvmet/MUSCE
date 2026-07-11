//! Observation verbs: describing the room and looking closely at the things in
//! it. Pure output, mutating nothing. The prose helpers (`describe_room`,
//! `description`) live here, the layer that renders the world to a player.

use musce_action::Ctx;
use musce_core::{Description, EntityId, World};
use musce_proto::EventKind;

use crate::kinds::Container;
use crate::names::{self, display_name, short_name};

/// `look`: describe the actor's current room, its exits, and its contents. With
/// an argument (`look <target>`) it looks closely at one thing, the same as
/// `examine`.
pub fn look(ctx: &mut Ctx, args: &str) {
    if !args.trim().is_empty() {
        examine(ctx, args);
        return;
    }
    match describe_room(ctx.world, ctx.actor) {
        Some(text) => ctx.emit_self(EventKind::Narration, text),
        None => ctx.emit_self(EventKind::Feedback, "You are nowhere."),
    }
}

/// `examine <target>` / `x`: look closely at a nearby thing (an item, a creature,
/// an exit, or yourself, addressed as `me`). Reveals its `Description`; a thing
/// carrying only a name gets a plain acknowledgement.
pub fn examine(ctx: &mut Ctx, args: &str) {
    let query = args.trim();
    if query.is_empty() {
        ctx.emit_self(EventKind::Feedback, "Examine what?");
        return;
    }
    let Some(target) = names::resolve_nearby(ctx.world, ctx.actor, query) else {
        ctx.emit_self(EventKind::Feedback, "You don't see that here.");
        return;
    };
    let mut text = match description(ctx.world, target) {
        Some(text) => text,
        None => format!(
            "You see nothing special about {}.",
            display_name(ctx.world, target)
        ),
    };
    // A container reveals what is inside it when looked at closely, so `put` is not
    // a black hole; a container kind with no contents reads as empty.
    if ctx.world.has::<Container>(target) {
        text.push('\n');
        text.push_str(&contents_line(ctx.world, target));
    }
    ctx.emit_self(EventKind::Narration, text);
}

/// A one-line inventory of a container's contents for `examine`: "It contains: a,
/// b." or "It is empty." when nothing inside carries a nameable handle.
fn contents_line(world: &World, container: EntityId) -> String {
    let items: Vec<String> = world
        .contents(container)
        .into_iter()
        .filter_map(|e| short_name(world, e))
        .collect();
    if items.is_empty() {
        "It is empty.".to_string()
    } else {
        format!("It contains: {}.", items.join(", "))
    }
}

/// `inventory` / `i`: list what the actor is holding.
pub fn inventory(ctx: &mut Ctx, _args: &str) {
    let items: Vec<String> = ctx
        .world
        .contents(ctx.actor)
        .into_iter()
        .filter_map(|e| short_name(ctx.world, e))
        .collect();
    if items.is_empty() {
        ctx.emit_self(EventKind::Feedback, "You are carrying nothing.");
    } else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You are carrying: {}.", items.join(", ")),
        );
    }
}

/// Build a room's look text: its description, its exits, and the other things in
/// it. Shared by `look` and the auto-look on arrival. `None` if the viewer is not
/// in a room.
fn describe_room(world: &World, viewer: EntityId) -> Option<String> {
    let room = world.enclosing_room(viewer)?;

    let mut s = description_or(world, room, "An indistinct space.");

    s.push_str("\nExits: ");
    let dirs: Vec<String> = world
        .exits_of(room)
        .into_iter()
        .filter_map(|e| world.name_of(e))
        .collect();
    if dirs.is_empty() {
        s.push_str("none");
    } else {
        s.push_str(&dirs.join(", "));
    }
    s.push('.');

    let others: Vec<String> = world
        .contents(room)
        .into_iter()
        .filter(|&e| e != viewer)
        .filter_map(|e| short_name(world, e))
        .collect();
    if !others.is_empty() {
        s.push_str("\nYou see: ");
        s.push_str(&others.join(", "));
        s.push('.');
    }

    Some(s)
}

fn description(world: &World, entity: EntityId) -> Option<String> {
    world
        .entity(entity)?
        .get::<&Description>()
        .map(|d| d.0.clone())
}

fn description_or(world: &World, entity: EntityId, fallback: &str) -> String {
    description(world, entity).unwrap_or_else(|| fallback.to_string())
}
