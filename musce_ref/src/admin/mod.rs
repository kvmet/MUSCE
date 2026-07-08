//! The reference game's admin (builder) verbs: the `@`-namespace, staff-gated,
//! rule-bypassing commands that compile straight to the structural action set,
//! skipping the gameplay rules a player command runs. They are game content (which
//! verbs exist, how they parse, their prose) over the engine's admin frame
//! (`CommandTable` + `Gate::Staff` + dispatch); the engine owns that mechanism.
//! See `docs/architecture/actions.md` (the three buckets) and
//! `docs/architecture/engine-and-game.md`.
//!
//! Entities are referenced by id, written `#7` (the form `@create`/`@dig` hand
//! back and a future `@find` will resolve names to). The creation verbs report the
//! new id so a builder can chain commands.

use musce_action::{Action, CommandTable, Ctx, Gate, execute};
use musce_core::{ComponentBlob, Controls, Description, EntityId, Name, Room, Value, World};
use musce_proto::EventKind;

use crate::commit_or_log;
use crate::kinds::{Container, Creature, Exit, Item};
use crate::names::display_name;
use crate::systems::Wander;

/// Known `@create` kinds, listed in the error when an unknown one is asked for.
const KINDS: &str = "torch, rock, goblin, box, rat";

/// Build the reference game's admin command table. All verbs are `Gate::Staff`;
/// only an actor carrying the `Staff` marker reaches them. Registration order
/// settles prefix ties: `summon` before `set` so the `s` prefix resolves to
/// `summon`; `dig` before `destroy` so the `d` prefix resolves to `dig`; and
/// `possess` before `purge` so the `p` prefix resolves to `possess`.
pub fn commands() -> CommandTable {
    let mut t = CommandTable::new();
    t.register("tel", Gate::Staff, tel);
    t.register("goto", Gate::Staff, goto);
    t.register("summon", Gate::Staff, summon);
    t.register("create", Gate::Staff, create);
    t.register("dig", Gate::Staff, dig);
    t.register("destroy", Gate::Staff, destroy);
    t.register("set", Gate::Staff, set);
    t.register("possess", Gate::Staff, possess);
    t.register("purge", Gate::Staff, purge);
    t.register("unpossess", Gate::Staff, unpossess);
    t
}

// --- verbs ---------------------------------------------------------------

/// `@tel #<thing> #<dest>`: move any entity into any other. Admin: no reach or
/// takeable rule, only the engine's structural cycle check.
pub fn tel(ctx: &mut Ctx, args: &str) {
    let mut p = args.split_whitespace();
    let (Some(t_tok), Some(d_tok)) = (p.next(), p.next()) else {
        ctx.emit_self(EventKind::Feedback, "Usage: @tel #<thing> #<dest>.");
        return;
    };
    let (Some(target), Some(dest)) = (parse_ref(ctx.world, t_tok), parse_ref(ctx.world, d_tok))
    else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };

    if execute(
        ctx.world,
        Action::Move {
            entity: target,
            into: dest,
        },
    )
    .is_err()
    {
        ctx.emit_self(
            EventKind::Feedback,
            format!("Can't put #{} there; it would create a cycle.", target.0),
        );
        return;
    }
    ctx.emit_self(
        EventKind::Feedback,
        format!("Teleported #{} into #{}.", target.0, dest.0),
    );
}

/// `@goto #<thing>`: travel to where a thing is (the room enclosing it). Refuses
/// a thing with no location (e.g. a top-level room), pointing at `@tel`.
pub fn goto(ctx: &mut Ctx, args: &str) {
    let Some(target) = parse_ref(ctx.world, args.trim()) else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };
    let Some(room) = ctx.world.enclosing_room(target) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("#{} has no location to go to. Did you mean @tel?", target.0),
        );
        return;
    };
    // The destination is the room enclosing `target`, never an ancestor of the
    // acting being, so moving the actor into it cannot cycle: a fire is a bug.
    if !commit_or_log(
        ctx.world,
        Action::Move {
            entity: ctx.actor,
            into: room,
        },
        "@goto: move actor into the target's room",
    ) {
        ctx.emit_self(EventKind::Feedback, "Something blocks the way.");
        return;
    }
    crate::verbs::look(ctx, "");
}

/// `@summon #<thing>`: bring a thing directly to you, wherever it is now.
pub fn summon(ctx: &mut Ctx, args: &str) {
    let Some(target) = parse_ref(ctx.world, args.trim()) else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };
    let Some(dest) = ctx.world.container_of(ctx.actor) else {
        ctx.emit_self(EventKind::Feedback, "You are nowhere to summon it to.");
        return;
    };
    if execute(
        ctx.world,
        Action::Move {
            entity: target,
            into: dest,
        },
    )
    .is_err()
    {
        ctx.emit_self(EventKind::Feedback, "You can't summon that here.");
        return;
    }
    ctx.emit_self(EventKind::Feedback, format!("Summoned #{}.", target.0));
}

