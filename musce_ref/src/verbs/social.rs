//! Social verbs: speaking and gesturing to others in the room. The three emit
//! shapes an interaction can take are all exercised here: `say` broadcasts to the
//! room, `tell` directs a private line to one entity, and `wave at` is three-party
//! (actor, target, and the rest of the room each read their own line).

use musce::action::Ctx;
use musce::wire::EventKind;

use crate::names::{self, Scope, display_name};

/// `say <message>`: speak to the room. Mutates nothing; pure output.
pub fn say(ctx: &mut Ctx, args: &str) {
    let msg = args.trim();
    if msg.is_empty() {
        ctx.emit_self(EventKind::Feedback, "Say what?");
        return;
    }
    let Some(room) = ctx.world.enclosing_locus(ctx.actor) else {
        ctx.emit_self(EventKind::Feedback, "There is no one to hear you.");
        return;
    };

    let who = display_name(ctx.world, ctx.actor);
    ctx.emit_self(EventKind::Feedback, format!("You say, \"{msg}\""));
    ctx.emit_locus_except_self(room, EventKind::Narration, format!("{who} says, \"{msg}\""));
}

/// `tell <target> <message>`: speak privately to one person in the room. Only the
/// sender and the target see it; the room does not overhear, by design. (The room
/// broadcast that would carry an overhear line, `emit_locus_except`, now exists and
/// drives `wave at`; `tell` deliberately omits it to stay private.) The first
/// consumer of `emit_entity`: the message is addressed to the target entity,
/// resolved to its connection(s) at output time, so an unconnected target hears
/// nothing.
pub fn tell(ctx: &mut Ctx, args: &str) {
    let (query, msg) = match args.trim().split_once(char::is_whitespace) {
        Some((q, m)) => (q, m.trim()),
        None => (args.trim(), ""),
    };
    if query.is_empty() {
        ctx.emit_self(EventKind::Feedback, "Tell whom?");
        return;
    }
    let Some(target) = names::resolve(ctx.world, ctx.actor, Scope::Room, query) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You don't see \"{query}\" here."),
        );
        return;
    };
    if msg.is_empty() {
        let them = display_name(ctx.world, target);
        ctx.emit_self(EventKind::Feedback, format!("Tell {them} what?"));
        return;
    }

    let who = display_name(ctx.world, ctx.actor);
    let them = display_name(ctx.world, target);
    ctx.emit_self(EventKind::Feedback, format!("You tell {them}, \"{msg}\""));
    ctx.emit_entity(
        target,
        EventKind::Narration,
        format!("{who} tells you, \"{msg}\""),
    );
}

/// `wave`, or `wave at <someone>`: a social gesture. Bare, it waves to the room.
/// Targeted, it is three-party: the actor, the target, and the rest of the room
/// each read their own line, so this is the first consumer of the room broadcast
/// that excludes a *set* (`emit_locus_except`), cutting both the actor and the
/// target from the bystander view they already saw addressed to them.
pub fn wave(ctx: &mut Ctx, args: &str) {
    let rest = args.trim();
    let query = match rest.split_once(char::is_whitespace) {
        Some(("at", who)) => who.trim(),
        _ => rest,
    };
    let Some(room) = ctx.world.enclosing_locus(ctx.actor) else {
        ctx.emit_self(EventKind::Feedback, "There is no one to see you.");
        return;
    };
    let who = display_name(ctx.world, ctx.actor);

    if query.is_empty() {
        ctx.emit_self(EventKind::Feedback, "You wave.");
        ctx.emit_locus_except_self(room, EventKind::Narration, format!("{who} waves."));
        return;
    }

    let Some(target) = names::resolve(ctx.world, ctx.actor, Scope::Room, query) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You don't see \"{query}\" here."),
        );
        return;
    };
    let them = display_name(ctx.world, target);
    ctx.emit_self(EventKind::Feedback, format!("You wave at {them}."));
    ctx.emit_entity(target, EventKind::Narration, format!("{who} waves at you."));
    ctx.emit_locus_except(
        room,
        EventKind::Narration,
        format!("{who} waves at {them}."),
        &[ctx.actor, target],
    );
}
