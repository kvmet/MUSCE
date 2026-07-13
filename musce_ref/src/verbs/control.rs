//! Embodiment control: aiming the character's control cursor at a puppet it
//! controls, and releasing it back to itself. The rule (you may only pilot what
//! you control) is game policy, enforced structurally by `set_focus`.

use musce::action::Ctx;
use musce::wire::EventKind;

use crate::names::{self, Scope, display_name};

/// `pilot <thing>`: aim the character's control cursor at something it controls,
/// so bare commands drive that thing. The rule is game policy: you may only pilot
/// what you control. (Establishing control is the `@possess` admin verb; the seed
/// also wires one controllable thing for out-of-box play.)
pub fn pilot(ctx: &mut Ctx, args: &str) {
    if args.trim().is_empty() {
        ctx.emit_self(EventKind::Feedback, "Pilot what?");
        return;
    }
    let character = ctx.world.control_root(ctx.actor);
    let Some(target) = names::resolve(ctx.world, ctx.actor, Scope::Room, args) else {
        ctx.emit_self(EventKind::Feedback, "You don't see that here.");
        return;
    };

    let name = display_name(ctx.world, target);
    let who = display_name(ctx.world, character);
    let room = ctx.world.enclosing_locus(character);

    // The control rule lives in `set_focus`: the cursor may only land on something
    // the character controls (transitively, so deep chains pilot too). A reject is
    // "you don't control that", surfaced to the player.
    if ctx.world.set_focus(character, target).is_err() {
        ctx.emit_self(EventKind::Feedback, "You can't pilot that.");
        return;
    }

    ctx.emit_self(EventKind::Feedback, format!("You take control of {name}."));
    if let Some(room) = room {
        ctx.emit_locus_except_self(
            room,
            EventKind::Narration,
            format!("{who} goes still, eyes distant."),
        );
    }
}

/// `release`: drop the character's cursor back to itself, so bare commands drive
/// you again. Tears down no `Controls` edge, so you can step back in.
pub fn release(ctx: &mut Ctx, _args: &str) {
    let character = ctx.world.control_root(ctx.actor);
    let Some(piloted) = ctx.world.focus_of(character) else {
        ctx.emit_self(EventKind::Feedback, "You aren't piloting anything.");
        return;
    };

    let name = display_name(ctx.world, piloted);
    let who = display_name(ctx.world, character);
    let room = ctx.world.enclosing_locus(character);

    ctx.world.clear_focus(character);

    ctx.emit_self(
        EventKind::Feedback,
        format!("You release {name} and return to yourself."),
    );
    if let Some(room) = room {
        ctx.emit_locus_except_self(
            room,
            EventKind::Narration,
            format!("{who} stirs and looks around."),
        );
    }
}
