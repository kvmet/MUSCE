//! The pointing web client's read projections: the world into a wire snapshot,
//! and a clicked entity into its affordance offers, both as `musce_proto` DTOs the
//! read query replies carry. These are the game side of the `Game.snapshot` and
//! `Game.offers` seams: the engine routes a read query to them and serializes the
//! result, holding no game vocabulary itself. Names, kinds, and the affordance set
//! are all game knowledge, which is why the projection lives here.
//!
//! Perception is the MVP rule the rest of the reference game already uses: an actor
//! sees its enclosing room and everything nested within (co-located implies known,
//! the same seed as `crate::agency::known_here`). A closed-container or
//! line-of-sight refinement would narrow `collect` here without touching the wire.
//!
//! See `docs/architecture/networking-and-sessions.md` and
//! `docs/architecture/offers.md`.

use musce::wire::{Entity, Offer, OfferStatus, Role, SnapshotData};
use musce::world::{Description, EntityId, Locus, World};

use crate::kinds::{Container, Creature, Edible, Exit, Item, Player};
use crate::offers::{self, affordances_on};
use crate::verbs::Locked;

/// Project the perceivable containment tree for `actor`: rooted at its enclosing
/// room, every entity nested within (including, as the actor's own contents, its
/// inventory). The wire snapshot the `Query::Snapshot` reply carries.
pub fn snapshot(world: &World, actor: EntityId) -> SnapshotData {
    let root = world.enclosing_locus(actor).unwrap_or(actor);
    let mut entities = Vec::new();
    collect(world, root, &mut entities);
    SnapshotData {
        root: root.0,
        actor: actor.0,
        entities,
    }
}

/// Walk containment depth-first from `id`, pushing one [`Entity`] per node.
/// Containment is acyclic, so this terminates without a visited set.
fn collect(world: &World, id: EntityId, out: &mut Vec<Entity>) {
    let contents = world.contents(id);
    out.push(Entity {
        id: id.0,
        name: world.name_of(id).unwrap_or_else(|| "something".into()),
        kinds: kinds_of(world, id),
        contents: contents.iter().map(|c| c.0).collect(),
        details: details_of(world, id),
    });
    for child in contents {
        collect(world, child, out);
    }
}

/// The passive detail an actor perceives about an entity by presence, as ordered
/// `(label, value)` pairs. Game vocabulary, like `kinds_of`: the reference game
/// exposes an entity's `Description`, the same prose a narrated `examine` reveals,
/// delivered silently so a click renders without a second round-trip.
fn details_of(world: &World, id: EntityId) -> Vec<(String, String)> {
    let mut details = Vec::new();
    if let Some(desc) = world.get::<Description>(id) {
        details.push(("description".to_string(), desc.0.clone()));
    }
    details
}

/// The game kind tags on an entity, the same vocabulary a text client's prose
/// implies. Probed by the game's own kind markers; the engine has no notion of what
/// a "container" is.
fn kinds_of(world: &World, id: EntityId) -> Vec<String> {
    let mut kinds = Vec::new();
    for (present, tag) in [
        (world.has::<Locus>(id), "locus"),
        (world.has::<Player>(id), "player"),
        (world.has::<Creature>(id), "creature"),
        (world.has::<Container>(id), "container"),
        (world.has::<Item>(id), "item"),
        (world.has::<Exit>(id), "exit"),
        (world.has::<Edible>(id), "edible"),
        (world.has::<Locked>(id), "locked"),
    ] {
        if present {
            kinds.push(tag.to_string());
        }
    }
    kinds
}

/// The affordances available on `clicked` for `actor`, in wire form. Delegates the
/// classification to [`affordances_on`] and maps its statuses to the serde DTOs.
pub fn offers(world: &World, actor: EntityId, clicked: EntityId) -> Vec<Offer> {
    affordances_on(world, actor, clicked)
        .into_iter()
        .map(|o| Offer {
            name: o.name,
            status: to_wire(o.status),
        })
        .collect()
}

fn to_wire(status: offers::OfferStatus) -> OfferStatus {
    match status {
        offers::OfferStatus::Available => OfferStatus::Available,
        offers::OfferStatus::Vetoed(reason) => OfferStatus::Vetoed {
            reason: reason.to_string(),
        },
        offers::OfferStatus::NeedsRole(role) => OfferStatus::NeedsRole {
            role: to_wire_role(role),
        },
    }
}

