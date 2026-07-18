//! Object manipulation: taking movable things up off the floor and putting them
//! back down. The takeable rule is game policy, kept in the `take` affordance's
//! guard (read through the shared `RefWorldModel`), not in `execute`.

use musce::action::{Action, Ctx, Frame, execute};
use musce::wire::EventKind;
use musce::world::{EntityId, World};

use crate::agency::RefWorldModel;
use crate::names::{self, Scope};
use crate::verbs::Outcome;

/// Pick `item` up into `actor`'s hands, subject to the takeable rule. The
/// grounded action a player's `take` verb and an agent's plan both resolve to,
/// so a scripted actor is vetoed exactly as a player is. `Ctx`-free and silent;
/// the caller narrates.
pub(crate) fn do_take(world: &mut World, actor: EntityId, item: EntityId) -> Outcome {
    // The takeable veto is the `take` affordance's guard, read through the same
    // `RefWorldModel` the planner reads, so a scripted take is filtered exactly
    // as a typed one. The frame binds the item to the `object` role the guard names.
    let frame = Frame {
        actor,
        object: Some(item),
        target: None,
        kind: None,
    };
    if let Some(guard) = crate::agency::take().veto(&frame, world, &RefWorldModel) {
        return Outcome::Refused(guard.reason);
    }
    // The one structural way this fails is taking a container the actor stands
    // inside (a containment cycle); the executor rejects it and "you can't take
    // that" is the right thing for the player to hear.
    match execute(
        world,
        Action::Move {
            entity: item,
            into: actor,
        },
    ) {
        Ok(_) => Outcome::Committed,
        Err(_) => Outcome::Refused("You can't take that."),
    }
}

/// Put `item` down into the actor's current room, subject to the held rule. The
/// grounded action a player's `drop` verb and an agent's plan both resolve to.
/// The destination is not a choice: `drop` always means "into the room I stand
/// in", so it is derived here rather than passed. `Ctx`-free and silent; the
/// caller narrates.
pub(crate) fn do_drop(world: &mut World, actor: EntityId, item: EntityId) -> Outcome {
    let Some(room) = world.enclosing_locus(actor) else {
        return Outcome::Refused("There is nowhere to drop it.");
    };
    // The held veto is the `drop` affordance's guard (`object contained_by
    // actor`), read through the same `RefWorldModel` the planner reads. `target`
    // is bound to the room the effect names, though the guard reads only `object`.
    let frame = Frame {
        actor,
        object: Some(item),
        target: Some(room),
        kind: None,
    };
    if let Some(guard) = crate::agency::drop().veto(&frame, world, &RefWorldModel) {
        return Outcome::Refused(guard.reason);
    }
    // Dropping a held item into its enclosing room cannot cycle, so this commits;
    // a structural error here would be a bug, surfaced as a refusal to the player.
    match execute(
        world,
        Action::Move {
            entity: item,
            into: room,
        },
    ) {
        Ok(_) => Outcome::Committed,
        Err(_) => Outcome::Refused("You can't drop that."),
    }
}

/// `take <item>`: pick a reachable thing up off the floor into the actor's hands.
/// The verb owns the parse and the room-scoped name resolution; the act and its
/// narration are the shared [`crate::act::perform_narrated`], so a typed take, a
/// clicked one, and a planned one commit and narrate alike.
pub fn take(ctx: &mut Ctx, args: &str) {
    if args.trim().is_empty() {
        ctx.emit_self(EventKind::Feedback, "Take what?");
        return;
    }
    let Some(target) = names::resolve(ctx.world, ctx.actor, Scope::Room, args) else {
        ctx.emit_self(EventKind::Feedback, "You don't see that here.");
        return;
    };

    let actor = ctx.actor;
    let frame = Frame {
        actor,
        object: Some(target),
        target: None,
        kind: None,
    };
    let verdict = ctx.verdict();
    let (world, out) = ctx.world_and_out();
    crate::act::perform_narrated(world, actor, &crate::agency::take(), &frame, verdict, out);
}

/// `drop <item>`: put a held thing down into the current room. The verb owns the
/// parse and the inventory-scoped resolution; the act and its narration are the
/// shared [`crate::act::perform_narrated`]. `drop` derives its destination (the
/// actor's room) inside the act, so the frame binds only the object.
pub fn drop(ctx: &mut Ctx, args: &str) {
    if args.trim().is_empty() {
        ctx.emit_self(EventKind::Feedback, "Drop what?");
        return;
    }
    let Some(target) = names::resolve(ctx.world, ctx.actor, Scope::Inventory, args) else {
        ctx.emit_self(EventKind::Feedback, "You aren't carrying that.");
        return;
    };

    let actor = ctx.actor;
    let frame = Frame {
        actor,
        object: Some(target),
        target: None,
        kind: None,
    };
    let verdict = ctx.verdict();
    let (world, out) = ctx.world_and_out();
    crate::act::perform_narrated(world, actor, &crate::agency::drop(), &frame, verdict, out);
}
