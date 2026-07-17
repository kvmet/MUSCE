//! Object manipulation: taking movable things up off the floor and putting them
//! back down. The takeable rule is game policy, kept in the `take` affordance's
//! guard (read through the shared `RefWorldModel`), not in `execute`.

use musce::action::{Action, Ctx, Frame, execute};
use musce::wire::EventKind;
use musce::world::{EntityId, World};

use crate::commit_or_log;
use crate::names::{self, Scope, display_name};

/// The outcome of the grounded `take`: the item was picked up, or the rule
/// refused it (with the reason a player should hear). Mirrors [`MoveOutcome`]:
/// the veto is structural game policy, decided here once, so the player verb and
/// a planned agent action share it.
///
/// [`MoveOutcome`]: super::movement::MoveOutcome
pub(crate) enum TakeOutcome {
    Took,
    Refused(&'static str),
}

/// Pick `item` up into `actor`'s hands, subject to the takeable rule. The
/// grounded action a player's `take` verb and an agent's plan both resolve to,
/// so a scripted actor is vetoed exactly as a player is. `Ctx`-free and silent;
/// the caller narrates.
pub(crate) fn do_take(world: &mut World, actor: EntityId, item: EntityId) -> TakeOutcome {
    // The takeable veto is the `take` affordance's guard, read through the same
    // `RefWorldModel` the planner reads, so a scripted take is filtered exactly
    // as a typed one. The frame binds the item to the `object` role the guard names.
    let frame = Frame {
        actor,
        object: Some(item),
        target: None,
        kind: None,
    };
    if let Some(reason) = crate::agency::take().veto(&frame, world, &crate::agency::RefWorldModel) {
        return TakeOutcome::Refused(reason);
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
        Ok(_) => TakeOutcome::Took,
        Err(_) => TakeOutcome::Refused("You can't take that."),
    }
}

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

    let name = display_name(ctx.world, target);
    let who = display_name(ctx.world, ctx.actor);
    let room = ctx.world.enclosing_locus(ctx.actor);

    match do_take(ctx.world, ctx.actor, target) {
        TakeOutcome::Refused(reason) => ctx.emit_self(EventKind::Feedback, reason),
        TakeOutcome::Took => {
            ctx.emit_self(EventKind::Feedback, format!("You take {name}."));
            if let Some(room) = room {
                ctx.emit_locus_except_self(
                    room,
                    EventKind::Narration,
                    format!("{who} takes {name}."),
                );
            }
        }
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
    let Some(room) = ctx.world.enclosing_locus(ctx.actor) else {
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
    ctx.emit_locus_except_self(room, EventKind::Narration, format!("{who} drops {name}."));
}
