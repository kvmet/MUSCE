//! The reference game's in-game verb handlers: the meaning layer over the
//! engine's structural executor. Each is shaped validate -> mutate -> emit.
//! Fallible rule checks (reach, "you don't see that") run first and produce
//! player-facing feedback (a Rejection); only then does the handler commit
//! through `execute`, which cannot fail because the checks already ruled the
//! structural error out. Output is emitted through the engine's `Ctx` emit API;
//! the dispatcher resolves audiences afterward. See
//! `docs/architecture/actions.md`.

use musce_action::{Action, CommandTable, Ctx, Gate, execute};
use musce_core::{Description, EntityId, NamedComponent, World};
use musce_proto::EventKind;
use serde::{Deserialize, Serialize};

use crate::commit_or_log;
use crate::names::{self, Scope};

/// Build the reference game's command table. Movement is registered first so
/// single-letter direction abbreviations win their prefix ties (`s` is south, so
/// `say` needs `sa`).
pub fn commands() -> CommandTable {
    let mut t = CommandTable::new();
    t.register("north", Gate::Open, |c, _| go(c, "north"));
    t.register("south", Gate::Open, |c, _| go(c, "south"));
    t.register("east", Gate::Open, |c, _| go(c, "east"));
    t.register("west", Gate::Open, |c, _| go(c, "west"));
    t.register("up", Gate::Open, |c, _| go(c, "up"));
    t.register("down", Gate::Open, |c, _| go(c, "down"));
    t.register("look", Gate::Open, look);
    t.register("go", Gate::Open, go);
    t.register("take", Gate::Open, take);
    t.register("drop", Gate::Open, drop);
    t.register("pilot", Gate::Open, pilot);
    t.register("release", Gate::Open, release);
    t.register("say", Gate::Open, say);
    t.register("help", Gate::Open, help);
    t
}

// --- verbs ---------------------------------------------------------------

