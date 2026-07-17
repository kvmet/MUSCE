use super::*;
use crate::kinds::Creature;
use crate::verbs::{pilot, release};
use musce::world::Controls;

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
