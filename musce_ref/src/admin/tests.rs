//! Unit tests for the admin builder verbs.

use super::*;
use crate::kinds::{Container, Creature, Item, Player};
use musce::action::{Audience, Outbound, Verdict};
use musce::wire::ConnectionId;
use musce::world::hecs::EntityBuilder;
use musce::world::{Description, Locus};

fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
    let mut b = EntityBuilder::new();
    f(&mut b);
    w.spawn(b)
}

fn described(w: &mut World, marker: impl FnOnce(&mut EntityBuilder), desc: &str) -> EntityId {
    spawn(w, |b| {
        marker(b);
        b.add(Description(desc.into()));
    })
}

/// A world with a builder standing in a hall: (world, hall, builder). These tests
/// call the admin handlers directly, past the capability gate, so the builder needs
/// no grant; the gate itself is covered in the dispatch layer.
fn world_with_builder() -> (World, EntityId, EntityId) {
    let mut w = World::new();
    // `@create` builds a component blob and spawns it through the registry, so the
    // game's own kinds must be registered, exactly as the runtime does at boot.
    crate::systems::register(&mut w);
    let hall = described(
        &mut w,
        |b| {
            b.add(Locus);
        },
        "a stone hall",
    );
    let builder = spawn(&mut w, |b| {
        b.add(Player);
        b.add(Description("a builder".into()));
    });
    w.move_entity(builder, hall).unwrap();
    (w, hall, builder)
}

fn run(world: &mut World, actor: EntityId, f: impl FnOnce(&mut Ctx)) -> Vec<Outbound> {
    let mut out = Vec::new();
    // A handler-level verdict; these tests exercise the verbs past the gate, and the
    // admin handlers read no authority of their own.
    let verdict = Verdict::guest();
    let mut ctx = Ctx::new(world, actor, ConnectionId(1), &verdict, &mut out);
    f(&mut ctx);
    out
}

fn feedback(out: &[Outbound]) -> Vec<String> {
    out.iter()
        .filter(|o| matches!(o.event.to, Audience::Connection(_)))
        .map(|o| o.event.text.clone())
        .collect()
}

fn re(id: EntityId) -> String {
    format!("#{}", id.0)
}

/// The destination of a room's exit in a given direction, if any.
fn exit_to(w: &World, room: EntityId, dir: &str) -> Option<EntityId> {
    w.exits_of(room)
        .into_iter()
        .find(|&e| w.name_of(e).as_deref() == Some(dir))
        .and_then(|e| w.exit_destination(e))
}

#[test]
fn tel_moves_any_entity_into_any_other() {
    let (mut w, hall, builder) = world_with_builder();
    let coin = described(
        &mut w,
        |b| {
            b.add(Item);
        },
        "a coin",
    ); // location-less

    let out = run(&mut w, builder, |c| {
        tel(c, &format!("{} {}", re(coin), re(hall)))
    });

    assert_eq!(w.container_of(coin), Some(hall));
    assert!(feedback(&out).iter().any(|t| t.contains("Teleported")));
}

#[test]
fn tel_without_hash_prefix_is_a_bad_ref() {
    let (mut w, hall, builder) = world_with_builder();
    let out = run(&mut w, builder, |c| tel(c, &format!("7 {}", hall.0)));
    assert!(feedback(&out).iter().any(|t| t.contains("look like #7")));
}

#[test]
fn goto_travels_to_the_room_a_thing_is_in() {
    let (mut w, _hall, builder) = world_with_builder();
    let cellar = described(
        &mut w,
        |b| {
            b.add(Locus);
        },
        "a damp cellar",
    );
    let lamp = described(
        &mut w,
        |b| {
            b.add(Item);
        },
        "a lamp",
    );
    w.move_entity(lamp, cellar).unwrap();

    run(&mut w, builder, |c| goto(c, &re(lamp)));

    assert_eq!(w.enclosing_locus(builder), Some(cellar));
}

#[test]
fn goto_refuses_a_thing_with_no_location() {
    let (mut w, hall, builder) = world_with_builder();
    let void = described(
        &mut w,
        |b| {
            b.add(Locus);
        },
        "a void",
    ); // top-level room

    let out = run(&mut w, builder, |c| goto(c, &re(void)));

    assert_eq!(w.enclosing_locus(builder), Some(hall)); // did not move
    assert!(
        feedback(&out)
            .iter()
            .any(|t| t.contains("no location to go to") && t.contains("@tel"))
    );
}

#[test]
fn summon_brings_a_thing_to_you_from_anywhere() {
    let (mut w, hall, builder) = world_with_builder();
    let far = described(
        &mut w,
        |b| {
            b.add(Locus);
        },
        "a far room",
    );
    let goblin = described(
        &mut w,
        |b| {
            b.add(Creature);
        },
        "a goblin",
    );
    w.move_entity(goblin, far).unwrap();

    run(&mut w, builder, |c| summon(c, &re(goblin)));

    // It came to the builder's own container, wherever it was before.
    assert_eq!(w.container_of(goblin), Some(hall));
}