/// `@create <kind>`: spawn a thing from the kind table into your room, reporting
/// its new id. Compound (Create then Move), both checks front-loaded.
pub fn create(ctx: &mut Ctx, args: &str) {
    let kind = args.trim();
    let Some(blob) = kind_blob(kind) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("Create what? Known kinds: {KINDS}."),
        );
        return;
    };
    let Some(room) = ctx.world.enclosing_room(ctx.actor) else {
        ctx.emit_self(EventKind::Feedback, "You are nowhere to create it.");
        return;
    };

    let id = match execute(ctx.world, Action::Create { components: blob }) {
        Ok(id) => id,
        Err(e) => {
            ctx.emit_self(EventKind::Feedback, format!("Couldn't create that: {e}."));
            return;
        }
    };
    // A fresh, location-less entity moving into a room cannot cycle: a fire here
    // is a bug, not the builder's mistake.
    if !commit_or_log(
        ctx.world,
        Action::Move {
            entity: id,
            into: room,
        },
        "@create: place the new entity in the room",
    ) {
        ctx.emit_self(EventKind::Feedback, "Created it, but couldn't place it.");
        return;
    }
    ctx.emit_self(
        EventKind::Feedback,
        format!("Created {} as #{}.", display_name(ctx.world, id), id.0),
    );
}

/// `@dig <dir> [name]`: dig a new room in a direction and link it both ways.
/// Compound; every check (direction known, no colliding exit, a room to dig from)
/// runs before the first mutation.
pub fn dig(ctx: &mut Ctx, args: &str) {
    let mut p = args.splitn(2, char::is_whitespace);
    let dir = p.next().unwrap_or("").trim();
    let Some((dir, opposite)) = opposite_dir(dir) else {
        ctx.emit_self(
            EventKind::Feedback,
            "Dig which way? Use n/s/e/w/u/d (or the full direction).",
        );
        return;
    };
    let name = p
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("a freshly dug passage");
    let Some(here) = ctx.world.enclosing_room(ctx.actor) else {
        ctx.emit_self(EventKind::Feedback, "You are nowhere to dig from.");
        return;
    };
    if has_exit(ctx.world, here, dir) {
        ctx.emit_self(
            EventKind::Feedback,
            format!("There is already an exit {dir} from here."),
        );
        return;
    }

    let new = match execute(
        ctx.world,
        Action::Create {
            components: room_blob(name),
        },
    ) {
        Ok(id) => id,
        Err(e) => {
            ctx.emit_self(EventKind::Feedback, format!("Couldn't dig: {e}."));
            return;
        }
    };
    if !dig_exit(ctx.world, here, new, dir) || !dig_exit(ctx.world, new, here, opposite) {
        ctx.emit_self(EventKind::Feedback, "Dug the room, but couldn't link it.");
        return;
    }
    ctx.emit_self(
        EventKind::Feedback,
        format!("Dug {dir} to {name} (#{}).", new.0),
    );
}

/// `@set #<id>.<component> <json>`: overwrite a whole component with a JSON value.
/// Field-level paths (`#<id>.<component>.<field>`) are reserved but not built; the
/// engine's structural guards reject the identity tag and relation tags.
pub fn set(ctx: &mut Ctx, args: &str) {
    let mut p = args.splitn(2, char::is_whitespace);
    let path = p.next().unwrap_or("").trim();
    let Some(json) = p.next().map(str::trim).filter(|s| !s.is_empty()) else {
        ctx.emit_self(
            EventKind::Feedback,
            "Usage: @set #<id>.<component> <json-value>.",
        );
        return;
    };

    let mut seg = path.split('.');
    let Some(id) = seg.next().and_then(|t| parse_ref(ctx.world, t)) else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };
    let Some(component) = seg.next().filter(|s| !s.is_empty()) else {
        ctx.emit_self(
            EventKind::Feedback,
            "Set what? Try @set #7.description \"...\".",
        );
        return;
    };
    if seg.next().is_some() {
        ctx.emit_self(
            EventKind::Feedback,
            "Field-level @set isn't supported yet; set the whole component (@set #7.<component> <json>).",
        );
        return;
    }
    let Ok(value) = json.parse::<Value>() else {
        ctx.emit_self(
            EventKind::Feedback,
            "Value must be JSON; quote strings, e.g. @set #7.description \"a torch\".",
        );
        return;
    };

    match execute(
        ctx.world,
        Action::SetComponent {
            entity: id,
            tag: component.to_string(),
            value,
        },
    ) {
        Ok(_) => ctx.emit_self(
            EventKind::Feedback,
            format!("Set {component} on #{}.", id.0),
        ),
        Err(e) => ctx.emit_self(EventKind::Feedback, format!("Can't set that: {e}.")),
    }
}

