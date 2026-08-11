//! Object manipulation commands. Parsing and name resolution live here; the
//! app-owned canonical affordances own rules, mutation, and narration.

use musce::action::Ctx;
use musce::wire::EventKind;

use crate::names::{self, Scope};

/// `take <item>`: pick a reachable thing up off the floor into the actor's hands.
/// The verb owns the parse and the room-scoped name resolution; the act and its
/// resulting grounding is performed through the same canonical registry used by
/// pointing, systems, and agency.
pub fn take(ctx: &mut Ctx, args: &str) {
    if args.trim().is_empty() {
        ctx.emit_self(EventKind::Feedback, "Take what?");
        return;
    }
    let Some(target) = names::resolve(ctx.world, ctx.actor(), Scope::Room, args) else {
        ctx.emit_self(EventKind::Feedback, "You don't see that here.");
        return;
    };

    let action = crate::affordances::take_action(ctx.actor(), target);
    crate::affordances::perform_command(ctx, &action, "You can't take that.");
}

/// `drop <item>`: put a held thing down into the current room. The verb owns the
/// parse and inventory-scoped resolution. Its destination is explicit in the
/// canonical grounding even though text derives it from the actor's locus.
pub fn drop(ctx: &mut Ctx, args: &str) {
    if args.trim().is_empty() {
        ctx.emit_self(EventKind::Feedback, "Drop what?");
        return;
    }
    let Some(target) = names::resolve(ctx.world, ctx.actor(), Scope::Inventory, args) else {
        ctx.emit_self(EventKind::Feedback, "You aren't carrying that.");
        return;
    };
    let Some(destination) = ctx.world.enclosing_locus(ctx.actor()) else {
        ctx.emit_self(EventKind::Feedback, "There is nowhere to drop it.");
        return;
    };
    let action = crate::affordances::drop_action(ctx.actor(), target, destination);
    crate::affordances::perform_command(ctx, &action, "You can't drop that.");
}
