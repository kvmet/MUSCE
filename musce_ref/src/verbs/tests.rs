//! Verb handler tests, split by concern to mirror the source module layout
//! (`observe`, `manipulate`, `movement`, `social`, `control`) plus the agency
//! `perform`/affordance-guard tests. The shared world fixture and the
//! outbound-buffer readers live here; each submodule pulls them in with
//! `use super::*`.

use super::help;
use crate::exits::{LeadsFrom, LeadsTo};
use crate::kinds::{Exit, Item, Player};
use musce::action::{Audience, Caller, Ctx, Outbound, Verdict};
use musce::wire::ConnectionId;
use musce::world::hecs::EntityBuilder;
use musce::world::{Description, EntityId, Locus, Name, World};

mod agency;
mod control;
mod manipulate;
mod movement;
mod observe;
mod social;

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
    crate::systems::register(&mut world);

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
    let affordances = musce::action::AffordanceRegistry::empty(world).unwrap();
    let mut out = Vec::new();
    let verdict = Verdict::guest();
    let mut ctx = Ctx::new(
        world,
        &affordances,
        Caller::new(actor, ConnectionId(1), &verdict),
        &mut out,
    );
    f(&mut ctx);
    out
}

/// The lines directed at a specific party, not broadcast to a locus. A handler's
/// first-person feedback is entity-addressed now (the narrating perform emits
/// `to_entity(actor)` so an NPC's self-line reaches no one), so both a
/// connection-addressed reply and an entity-addressed one count as directed
/// output here.
fn self_feedback(out: &[Outbound]) -> Vec<String> {
    out.iter()
        .filter(|o| matches!(o.event.to, Audience::Connection(_) | Audience::Entity(_)))
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
fn help_lists_in_world_verbs() {
    let mut f = fixture();
    let out = run(&mut f.world, f.actor, |c| help(c, ""));

    let text = &self_feedback(&out)[0];
    assert!(text.contains("look"));
    assert!(text.contains("say"));
    assert!(room_narration(&out).is_empty()); // pure feedback, no broadcast
}
