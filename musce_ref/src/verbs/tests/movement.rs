use super::*;
use crate::names::{self, Scope};
use crate::verbs::{Locked, go};

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
/// veto stops a scripted/ambient mover through the same canonical `go` guard.
#[test]
fn go_through_a_locked_exit_rejects() {
    let mut f = fixture();
    let north = names::resolve(&f.world, f.actor, Scope::Exits, "north").unwrap();
    f.world.insert(north, Locked).unwrap();

    let out = run(&mut f.world, f.actor, |c| go(c, "north"));

    assert_eq!(f.world.enclosing_locus(f.actor), Some(f.hall)); // didn't move
    assert!(self_feedback(&out).iter().any(|t| t.contains("locked")));
}
