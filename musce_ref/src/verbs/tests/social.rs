use super::*;
use crate::kinds::Creature;
use crate::verbs::{say, tell, wave};

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