#[test]
fn create_spawns_into_the_room_and_reports_the_id() {
    let (mut w, hall, builder) = world_with_builder();
    let before = w.contents(hall).len();

    let out = run(&mut w, builder, |c| create(c, "torch"));

    let contents = w.contents(hall);
    assert_eq!(contents.len(), before + 1);
    let torch = *contents.iter().find(|&&e| e != builder).unwrap();
    assert!(w.has::<Item>(torch));
    assert!(
        feedback(&out)
            .iter()
            .any(|t| t.contains("Created") && t.contains('#'))
    );
}

#[test]
fn create_unknown_kind_spawns_nothing() {
    let (mut w, _hall, builder) = world_with_builder();
    let before = w.index().len();

    let out = run(&mut w, builder, |c| create(c, "dragon"));

    assert_eq!(w.index().len(), before);
    assert!(feedback(&out).iter().any(|t| t.contains("Known kinds")));
}

#[test]
fn dig_creates_a_room_with_reciprocal_exits() {
    let (mut w, hall, builder) = world_with_builder();

    run(&mut w, builder, |c| dig(c, "north a winding stair"));

    let new = exit_to(&w, hall, "north").expect("north exit added to here");
    assert!(w.has::<Locus>(new));
    assert_eq!(exit_to(&w, new, "south"), Some(hall)); // reciprocal back
}

#[test]
fn dig_refuses_a_colliding_exit_and_creates_nothing() {
    let (mut w, _hall, builder) = world_with_builder();
    run(&mut w, builder, |c| dig(c, "north")); // first dig: succeeds
    let count = w.index().len();

    let out = run(&mut w, builder, |c| dig(c, "north")); // collides

    // The collision check fires before Create, so nothing new spawned.
    assert_eq!(w.index().len(), count);
    assert!(feedback(&out).iter().any(|t| t.contains("already an exit")));
}

#[test]
fn set_overwrites_a_whole_component() {
    let (mut w, hall, builder) = world_with_builder();
    let gem = described(
        &mut w,
        |b| {
            b.add(Item);
        },
        "a plain stone",
    );
    w.move_entity(gem, hall).unwrap();

    run(&mut w, builder, |c| {
        set(c, &format!("{}.description \"a gleaming gem\"", re(gem)))
    });

    assert_eq!(
        w.component_value(gem, "description"),
        Some(Value::String("a gleaming gem".into()))
    );
}

#[test]
fn set_reserves_but_rejects_field_paths() {
    let (mut w, _hall, builder) = world_with_builder();
    let out = run(&mut w, builder, |c| {
        set(c, &format!("{}.description.value \"x\"", re(builder)))
    });
    assert!(feedback(&out).iter().any(|t| t.contains("Field-level")));
    // The component was left untouched.
    assert_eq!(
        w.component_value(builder, "description"),
        Some(Value::String("a builder".into()))
    );
}

#[test]
fn set_defers_to_the_engine_guards_for_id_and_relation_tags() {
    let (mut w, _hall, builder) = world_with_builder();
    let id_out = run(&mut w, builder, |c| {
        set(c, &format!("{}.id 5", re(builder)))
    });
    assert!(feedback(&id_out).iter().any(|t| t.contains("Can't set")));

    let rel_out = run(&mut w, builder, |c| {
        set(c, &format!("{}.contained_by 1", re(builder)))
    });
    assert!(feedback(&rel_out).iter().any(|t| t.contains("Can't set")));
}

#[test]
fn destroy_removes_the_entity_and_reparents_its_contents() {
    let (mut w, hall, builder) = world_with_builder();
    let box_ = described(
        &mut w,
        |b| {
            b.add(Container);
        },
        "a box",
    );
    w.move_entity(box_, hall).unwrap();
    let coin = described(
        &mut w,
        |b| {
            b.add(Item);
        },
        "a coin",
    );
    w.move_entity(coin, box_).unwrap();

    run(&mut w, builder, |c| destroy(c, &re(box_)));

    assert!(!w.contains(box_));
    assert_eq!(w.container_of(coin), Some(hall)); // spilled up to the hall
}

#[test]
fn destroy_refuses_yourself() {
    let (mut w, _hall, builder) = world_with_builder();
    let out = run(&mut w, builder, |c| destroy(c, &re(builder)));
    assert!(
        feedback(&out)
            .iter()
            .any(|t| t.contains("destroy yourself"))
    );
    assert!(w.contains(builder));
}

#[test]
fn purge_removes_the_entity_and_its_contents() {
    let (mut w, hall, builder) = world_with_builder();
    let box_ = described(
        &mut w,
        |b| {
            b.add(Container);
        },
        "a box",
    );
    w.move_entity(box_, hall).unwrap();
    let coin = described(
        &mut w,
        |b| {
            b.add(Item);
        },
        "a coin",
    );
    w.move_entity(coin, box_).unwrap();

    run(&mut w, builder, |c| purge(c, &re(box_)));

    assert!(!w.contains(box_));
    assert!(!w.contains(coin)); // went with the box, not spilled
}

