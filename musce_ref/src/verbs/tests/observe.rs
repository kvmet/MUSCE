use super::*;
use crate::kinds::Container;
use crate::verbs::{examine, inventory, look};

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
