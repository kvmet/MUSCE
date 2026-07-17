use super::*;
use crate::verbs::{drop, take};

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