/// `@possess #<target>`: establish the control capability edge from you onto a
/// target, so you may pilot it. This only wires the `Controls` edge; aiming the
/// control cursor is the player `pilot` verb's job. Refuses a target already
/// controlled by someone else rather than silently re-homing it (which would
/// strand the prior controller's cursor); forcibly taking another's puppet would
/// be a separate, explicit capability.
pub fn possess(ctx: &mut Ctx, args: &str) {
    let Some(target) = parse_ref(ctx.world, args.trim()) else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };
    if target == ctx.actor {
        ctx.emit_self(EventKind::Feedback, "You can't possess yourself.");
        return;
    }
    match ctx.world.target_of::<Controls>(target) {
        Some(c) if c == ctx.actor => {
            ctx.emit_self(
                EventKind::Feedback,
                format!("You already control #{}.", target.0),
            );
            return;
        }
        Some(c) => {
            ctx.emit_self(
                EventKind::Feedback,
                format!("#{} is already controlled by #{}.", target.0, c.0),
            );
            return;
        }
        None => {}
    }
    match execute(
        ctx.world,
        Action::Relate {
            source: target,
            target: ctx.actor,
            kind: "controlled_by".into(),
        },
    ) {
        Ok(_) => ctx.emit_self(
            EventKind::Feedback,
            format!("You possess #{}; pilot it to take the helm.", target.0),
        ),
        Err(e) => ctx.emit_self(EventKind::Feedback, format!("Can't possess that: {e}.")),
    }
}

/// `@unpossess #<target>`: tear down your control edge onto a target. Refuses a
/// target you do not control, so it only ever drops your own edge: that keeps the
/// dangling-focus clear below sound (it touches only your cursor) and avoids
/// stranding another controller's. Removing the edge can strand your focus
/// pointing into the target's now-detached subtree, so while the chain is still
/// intact this clears a focus aimed at the target or any of its descendants before
/// unrelating.
pub fn unpossess(ctx: &mut Ctx, args: &str) {
    let Some(target) = parse_ref(ctx.world, args.trim()) else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };
    match ctx.world.target_of::<Controls>(target) {
        Some(c) if c == ctx.actor => {}
        Some(c) => {
            ctx.emit_self(
                EventKind::Feedback,
                format!("#{} is controlled by #{}, not you.", target.0, c.0),
            );
            return;
        }
        None => {
            ctx.emit_self(
                EventKind::Feedback,
                format!("Nothing controls #{}.", target.0),
            );
            return;
        }
    }
    if let Some(f) = ctx.world.focus_of(ctx.actor)
        && (f == target || ctx.world.ancestors::<Controls>(f).contains(&target))
    {
        ctx.world.clear_focus(ctx.actor);
    }
    // `Unrelate` of a registered, hardcoded kind only fails on an unknown kind,
    // so this cannot fail; report success, and let `commit_or_log` shout if the
    // registry ever regresses.
    if commit_or_log(
        ctx.world,
        Action::Unrelate {
            source: target,
            kind: "controlled_by".into(),
        },
        "@unpossess: clear controlled_by",
    ) {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You release control of #{}.", target.0),
        );
    }
}

/// `@destroy #<target>`: despawn one entity. The containment cascade spills the
/// target's contents up into its own container (a destroyed box drops its coins to
/// the floor) and detaches a destroyed room's exits, so this is the safe default;
/// `@purge` is the recursive opt-in that takes the contents with it.
pub fn destroy(ctx: &mut Ctx, args: &str) {
    let Some(target) = parse_ref(ctx.world, args.trim()) else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };
    if target == ctx.actor {
        ctx.emit_self(EventKind::Feedback, "You can't destroy yourself.");
        return;
    }
    // `Destroy` is infallible (despawn no-ops on a missing entity), so there is
    // no error to report; the subject is discarded.
    let _ = execute(ctx.world, Action::Destroy { entity: target });
    ctx.emit_self(
        EventKind::Feedback,
        format!(
            "Destroyed #{}; its contents spilled where it stood.",
            target.0
        ),
    );
}

