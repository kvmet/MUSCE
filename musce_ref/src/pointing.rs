//! The reference app's generic pointing policy. [`snapshot`] projects the
//! perceivable world; [`offers`] exposes app-selected partial canonical actions;
//! [`validate`] narrows complete client groundings before shared execution. The
//! host owns routing and canonical validation but no game vocabulary.
//!
//! Perception is the reference game's MVP rule: an actor sees its enclosing room
//! and everything nested within. Relation-backed exits are also projected under
//! the room so a pointing client can render them.
//!
//! See `docs/architecture/networking-and-sessions.md` and
//! `docs/architecture/offers.md`.

use musce::InteractionCtx;
use musce::action::schema::{
    AffordanceId, GroundAction, ParameterBinding, ParameterId, PartialGrounding, Value,
};
use musce::action::{InputCandidates, OfferProposal};
use musce::wire::{Entity, SnapshotData};
use musce::world::{Description, EntityId, Locus, World};

use crate::exits::ExitQueries;
use crate::kinds::{Container, Creature, Edible, Exit, Item, Player};
use crate::verbs::Locked;

/// Project the perceivable containment tree for `actor`, rooted at its enclosing
/// room. The actor node's contents are its inventory.
pub fn snapshot(world: &World, actor: EntityId) -> SnapshotData {
    let root = world.enclosing_locus(actor).unwrap_or(actor);
    let mut entities = Vec::new();
    collect(world, root, &mut entities);
    SnapshotData {
        root: root.0.to_string(),
        actor: actor.0.to_string(),
        entities,
    }
}

fn collect(world: &World, id: EntityId, out: &mut Vec<Entity>) {
    let mut contents = world.contents(id).to_vec();
    if world.has::<Locus>(id) {
        contents.extend_from_slice(world.exits_of(id));
    }
    out.push(Entity {
        id: id.0.to_string(),
        name: world.name_of(id).unwrap_or_else(|| "something".into()),
        kinds: kinds_of(world, id),
        contents: contents.iter().map(|child| child.0.to_string()).collect(),
        details: details_of(world, id),
    });
    for child in contents {
        collect(world, child, out);
    }
}

fn details_of(world: &World, id: EntityId) -> Vec<(String, String)> {
    world
        .get::<Description>(id)
        .map(|description| vec![("description".into(), description.0.clone())])
        .unwrap_or_default()
}

fn kinds_of(world: &World, id: EntityId) -> Vec<String> {
    [
        (world.has::<Locus>(id), "locus"),
        (world.has::<Player>(id), "player"),
        (world.has::<Creature>(id), "creature"),
        (world.has::<Container>(id), "container"),
        (world.has::<Item>(id), "item"),
        (world.has::<Exit>(id), "exit"),
        (world.has::<Edible>(id), "edible"),
        (world.has::<Locked>(id), "locked"),
    ]
    .into_iter()
    .filter(|(present, _)| *present)
    .map(|(_, tag)| tag.to_string())
    .collect()
}

/// Expose the reference app's canonical `give` action with the clicked entity
/// bound as `recipient`. The actor's current inventory supplies presentation
/// candidates for the missing `item`; the engine classifies the proposal.
pub fn offers(ctx: &InteractionCtx<'_>, clicked: EntityId) -> Vec<OfferProposal> {
    if !perceivable(ctx.world, ctx.actor(), clicked) {
        return Vec::new();
    }
    let item = ParameterId::new("item").expect("static parameter id");
    let recipient = ParameterId::new("recipient").expect("static parameter id");
    let candidates = ctx
        .world
        .contents(ctx.actor())
        .iter()
        .copied()
        .map(Value::Entity)
        .collect::<Vec<_>>();
    vec![OfferProposal::new(
        PartialGrounding::new(
            AffordanceId::new(crate::affordances::GIVE).expect("static affordance id"),
            vec![ParameterBinding::new(recipient, Value::Entity(clicked))],
        ),
        vec![InputCandidates::new(item, candidates)],
    )]
}

/// Narrow a complete, untrusted pointing grounding using reference-app exposure
/// policy. Canonical liveness, authority, and guards are enforced afterward.
pub fn validate(ctx: &InteractionCtx<'_>, action: &GroundAction) -> Result<(), String> {
    if action.affordance().as_str() != crate::affordances::GIVE {
        return Err("You can't do that.".into());
    }
    let [Value::Entity(item), Value::Entity(recipient)] = action.inputs() else {
        return Err("You can't do that.".into());
    };
    if !perceivable(ctx.world, ctx.actor(), *recipient) {
        return Err("You don't see that here.".into());
    }
    if !ctx.world.contents(ctx.actor()).contains(item) {
        return Err("You can't reach that.".into());
    }
    Ok(())
}