#[test]
fn purge_refuses_a_container_you_are_inside() {
    let (mut w, hall, builder) = world_with_builder();

    let out = run(&mut w, builder, |c| purge(c, &re(hall)));

    assert!(
        feedback(&out)
            .iter()
            .any(|t| t.contains("something you're inside"))
    );
    assert!(w.contains(hall));
    assert!(w.contains(builder));
}

#[test]
fn possess_wires_the_controls_edge() {
    let (mut w, _hall, builder) = world_with_builder();
    let drone = described(
        &mut w,
        |b| {
            b.add(Creature);
        },
        "a drone",
    );

    run(&mut w, builder, |c| possess(c, &re(drone)));

    assert_eq!(w.target_of::<Controls>(drone), Some(builder));
}

#[test]
fn possess_refuses_yourself() {
    let (mut w, _hall, builder) = world_with_builder();
    let out = run(&mut w, builder, |c| possess(c, &re(builder)));
    assert!(
        feedback(&out)
            .iter()
            .any(|t| t.contains("possess yourself"))
    );
    assert_eq!(w.target_of::<Controls>(builder), None);
}

#[test]
fn possess_refuses_a_target_controlled_by_another() {
    let (mut w, _hall, builder) = world_with_builder();
    let rival = described(
        &mut w,
        |b| {
            b.add(Creature);
        },
        "another handler",
    );
    let drone = described(
        &mut w,
        |b| {
            b.add(Creature);
        },
        "a drone",
    );
    w.relate::<Controls>(drone, rival).unwrap();

    let out = run(&mut w, builder, |c| possess(c, &re(drone)));

    // The edge is untouched (no silent steal), and the prior controller stands.
    assert_eq!(w.target_of::<Controls>(drone), Some(rival));
    assert!(
        feedback(&out)
            .iter()
            .any(|t| t.contains("already controlled by"))
    );
}

#[test]
fn unpossess_refuses_a_target_you_do_not_control() {
    let (mut w, _hall, builder) = world_with_builder();
    // Nothing controls it.
    let stray = described(
        &mut w,
        |b| {
            b.add(Creature);
        },
        "a stray drone",
    );
    let out = run(&mut w, builder, |c| unpossess(c, &re(stray)));
    assert!(
        feedback(&out)
            .iter()
            .any(|t| t.contains("Nothing controls"))
    );

    // Controlled, but by someone else: the edge survives, untouched.
    let rival = described(
        &mut w,
        |b| {
            b.add(Creature);
        },
        "another handler",
    );
    let drone = described(
        &mut w,
        |b| {
            b.add(Creature);
        },
        "a drone",
    );
    w.relate::<Controls>(drone, rival).unwrap();
    let out = run(&mut w, builder, |c| unpossess(c, &re(drone)));
    assert_eq!(w.target_of::<Controls>(drone), Some(rival));
    assert!(feedback(&out).iter().any(|t| t.contains("not you")));
}

#[test]
fn unpossess_removes_the_controls_edge() {
    let (mut w, _hall, builder) = world_with_builder();
    let drone = described(
        &mut w,
        |b| {
            b.add(Creature);
        },
        "a drone",
    );
    run(&mut w, builder, |c| possess(c, &re(drone)));

    run(&mut w, builder, |c| unpossess(c, &re(drone)));

    assert_eq!(w.target_of::<Controls>(drone), None);
}

#[test]
fn unpossess_clears_a_dangling_focus() {
    let (mut w, _hall, builder) = world_with_builder();
    let drone = described(
        &mut w,
        |b| {
            b.add(Creature);
        },
        "a drone",
    );
    run(&mut w, builder, |c| possess(c, &re(drone)));
    w.set_focus(builder, drone).expect("aim focus at the drone");

    run(&mut w, builder, |c| unpossess(c, &re(drone)));

    assert_eq!(w.focus_of(builder), None);
    assert_eq!(w.target_of::<Controls>(drone), None);
}

#[test]
fn unpossess_clears_a_focus_aimed_below_the_released_target() {
    // builder -> mech -> drone, with the cursor on the drone. Releasing the
    // mech detaches the whole subtree, so the drone-aimed focus, a descendant
    // of the released target, must clear too (the ancestors-walk branch).
    let (mut w, _hall, builder) = world_with_builder();
    let mech = described(
        &mut w,
        |b| {
            b.add(Creature);
        },
        "a mech",
    );
    let drone = described(
        &mut w,
        |b| {
            b.add(Creature);
        },
        "a drone",
    );
    run(&mut w, builder, |c| possess(c, &re(mech)));
    w.relate::<Controls>(drone, mech)
        .expect("drone under the mech");
    w.set_focus(builder, drone).expect("aim focus at the drone");

    run(&mut w, builder, |c| unpossess(c, &re(mech)));

    assert_eq!(w.focus_of(builder), None);
    assert_eq!(w.target_of::<Controls>(mech), None);
}
