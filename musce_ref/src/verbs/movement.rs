//! Movement: traversing exits between rooms. The rule-checked move itself
//! ([`do_move`]) is shared with the ambient `wander` system and scripted
//! sequences, so every mover is vetoed alike; the `go` verb owns only the parse,
//! the exit resolution, and the player-facing prose.

use serde::{Deserialize, Serialize};

use musce_action::{Action, Ctx};
use musce_core::{EntityId, NamedComponent, World};
use musce_proto::EventKind;

use crate::commit_or_log;
use crate::names::{self, Scope, display_name};

use super::observe::look;

/// `go <dir>` / a bare direction: traverse the named exit out of the room. The
/// rule-checked move itself lives in [`do_move`], shared with the ambient `wander`
/// system (and, later, scripted sequences); this verb owns only the parse, the
/// exit resolution, and the player-facing prose.
pub fn go(ctx: &mut Ctx, dir: &str) {
    let dir = dir.trim();
    if ctx.world.enclosing_room(ctx.actor).is_none() {
        ctx.emit_self(EventKind::Feedback, "You are nowhere.");
        return;
    }
    if dir.is_empty() {
        ctx.emit_self(EventKind::Feedback, "Go where?");
        return;
    }

    let Some(exit) = names::resolve(ctx.world, ctx.actor, Scope::Exits, dir) else {
        ctx.emit_self(EventKind::Feedback, "You can't go that way.");
        return;
    };

    let who = display_name(ctx.world, ctx.actor);
    match do_move(ctx.world, ctx.actor, exit) {
        MoveOutcome::Moved {
            from,
            dest,
            direction,
        } => {
            // Departure/arrival narration is audience-resolved after the handler
            // runs, against the committed world, so the actor (now in `dest`) is
            // not among the departure room's hearers.
            if let Some(from) = from {
                ctx.emit_room_except_self(
                    from,
                    EventKind::Narration,
                    format!("{who} leaves {direction}."),
                );
            }
            ctx.emit_room_except_self(dest, EventKind::Narration, format!("{who} arrives."));
            ctx.emit_self(EventKind::Feedback, format!("You go {direction}."));
            look(ctx, "");
        }
        // A half-wired exit (no destination) is no exit to the player.
        MoveOutcome::NoDestination => {
            ctx.emit_self(EventKind::Feedback, "You can't go that way.");
        }
        MoveOutcome::Blocked(reason) => {
            ctx.emit_self(EventKind::Feedback, reason);
        }
    }
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

/// The result of a [`do_move`] attempt, so each caller phrases its own narration
/// over one shared rule-checked move. `Moved` carries the room left (`from`, `None`
/// if the mover was location-less), the destination, and the exit's label.
pub(crate) enum MoveOutcome {
    Moved {
        from: Option<EntityId>,
        dest: EntityId,
        direction: String,
    },
    /// The exit has no destination (a half-wired exit): no exit, to the mover.
    NoDestination,
    /// A traversal rule vetoed the move; carries the player-facing reason.
    Blocked(&'static str),
}

/// Move `actor` through `exit`, subject to the traversal rules: the single
/// rule-checked move path, shared by the player `go` verb, the ambient `wander`
/// system, and scripted sequences, so a scripted or wandering mover is vetoed
/// exactly as a player is. Resolves the destination, runs [`can_traverse`], then
/// commits through `execute`; it emits no narration (each caller owns its prose).
/// The caller resolves which exit (by direction name, or by deterministic choice)
/// and hands it in already resolved.
pub(crate) fn do_move(world: &mut World, actor: EntityId, exit: EntityId) -> MoveOutcome {
    let Some(dest) = world.exit_destination(exit) else {
        return MoveOutcome::NoDestination;
    };
    if let Err(reason) = can_traverse(world, actor, exit) {
        return MoveOutcome::Blocked(reason);
    }

    // Capture the room being left before the move commits; the caller narrates the
    // departure to it.
    let from = world.enclosing_room(actor);
    let direction = world.name_of(exit).unwrap_or_else(|| "away".to_string());

    // A being moving into a room cannot close a containment cycle, so this should
    // never fail; a bug here is logged loud rather than silently swallowed.
    if !commit_or_log(
        world,
        Action::Move {
            entity: actor,
            into: dest,
        },
        "do_move: move actor into the exit destination",
    ) {
        return MoveOutcome::Blocked("Something blocks the way.");
    }

    MoveOutcome::Moved {
        from,
        dest,
        direction,
    }
}

/// Whether `mover` may traverse `exit` right now. The traversal veto is game
/// policy, not an engine concept: the engine provides the exit entity and a home
/// for door/lock state but bakes in no lock semantics. The single traversal-rule
/// seam, reached through [`do_move`] so every mover (player, wanderer, sequence)
/// is vetoed alike. Today the one veto is a [`Locked`] exit; richer checks (a key,
/// a skill check, open/closed door state) slot in here additively.
fn can_traverse(world: &World, _mover: EntityId, exit: EntityId) -> Result<(), &'static str> {
    if world.has::<Locked>(exit) {
        return Err("It's locked.");
    }
    Ok(())
}
