//! Object manipulation: taking movable things up off the floor and putting them
//! back down. The takeable rule is game policy, kept in the handler, not in
//! `execute`.

use musce_action::{Action, Ctx, execute};
use musce_core::{EntityId, World};
use musce_proto::EventKind;

use crate::commit_or_log;
use crate::names::{self, Scope, display_name};

/// `take <item>`: pick a reachable thing up off the floor into the actor's hands.
pub fn take(ctx: &mut Ctx, args: &str) {
    if args.trim().is_empty() {
        ctx.emit_self(EventKind::Feedback, "Take what?");
        return;
    }
    let Some(target) = names::resolve(ctx.world, ctx.actor, Scope::Room, args) else {
        ctx.emit_self(EventKind::Feedback, "You don't see that here.");
        return;
    };
    if !is_takeable(ctx.world, target) {
        ctx.emit_self(EventKind::Feedback, "You can't take that.");
        return;
    }

    let name = display_name(ctx.world, target);
    let who = display_name(ctx.world, ctx.actor);
    let room = ctx.world.enclosing_room(ctx.actor);

    // The one structural way this fails is taking a container the actor stands
    // inside (a containment cycle); the executor rejects it and "you can't take
    // that" is the right thing for the player to hear.
    if execute(
        ctx.world,
        Action::Move {
            entity: target,
            into: ctx.actor,
        },
    )
    .is_err()
    {
        ctx.emit_self(EventKind::Feedback, "You can't take that.");
        return;
    }

    ctx.emit_self(EventKind::Feedback, format!("You take {name}."));
    if let Some(room) = room {
        ctx.emit_room_except_self(room, EventKind::Narration, format!("{who} takes {name}."));
    }
}

/// `drop <item>`: put a held thing down into the current room.
pub fn drop(ctx: &mut Ctx, args: &str) {
    if args.trim().is_empty() {
        ctx.emit_self(EventKind::Feedback, "Drop what?");
        return;
    }
    let Some(target) = names::resolve(ctx.world, ctx.actor, Scope::Inventory, args) else {
        ctx.emit_self(EventKind::Feedback, "You aren't carrying that.");
        return;
    };
    let Some(room) = ctx.world.enclosing_room(ctx.actor) else {
        ctx.emit_self(EventKind::Feedback, "There is nowhere to drop it.");
        return;
    };

    let name = display_name(ctx.world, target);
    let who = display_name(ctx.world, ctx.actor);

    // Dropping a held item into its enclosing room cannot cycle, so this should
    // never fail; a bug here is logged loud, not silently shown as a refusal.
    if !commit_or_log(
        ctx.world,
        Action::Move {
            entity: target,
            into: room,
        },
        "drop: move held item into the room",
    ) {
        ctx.emit_self(EventKind::Feedback, "You can't drop that.");
        return;
    }

    ctx.emit_self(EventKind::Feedback, format!("You drop {name}."));
    ctx.emit_room_except_self(room, EventKind::Narration, format!("{who} drops {name}."));
}

/// Takeable means a movable object, not a fixture or a being: rooms and players
/// and creatures stay put. This is the gameplay rule, kept here in the handler,
/// not in `execute`.
fn is_takeable(world: &World, entity: EntityId) -> bool {
    use crate::kinds::{Creature, Player};
    use musce_core::Room;
    !(world.has::<Room>(entity) || world.has::<Player>(entity) || world.has::<Creature>(entity))
}
