use super::movement::Locked;
use super::{drop, examine, go, help, inventory, look, pilot, release, say, take, tell, wave};
use crate::exits::{LeadsFrom, LeadsTo};
use crate::kinds::{Container, Creature, Exit, Item, Player};
use crate::names::{self, Scope};
use musce_action::{Ctx, Outbound, Verdict};
use musce_core::hecs::EntityBuilder;
use musce_core::{Controls, Description, EntityId, Locus, Name, World};
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
        b.add(Locus);
        b.add(Description("a stone hall".into()));
    });
    let garden = spawn(&mut world, |b| {
        b.add(Locus);
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
        b.add(Name(dir.into()));
    });
    w.relate::<LeadsFrom>(exit, from).unwrap();
    w.relate::<LeadsTo>(exit, to).unwrap();
}

/// Run a handler and return its emitted (pre-resolution) outbound buffer.
fn run(world: &mut World, actor: EntityId, f: impl FnOnce(&mut Ctx)) -> Vec<Outbound> {
    let mut out = Vec::new();
    let verdict = Verdict::guest();
    let mut ctx = Ctx::new(world, actor, ConnectionId(1), &verdict, &mut out);
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
        .filter(|o| matches!(o.event.to, Audience::Locus(_)))
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
fn tell_addresses_the_target_privately() {
    let mut f = fixture();
    // A second being standing in the hall with the actor.
    let guard = spawn(&mut f.world, |b| {
        b.add(Creature);
        b.add(Name("a stone guard".into()));
    });
    f.world.move_entity(guard, f.hall).unwrap();

    let out = run(&mut f.world, f.actor, |c| tell(c, "guard hello there"));

    // Sender sees a confirmation.
    assert!(
        self_feedback(&out)
            .iter()
            .any(|t| t.contains("You tell a stone guard, \"hello there\""))
    );
    // The message is directed to the target entity, not broadcast to the room.
    let directed: Vec<String> = out
        .iter()
        .filter(|o| matches!(o.event.to, Audience::Entity(e) if e == guard))
        .map(|o| o.event.text.clone())
        .collect();
    assert_eq!(directed.len(), 1);
    assert!(directed[0].contains("tells you, \"hello there\""));
    // No room-overhear: nobody else present sees it.
    assert!(room_narration(&out).is_empty());
}

#[test]
fn tell_without_a_target_present_rejects() {
    let mut f = fixture();
    let out = run(&mut f.world, f.actor, |c| tell(c, "nobody hi"));

    assert!(self_feedback(&out).iter().any(|t| t.contains("don't see")));
    assert!(
        out.iter()
            .all(|o| !matches!(o.event.to, Audience::Entity(_)))
    );
}

#[test]
fn wave_at_target_is_three_party() {
    let mut f = fixture();
    let guard = spawn(&mut f.world, |b| {
        b.add(Creature);
        b.add(Name("a stone guard".into()));
    });
    f.world.move_entity(guard, f.hall).unwrap();

    let out = run(&mut f.world, f.actor, |c| wave(c, "at guard"));

    // Actor sees a first-person confirmation.
    assert!(
        self_feedback(&out)
            .iter()
            .any(|t| t == "You wave at a stone guard.")
    );
    // Target gets a directed second-person line, addressed to the entity.
    let directed: Vec<String> = out
        .iter()
        .filter(|o| matches!(o.event.to, Audience::Entity(e) if e == guard))
        .map(|o| o.event.text.clone())
        .collect();
    assert_eq!(directed.len(), 1);
    assert!(directed[0].contains("waves at you"));
    // The room gets one bystander line, cutting both parties who already saw one.
    let room: Vec<&Outbound> = out
        .iter()
        .filter(|o| matches!(o.event.to, Audience::Locus(_)))
        .collect();
    assert_eq!(room.len(), 1);
    assert!(room[0].event.text.contains("waves at a stone guard"));
    assert!(room[0].exclude.contains(&f.actor) && room[0].exclude.contains(&guard));
}

#[test]
fn wave_bare_greets_the_room() {
    let mut f = fixture();
    let out = run(&mut f.world, f.actor, |c| wave(c, ""));

    assert!(self_feedback(&out).iter().any(|t| t == "You wave."));
    let room = room_narration(&out);
    assert_eq!(room.len(), 1);
    assert!(room[0].contains("waves."));
    // No target, so no directed line.
    assert!(
        out.iter()
            .all(|o| !matches!(o.event.to, Audience::Entity(_)))
    );
}

#[test]
fn go_traverses_a_valid_exit() {
    let mut f = fixture();
    let out = run(&mut f.world, f.actor, |c| go(c, "north"));

    assert_eq!(f.world.enclosing_locus(f.actor), Some(f.garden));
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

    assert_eq!(f.world.enclosing_locus(f.actor), Some(f.hall)); // didn't move
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
    f.world.insert(north, Locked);

    let out = run(&mut f.world, f.actor, |c| go(c, "north"));

    assert_eq!(f.world.enclosing_locus(f.actor), Some(f.hall)); // didn't move
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
    let before = f.world.enclosing_locus(f.actor);
    let out = run(&mut f.world, f.actor, |c| say(c, "hello"));

    assert_eq!(f.world.enclosing_locus(f.actor), before);
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

#[test]
fn examine_reveals_a_things_description() {
    let mut f = fixture();
    f.world.move_entity(f.actor, f.garden).unwrap(); // be where the key is
    let out = run(&mut f.world, f.actor, |c| examine(c, "key"));

    assert!(
        self_feedback(&out)
            .iter()
            .any(|t| t.contains("a brass key")),
        "examine shows the target's description, got: {:?}",
        self_feedback(&out)
    );
    assert!(room_narration(&out).is_empty()); // examine is private
}

#[test]
fn examine_self_looks_at_the_actor() {
    let mut f = fixture();
    let out = run(&mut f.world, f.actor, |c| examine(c, "me"));

    assert!(
        self_feedback(&out)
            .iter()
            .any(|t| t.contains("a brave adventurer"))
    );
}

#[test]
fn examine_a_thing_not_present_rejects() {
    let mut f = fixture();
    let out = run(&mut f.world, f.actor, |c| examine(c, "dragon"));

    assert!(
        self_feedback(&out)
            .iter()
            .any(|t| t.contains("don't see that here"))
    );
}

#[test]
fn examine_a_container_reveals_its_contents() {
    let mut f = fixture();
    let chest = spawn(&mut f.world, |b| {
        b.add(Container);
        b.add(Name("a wooden chest".into()));
    });
    f.world.move_entity(chest, f.hall).unwrap();

    // An empty container reads as empty.
    let empty = run(&mut f.world, f.actor, |c| examine(c, "chest"));
    assert!(
        self_feedback(&empty)
            .iter()
            .any(|t| t.contains("It is empty.")),
        "an empty container reports it, got: {:?}",
        self_feedback(&empty)
    );

    // With something inside, examine lists it.
    let coin = spawn(&mut f.world, |b| {
        b.add(Item);
        b.add(Name("a copper coin".into()));
    });
    f.world.move_entity(coin, chest).unwrap();
    let full = run(&mut f.world, f.actor, |c| examine(c, "chest"));
    assert!(
        self_feedback(&full)
            .iter()
            .any(|t| t.contains("It contains: a copper coin.")),
        "a full container lists its contents, got: {:?}",
        self_feedback(&full)
    );
}

#[test]
fn look_with_an_argument_examines() {
    let mut f = fixture();
    f.world.move_entity(f.actor, f.garden).unwrap();
    let out = run(&mut f.world, f.actor, |c| look(c, "key"));

    // `look key` reveals the key, not the room.
    assert!(
        self_feedback(&out)
            .iter()
            .any(|t| t.contains("a brass key"))
    );
}

#[test]
fn inventory_lists_held_things_and_reports_empty() {
    let mut f = fixture();

    let out = run(&mut f.world, f.actor, |c| inventory(c, ""));
    assert!(
        self_feedback(&out)
            .iter()
            .any(|t| t.contains("carrying nothing"))
    );

    // Give the actor the key, then it shows up in the listing.
    f.world.move_entity(f.key, f.actor).unwrap();
    let out = run(&mut f.world, f.actor, |c| inventory(c, ""));
    assert!(
        self_feedback(&out)
            .iter()
            .any(|t| t.contains("carrying") && t.contains("a brass key"))
    );
}