fn perceivable(world: &World, actor: EntityId, id: EntityId) -> bool {
    match world.enclosing_locus(actor) {
        Some(locus) => {
            world.enclosing_locus(id) == Some(locus) || world.exits_of(locus).contains(&id)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce::action::{CapRegistry, OfferStatus, Verdict};
    use musce::world::Name;
    use musce::world::hecs::EntityBuilder;

    use crate::exits::LeadsFrom;
    use crate::kinds::{Container, GiftRecipient, Item};

    struct Fixture {
        world: World,
        room: EntityId,
        actor: EntityId,
        coin: EntityId,
        recipient: EntityId,
        chest: EntityId,
        rock: EntityId,
    }

    fn spawn(world: &mut World, build: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut builder = EntityBuilder::new();
        build(&mut builder);
        world.spawn(builder)
    }

    fn fixture() -> Fixture {
        let mut world = World::new();
        crate::systems::register(&mut world);
        let room = spawn(&mut world, |builder| {
            builder.add(Locus);
            builder.add(Description("a bare room".into()));
        });
        let actor = spawn(&mut world, |builder| {
            builder.add(Player);
            builder.add(Name("an adventurer".into()));
        });
        world.move_entity(actor, room).unwrap();
        let coin = spawn(&mut world, |builder| {
            builder.add(Item);
            builder.add(Name("a copper coin".into()));
        });
        world.move_entity(coin, actor).unwrap();
        let recipient = spawn(&mut world, |builder| {
            builder.add(Creature);
            builder.add(GiftRecipient);
            builder.add(Name("a brass drone".into()));
        });
        world.move_entity(recipient, room).unwrap();
        let chest = spawn(&mut world, |builder| {
            builder.add(Container);
            builder.add(Name("a wooden chest".into()));
        });
        world.move_entity(chest, room).unwrap();
        let rock = spawn(&mut world, |builder| {
            builder.add(Item);
            builder.add(Name("a smooth rock".into()));
            builder.add(Description("a smooth grey rock, worn round".into()));
        });
        world.move_entity(rock, room).unwrap();
        Fixture {
            world,
            room,
            actor,
            coin,
            recipient,
            chest,
            rock,
        }
    }

    fn node(snapshot: &SnapshotData, id: EntityId) -> &Entity {
        snapshot
            .entities
            .iter()
            .find(|entity| entity.id == id.0.to_string())
            .expect("entity in snapshot")
    }

    #[test]
    fn snapshot_projects_room_inventory_kinds_and_details() {
        let fixture = fixture();
        let snapshot = snapshot(&fixture.world, fixture.actor);
        assert_eq!(snapshot.root, fixture.room.0.to_string());
        assert_eq!(snapshot.actor, fixture.actor.0.to_string());
        assert_eq!(
            node(&snapshot, fixture.actor).contents,
            vec![fixture.coin.0.to_string()]
        );
        assert!(
            node(&snapshot, fixture.chest)
                .kinds
                .contains(&"container".to_string())
        );
        assert_eq!(
            node(&snapshot, fixture.rock).details,
            vec![(
                "description".to_string(),
                "a smooth grey rock, worn round".to_string()
            )]
        );
    }

    #[test]
    fn snapshot_projects_relation_backed_exits_under_the_room() {
        let mut fixture = fixture();
        let exit = spawn(&mut fixture.world, |builder| {
            builder.add(Exit);
            builder.add(Name("east".into()));
        });
        fixture
            .world
            .relate::<LeadsFrom>(exit, fixture.room)
            .unwrap();

        let snapshot = snapshot(&fixture.world, fixture.actor);
        assert!(
            node(&snapshot, fixture.room)
                .contents
                .contains(&exit.0.to_string())
        );
        assert!(perceivable(&fixture.world, fixture.actor, exit));
    }

    #[test]
    fn app_proposes_partial_give_and_engine_classifies_it() {
        let fixture = fixture();
        let caps = CapRegistry::new();
        let registry = crate::affordances::build(&fixture.world, &caps).unwrap();
        let verdict = Verdict::guest();
        let ctx = InteractionCtx::new(&fixture.world, &registry, fixture.actor, &verdict);

        let proposal = offers(&ctx, fixture.recipient).pop().unwrap();
        assert_eq!(proposal.candidates().len(), 1);
        assert_eq!(
            proposal.candidates()[0].values(),
            &[Value::Entity(fixture.coin)]
        );
        let classified = registry
            .classify_offer(&fixture.world, &verdict, fixture.actor, proposal)
            .unwrap();
        assert!(matches!(
            classified.status(),
            OfferStatus::Needs { parameters }
                if parameters.iter().map(ParameterId::as_str).eq(["item"])
        ));

        let chest = offers(&ctx, fixture.chest).pop().unwrap();
        let classified = registry
            .classify_offer(&fixture.world, &verdict, fixture.actor, chest)
            .unwrap();
        assert!(matches!(
            classified.status(),
            OfferStatus::Vetoed { reason } if &**reason == "You can't give things to that."
        ));
    }

    #[test]
    fn app_validation_accepts_only_exposed_reachable_groundings() {
        let mut fixture = fixture();
        let caps = CapRegistry::new();
        let registry = crate::affordances::build(&fixture.world, &caps).unwrap();
        let verdict = Verdict::guest();
        let ctx = InteractionCtx::new(&fixture.world, &registry, fixture.actor, &verdict);
        assert!(
            validate(
                &ctx,
                &crate::affordances::give_action(fixture.actor, fixture.coin, fixture.recipient,),
            )
            .is_ok()
        );
        assert_eq!(
            validate(
                &ctx,
                &crate::affordances::give_action(fixture.actor, fixture.rock, fixture.recipient,),
            ),
            Err("You can't reach that.".into())
        );

        let elsewhere = spawn(&mut fixture.world, |builder| {
            builder.add(Locus);
        });
        let far_recipient = spawn(&mut fixture.world, |builder| {
            builder.add(Creature);
            builder.add(GiftRecipient);
        });
        fixture.world.move_entity(far_recipient, elsewhere).unwrap();
        let ctx = InteractionCtx::new(&fixture.world, &registry, fixture.actor, &verdict);
        assert!(offers(&ctx, far_recipient).is_empty());
        assert_eq!(
            validate(
                &ctx,
                &crate::affordances::give_action(fixture.actor, fixture.coin, far_recipient),
            ),
            Err("You don't see that here.".into())
        );
    }
}