fn to_wire_role(role: offers::Role) -> Role {
    match role {
        offers::Role::Object => Role::Object,
        offers::Role::Target => Role::Target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce::world::hecs::EntityBuilder;
    use musce::world::{Description, Name};

    use crate::kinds::{Container, Item};

    struct Fixture {
        world: World,
        room: EntityId,
        actor: EntityId,
        coin: EntityId,
        chest: EntityId,
        rock: EntityId,
        gate: EntityId,
    }

    /// A room with the actor holding a coin, plus a chest, a takeable rock, and a
    /// locked gate. Registered as at boot so kind tags read by name.
    fn fixture() -> Fixture {
        let mut world = World::new();
        crate::systems::register(&mut world);

        let room = spawn(&mut world, |b| {
            b.add(Locus);
            b.add(Description("a bare room".into()));
        });
        let actor = spawn(&mut world, |b| {
            b.add(Player);
            b.add(Name("an adventurer".into()));
        });
        world.move_entity(actor, room).unwrap();
        let coin = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Name("a copper coin".into()));
        });
        world.move_entity(coin, actor).unwrap();
        let chest = spawn(&mut world, |b| {
            b.add(Container);
            b.add(Name("a wooden chest".into()));
        });
        world.move_entity(chest, room).unwrap();
        let rock = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Name("a smooth rock".into()));
            b.add(Description("a smooth grey rock, worn round".into()));
        });
        world.move_entity(rock, room).unwrap();
        let gate = spawn(&mut world, |b| {
            b.add(Exit);
            b.add(Locked);
            b.add(Name("north".into()));
        });
        world.move_entity(gate, room).unwrap();

        Fixture {
            world,
            room,
            actor,
            coin,
            chest,
            rock,
            gate,
        }
    }

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    fn node(snap: &SnapshotData, id: EntityId) -> &Entity {
        snap.entities
            .iter()
            .find(|e| e.id == id.0)
            .expect("entity in snapshot")
    }

    #[test]
    fn snapshot_roots_at_the_room_and_carries_the_actor() {
        let f = fixture();
        let snap = snapshot(&f.world, f.actor);
        assert_eq!(snap.root, f.room.0);
        assert_eq!(snap.actor, f.actor.0);
        // Every entity in the room is present, including the actor and its held coin.
        for id in [f.room, f.actor, f.coin, f.chest, f.rock, f.gate] {
            assert!(
                snap.entities.iter().any(|e| e.id == id.0),
                "missing entity {}",
                id.0
            );
        }
    }

    #[test]
    fn inventory_is_the_actors_contents_in_the_snapshot() {
        // The reason there is no separate inventory query: the held coin is a child
        // of the actor node.
        let f = fixture();
        let snap = snapshot(&f.world, f.actor);
        assert_eq!(node(&snap, f.actor).contents, vec![f.coin.0]);
    }

    #[test]
    fn kinds_project_the_game_vocabulary() {
        let f = fixture();
        let snap = snapshot(&f.world, f.actor);
        assert!(node(&snap, f.room).kinds.contains(&"locus".to_string()));
        assert!(
            node(&snap, f.chest)
                .kinds
                .contains(&"container".to_string())
        );
        let gate = &node(&snap, f.gate).kinds;
        assert!(gate.contains(&"exit".to_string()));
        assert!(gate.contains(&"locked".to_string()));
    }

    #[test]
    fn details_carry_the_passive_description() {
        // Each node carries the game-projected passive detail: its Description, the
        // prose a narrated examine would reveal, delivered silently in the read. A
        // node with no Description carries an empty bag, not a missing entry.
        let f = fixture();
        let snap = snapshot(&f.world, f.actor);
        assert_eq!(
            node(&snap, f.room).details,
            vec![("description".to_string(), "a bare room".to_string())]
        );
        assert_eq!(
            node(&snap, f.rock).details,
            vec![(
                "description".to_string(),
                "a smooth grey rock, worn round".to_string()
            )]
        );
        assert!(node(&snap, f.actor).details.is_empty());
    }

    #[test]
    fn offers_convert_to_the_wire_statuses() {
        let f = fixture();
        let put = offers(&f.world, f.actor, f.chest)
            .into_iter()
            .find(|o| o.name == "put")
            .unwrap();
        assert!(matches!(
            put.status,
            OfferStatus::NeedsRole { role: Role::Object }
        ));

        let take = offers(&f.world, f.actor, f.rock)
            .into_iter()
            .find(|o| o.name == "take")
            .unwrap();
        assert!(matches!(take.status, OfferStatus::Available));

        let go = offers(&f.world, f.actor, f.gate)
            .into_iter()
            .find(|o| o.name == "go")
            .unwrap();
        assert!(matches!(go.status, OfferStatus::Vetoed { reason } if reason == "It's locked."));
    }
}