/// `@purge #<target>`: recursively despawn a target and everything inside it,
/// depth-first so the contents go with it rather than spilling out. Irreversible,
/// so it refuses the actor and refuses a subtree the actor is standing inside (you
/// can't purge the room out from under yourself).
pub fn purge(ctx: &mut Ctx, args: &str) {
    let Some(target) = parse_ref(ctx.world, args.trim()) else {
        ctx.emit_self(EventKind::Feedback, bad_ref());
        return;
    };
    if target == ctx.actor {
        ctx.emit_self(EventKind::Feedback, "You can't purge yourself.");
        return;
    }
    let mut c = ctx.world.container_of(ctx.actor);
    while let Some(x) = c {
        if x == target {
            ctx.emit_self(
                EventKind::Feedback,
                "You can't purge something you're inside.",
            );
            return;
        }
        c = ctx.world.container_of(x);
    }
    purge_entity(ctx.world, target);
    ctx.emit_self(
        EventKind::Feedback,
        format!("Purged #{} and everything in it.", target.0),
    );
}

// --- helpers -------------------------------------------------------------

/// Recursively despawn `e` and its containment subtree, post-order so each child
/// is gone before its parent. Contents are collected into an owned Vec first, as
/// despawning mutates the world while we walk it.
fn purge_entity(world: &mut World, e: EntityId) {
    for child in world.contents(e) {
        purge_entity(world, child);
    }
    let _ = execute(world, Action::Destroy { entity: e });
}

/// Resolve a `#<id>` token to a live entity. `None` if it lacks the `#`, is not a
/// number, or names nothing in the world.
fn parse_ref(world: &World, token: &str) -> Option<EntityId> {
    let id = EntityId(token.strip_prefix('#')?.parse().ok()?);
    world.entity(id).is_some().then_some(id)
}

fn bad_ref() -> &'static str {
    "No such entity. Entity references look like #7."
}

/// The blob for a `@create` kind, or `None` if unknown. Plain components only (no
/// id, no relation tags), so `World::create` accepts it. Each kind names its Rust
/// components and `ComponentBlob` derives the tags; a kind can carry several
/// markers (a `rat` is a creature that wanders). The `Wander` tag only round-trips
/// because the game registered it (see `systems::register`).
fn kind_blob(kind: &str) -> Option<Value> {
    let blob = ComponentBlob::new();
    let blob = match kind {
        "torch" => blob
            .with(Item)
            .with(Description("a guttering torch".into())),
        "rock" => blob.with(Item).with(Description("a heavy rock".into())),
        "goblin" => blob
            .with(Creature)
            .with(Description("a snaggle-toothed goblin".into())),
        "box" => blob
            .with(Container)
            .with(Description("a sturdy wooden box".into())),
        "rat" => blob
            .with(Creature)
            .with(Wander)
            .with(Description("a twitching sewer rat".into())),
        _ => return None,
    };
    Some(blob.build())
}

fn room_blob(name: &str) -> Value {
    ComponentBlob::new()
        .with(Room)
        .with(Description(name.into()))
        .build()
}

/// Map a typed direction (abbreviation or full word) to its canonical name and
/// the canonical name of its opposite. Hardcoded n/s, e/w, u/d.
fn opposite_dir(d: &str) -> Option<(&'static str, &'static str)> {
    Some(match d.to_lowercase().as_str() {
        "n" | "north" => ("north", "south"),
        "s" | "south" => ("south", "north"),
        "e" | "east" => ("east", "west"),
        "w" | "west" => ("west", "east"),
        "u" | "up" => ("up", "down"),
        "d" | "down" => ("down", "up"),
        _ => return None,
    })
}

fn has_exit(world: &World, room: EntityId, dir: &str) -> bool {
    world
        .exits_of(room)
        .into_iter()
        .any(|e| world.name_of(e).as_deref() == Some(dir))
}

/// Spawn one exit entity from `from` to `to`, labeled `label`, through the
/// executor (Create the exit, then Relate it both endpoints) so the wiring
/// rides the action path like every other mutation. Returns whether it
/// committed.
fn dig_exit(world: &mut World, from: EntityId, to: EntityId, label: &str) -> bool {
    let exit = match execute(
        world,
        Action::Create {
            components: exit_blob(label),
        },
    ) {
        Ok(id) => id,
        Err(e) => {
            // A hardcoded exit blob cannot fail to create; a fire means the blob
            // or the registry regressed, so shout rather than quietly fail.
            tracing::error!(error = %e, "@dig: exit blob failed to create");
            return false;
        }
    };
    // A fresh exit pointing at existing rooms cannot cycle, and the kinds are
    // hardcoded registered literals, so both wires should always commit.
    commit_or_log(
        world,
        Action::Relate {
            source: exit,
            target: from,
            kind: "leads_from".into(),
        },
        "@dig: wire the exit's leads_from",
    ) && commit_or_log(
        world,
        Action::Relate {
            source: exit,
            target: to,
            kind: "leads_to".into(),
        },
        "@dig: wire the exit's leads_to",
    )
}

fn exit_blob(name: &str) -> Value {
    ComponentBlob::new()
        .with(Exit)
        .with(Name(name.to_string()))
        .build()
}

#[cfg(test)]
mod tests;