/// `look`: describe the actor's current room, its exits, and its contents.
pub fn look(ctx: &mut Ctx, _args: &str) {
    match describe_room(ctx.world, ctx.actor) {
        Some(text) => ctx.emit_self(EventKind::Narration, text),
        None => ctx.emit_self(EventKind::Feedback, "You are nowhere."),
    }
}

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
    let room = ctx.world.enclosing_room(character);

    // The control rule lives in `set_focus`: the cursor may only land on something
    // the character controls (transitively, so deep chains pilot too). A reject is
    // "you don't control that", surfaced to the player.
    if ctx.world.set_focus(character, target).is_err() {
        ctx.emit_self(EventKind::Feedback, "You can't pilot that.");
        return;
    }

    ctx.emit_self(EventKind::Feedback, format!("You take control of {name}."));
    if let Some(room) = room {
        ctx.emit_room_except_self(
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
    let room = ctx.world.enclosing_room(character);

    ctx.world.clear_focus(character);

    ctx.emit_self(
        EventKind::Feedback,
        format!("You release {name} and return to yourself."),
    );
    if let Some(room) = room {
        ctx.emit_room_except_self(
            room,
            EventKind::Narration,
            format!("{who} stirs and looks around."),
        );
    }
}

/// `say <message>`: speak to the room. Mutates nothing; pure output.
pub fn say(ctx: &mut Ctx, args: &str) {
    let msg = args.trim();
    if msg.is_empty() {
        ctx.emit_self(EventKind::Feedback, "Say what?");
        return;
    }
    let Some(room) = ctx.world.enclosing_room(ctx.actor) else {
        ctx.emit_self(EventKind::Feedback, "There is no one to hear you.");
        return;
    };

    let who = display_name(ctx.world, ctx.actor);
    ctx.emit_self(EventKind::Feedback, format!("You say, \"{msg}\""));
    ctx.emit_room_except_self(room, EventKind::Narration, format!("{who} says, \"{msg}\""));
}

/// `help`: list the in-world verbs. This is the game's surface, so the game
/// documents it; the engine floor's `@help` covers only the account commands.
pub fn help(ctx: &mut Ctx, _args: &str) {
    ctx.emit_self(
        EventKind::Feedback,
        "You can: look, go <direction> (or just a direction), take <item>, \
         drop <item>, pilot <thing>, release, say <message>, help.",
    );
}

// --- shared helpers ------------------------------------------------------

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
        .filter_map(|e| world.label_of(e))
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
        .filter_map(|e| description(world, e))
        .collect();
    if !others.is_empty() {
        s.push_str("\nYou see: ");
        s.push_str(&others.join(", "));
        s.push('.');
    }

    Some(s)
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
    let direction = world.label_of(exit).unwrap_or_else(|| "away".to_string());

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

/// Takeable means a movable object, not a fixture or a being: rooms and players
/// and creatures stay put. This is the gameplay rule, kept here in the handler,
/// not in `execute`.
fn is_takeable(world: &World, entity: EntityId) -> bool {
    use musce_core::{Creature, Player, Room};
    !(world.has::<Room>(entity) || world.has::<Player>(entity) || world.has::<Creature>(entity))
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

/// A name for narration. Uses the `Description` because most entities carry no
/// `Label` yet (a `Label` is the resolver match token, not display prose; see
/// names.rs), falling back to a neutral noun when an entity has no description.
fn display_name(world: &World, entity: EntityId) -> String {
    description_or(world, entity, "something")
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_action::Outbound;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Controls, Creature, Exit, Item, Label, LeadsFrom, LeadsTo, Player, Room};
    use musce_proto::{Audience, ConnectionId};

    struct Fixture {
        world: World,
        actor: EntityId,
        hall: EntityId,
        garden: EntityId,
        key: EntityId,
    }

    /// hall --north--> garden; a brass key on the garden floor; the actor in the
    /// hall. The reverse exit (garden --south--> hall) too.
    fn fixture() -> Fixture {
        let mut world = World::new();

        let hall = spawn(&mut world, |b| {
            b.add(Room);
            b.add(Description("a stone hall".into()));
        });
        let garden = spawn(&mut world, |b| {
            b.add(Room);
            b.add(Description("a quiet garden".into()));
        });
        link(&mut world, hall, garden, "north");
        link(&mut world, garden, hall, "south");

        let actor = spawn(&mut world, |b| {
            b.add(Player);
            b.add(Description("a brave adventurer".into()));
        });
        world.move_entity(actor, hall).unwrap();

        let key = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Description("a brass key".into()));
        });
        world.move_entity(key, garden).unwrap();

        Fixture {
            world,
            actor,
            hall,
            garden,
            key,
        }
    }

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    fn link(w: &mut World, from: EntityId, to: EntityId, dir: &str) {
        let exit = spawn(w, |b| {
            b.add(Exit);
            b.add(Label(dir.into()));
        });
        w.relate::<LeadsFrom>(exit, from).unwrap();
        w.relate::<LeadsTo>(exit, to).unwrap();
    }

    /// Run a handler and return its emitted (pre-resolution) outbound buffer.
    fn run(world: &mut World, actor: EntityId, f: impl FnOnce(&mut Ctx)) -> Vec<Outbound> {
        let mut out = Vec::new();
        let mut ctx = Ctx::new(world, actor, ConnectionId(1), &mut out);
        f(&mut ctx);
        out
    }

    fn self_feedback(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Connection(_)))
            .map(|o| o.event.text.clone())
            .collect()
    }

    fn room_narration(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Room(_)))
            .map(|o| o.event.text.clone())
            .collect()
    }

    #[test]
    fn look_lists_exits_and_contents() {
        let mut f = fixture();
        // Put the actor in the garden so it can see the key.
        f.world.move_entity(f.actor, f.garden).unwrap();
        let out = run(&mut f.world, f.actor, |c| look(c, ""));

        let text = &self_feedback(&out)[0];
        assert!(text.contains("a quiet garden"));
        assert!(text.contains("south")); // the garden's exit
        assert!(text.contains("a brass key")); // contents
    }

    #[test]
    fn take_moves_item_and_narrates() {
        let mut f = fixture();
        f.world.move_entity(f.actor, f.garden).unwrap(); // be where the key is
        let out = run(&mut f.world, f.actor, |c| take(c, "key"));

        // Structural effect: the key is now in the actor's inventory.
        assert_eq!(f.world.container_of(f.key), Some(f.actor));

        // Both channels fired: first-person feedback and third-person room narration.
        assert!(
            self_feedback(&out)
                .iter()
                .any(|t| t.contains("You take a brass key"))
        );
        assert!(
            room_narration(&out)
                .iter()
                .any(|t| t.contains("takes a brass key"))
        );
    }

    #[test]
    fn take_out_of_reach_rejects() {
        let mut f = fixture();
        // Actor is in the hall; the key is in the garden, out of reach.
        let out = run(&mut f.world, f.actor, |c| take(c, "key"));

        assert_eq!(f.world.container_of(f.key), Some(f.garden)); // unmoved
        assert!(
            self_feedback(&out)
                .iter()
                .any(|t| t.contains("don't see that here"))
        );
        assert!(room_narration(&out).is_empty());
    }

    #[test]
    fn go_traverses_a_valid_exit() {
        let mut f = fixture();
        let out = run(&mut f.world, f.actor, |c| go(c, "north"));

        assert_eq!(f.world.enclosing_room(f.actor), Some(f.garden));
        // The auto-look on arrival shows the destination.
        assert!(
            self_feedback(&out)
                .iter()
                .any(|t| t.contains("a quiet garden"))
        );
    }

    #[test]
    fn go_invalid_exit_rejects() {
        let mut f = fixture();
        let out = run(&mut f.world, f.actor, |c| go(c, "west"));

        assert_eq!(f.world.enclosing_room(f.actor), Some(f.hall)); // didn't move
        assert!(
            self_feedback(&out)
                .iter()
                .any(|t| t.contains("can't go that way"))
        );
    }

    /// Half of the shared-rule guarantee: a locked exit vetoes the player. The
    /// `wander` twin (`a_locked_exit_keeps_it_put` in systems.rs) proves the same
    /// veto stops a scripted/ambient mover, which is the bug routing both through
    /// `do_move` fixes. Lock the resolved exit directly (no registry needed: the
    /// veto reads `world.has::<Locked>`, not the persisted blob).
    #[test]
    fn go_through_a_locked_exit_rejects() {
        let mut f = fixture();
        let north = names::resolve(&f.world, f.actor, Scope::Exits, "north").unwrap();
        let e = f.world.index().get(north).unwrap();
        f.world.ecs.insert_one(e, Locked).unwrap();

        let out = run(&mut f.world, f.actor, |c| go(c, "north"));

        assert_eq!(f.world.enclosing_room(f.actor), Some(f.hall)); // didn't move
        assert!(self_feedback(&out).iter().any(|t| t.contains("locked")));
    }

    #[test]
    fn drop_puts_item_in_room() {
        let mut f = fixture();
        // Give the actor the key first.
        f.world.move_entity(f.key, f.actor).unwrap();
        let out = run(&mut f.world, f.actor, |c| drop(c, "key"));

        assert_eq!(f.world.container_of(f.key), Some(f.hall));
        assert!(
            self_feedback(&out)
                .iter()
                .any(|t| t.contains("You drop a brass key"))
        );
        assert!(
            room_narration(&out)
                .iter()
                .any(|t| t.contains("drops a brass key"))
        );
    }

    #[test]
    fn help_lists_in_world_verbs() {
        let mut f = fixture();
        let out = run(&mut f.world, f.actor, |c| help(c, ""));

        let text = &self_feedback(&out)[0];
        assert!(text.contains("look"));
        assert!(text.contains("say"));
        assert!(room_narration(&out).is_empty()); // pure feedback, no broadcast
    }

    #[test]
    fn say_emits_both_views_and_mutates_nothing() {
        let mut f = fixture();
        let before = f.world.enclosing_room(f.actor);
        let out = run(&mut f.world, f.actor, |c| say(c, "hello"));

        assert_eq!(f.world.enclosing_room(f.actor), before);
        assert!(
            self_feedback(&out)
                .iter()
                .any(|t| t.contains("You say, \"hello\""))
        );
        assert!(
            room_narration(&out)
                .iter()
                .any(|t| t.contains("says, \"hello\""))
        );
    }

    /// Wire a drone the actor controls into the actor's room, returning it.
    fn controlled_drone(f: &mut Fixture) -> EntityId {
        let drone = spawn(&mut f.world, |b| {
            b.add(Creature);
            b.add(Description("a patrol drone".into()));
        });
        f.world.move_entity(drone, f.hall).unwrap();
        f.world.relate::<Controls>(drone, f.actor).unwrap();
        drone
    }

    #[test]
    fn pilot_aims_focus_at_a_controlled_thing() {
        let mut f = fixture();
        let drone = controlled_drone(&mut f);

        let out = run(&mut f.world, f.actor, |c| pilot(c, "drone"));

        assert_eq!(f.world.focus_of(f.actor), Some(drone));
        assert!(
            self_feedback(&out)
                .iter()
                .any(|t| t.contains("You take control of a patrol drone"))
        );
    }

    #[test]
    fn pilot_refuses_a_thing_you_do_not_control() {
        let mut f = fixture();
        // A drone in the room, but with no Controls edge to the actor.
        let drone = spawn(&mut f.world, |b| {
            b.add(Creature);
            b.add(Description("a wild drone".into()));
        });
        f.world.move_entity(drone, f.hall).unwrap();

        let out = run(&mut f.world, f.actor, |c| pilot(c, "drone"));

        assert_eq!(f.world.focus_of(f.actor), None);
        assert!(
            self_feedback(&out)
                .iter()
                .any(|t| t.contains("can't pilot"))
        );
    }

    #[test]
    fn release_returns_focus_to_self() {
        let mut f = fixture();
        let drone = controlled_drone(&mut f);
        f.world.set_focus(f.actor, drone).unwrap();
        assert_eq!(f.world.focus_of(f.actor), Some(drone));

        // Released from inside the puppet: `character_of` walks back to the
        // controller, so the cursor clears even though the acting actor is the
        // drone.
        let out = run(&mut f.world, drone, |c| release(c, ""));

        assert_eq!(f.world.focus_of(f.actor), None);
        assert!(
            self_feedback(&out)
                .iter()
                .any(|t| t.contains("return to yourself"))
        );
    }
}
