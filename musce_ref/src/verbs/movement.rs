//! Movement command parsing. The app-owned `go` affordance is the only traversal
//! rule, mutation, and narration path for players, systems, and sequences.

use serde::{Deserialize, Serialize};

use musce::action::Ctx;
use musce::wire::EventKind;
use musce::world::NamedComponent;

use crate::exits::ExitQueries;
use crate::names::{self, Scope};

/// `go <dir>` / a bare direction: resolve the app's exit and destination, then
/// perform the canonical `go` action.
pub fn go(ctx: &mut Ctx, dir: &str) {
    let dir = dir.trim();
    if ctx.world.enclosing_locus(ctx.actor()).is_none() {
        ctx.emit_self(EventKind::Feedback, "You are nowhere.");
        return;
    }
    if dir.is_empty() {
        ctx.emit_self(EventKind::Feedback, "Go where?");
        return;
    }

    let Some(exit) = names::resolve(ctx.world, ctx.actor(), Scope::Exits, dir) else {
        ctx.emit_self(EventKind::Feedback, "You can't go that way.");
        return;
    };

    let Some(destination) = ctx.world.exit_destination(exit) else {
        ctx.emit_self(EventKind::Feedback, "You can't go that way.");
        return;
    };
    let action = crate::affordances::go_action(ctx.actor(), exit, destination);
    crate::affordances::perform_command(ctx, &action, "You can't go that way.");
}

/// Marks an exit that cannot be traversed: the minimal door/lock primitive and the
/// state [`can_traverse`] vetoes on. Zero-sized on purpose, it is the simple
/// always-impassable case (a sealed or one-way passage). Data-carrying locks (a
/// required key, a difficulty for a skill check) are a later design that adds its
/// own components `can_traverse` also reads, not fields bolted on here. Registered
/// (see [`crate::systems::register`]) so a locked exit survives a reload.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub(crate) struct Locked;

impl NamedComponent for Locked {
    const TAG: &'static str = "locked";
}
