//! The reference game's canonical affordance vocabulary. Definitions here own
//! applicability, advertised effects, typed execution, and post-commit narration;
//! callers only resolve app intent into typed inputs.

use musce::action::schema::GroundAction;
use musce::action::state::StateRegistry;
use musce::action::{
    Action, AffordanceRegistry, AffordanceRegistryBuilder, Ctx, ExecError, NarrationCtx,
    PerformCtx, PerformOutcome, Refusal, RegistryError, TypedHandlerOutcome, execute,
};
use musce::affordance;
use musce::wire::EventKind;
use musce::world::{Containment, EntityId, Locus, RelationError, World};

use crate::consume::Fed;
use crate::exits::{LeadsFrom, LeadsTo};
use crate::kinds::{Container, Creature, Edible, Exit, GiftRecipient, Player, Shiny};
use crate::names::display_name;
use crate::verbs::Locked;
use crate::verbs::describe_room;

pub(crate) fn build(
    world: &World,
    _caps: &musce::action::CapRegistry,
) -> Result<AffordanceRegistry, RegistryError> {
    let mut state = StateRegistry::new();
    // Goal-only vocabulary belongs in the same app registry even when no
    // affordance guard or effect mentions it directly.
    state.register_component::<Shiny>()?;
    let take = Take::register(&mut state)?;
    let drop = Drop::register(&mut state)?;
    let put = Put::register(&mut state)?;
    let eat = Eat::register(&mut state)?;
    let give = Give::register(&mut state)?;
    let go = Go::register(&mut state)?;
    let mut builder = AffordanceRegistryBuilder::new(state);
    builder.register_typed(take)?;
    builder.register_typed(drop)?;
    builder.register_typed(put)?;
    builder.register_typed(eat)?;
    builder.register_typed(give)?;
    builder.register_typed(go)?;
    builder.build(world)
}

/// Perform a command's already-grounded app affordance and turn ordinary
/// refusals into player feedback. Contract errors remain loud and receive only a
/// stable fallback line; callers cannot replace or suppress committed narration.
pub(crate) fn perform_command(ctx: &mut Ctx<'_>, action: &GroundAction, fallback: &'static str) {
    match ctx.perform(action) {
        Ok(PerformOutcome::Committed(_)) => {}
        Ok(PerformOutcome::Refused(Refusal::Guard { reason, .. }))
        | Ok(PerformOutcome::Refused(Refusal::Resolution { reason })) => {
            ctx.emit_self(EventKind::Feedback, reason);
        }
        Ok(PerformOutcome::Refused(Refusal::Gate)) => {
            ctx.emit_self(EventKind::Feedback, "You aren't allowed to do that.");
        }
        Err(error) => {
            tracing::error!(error = %error, affordance = %action.affordance(), "canonical action failed");
            ctx.emit_self(EventKind::Feedback, fallback);
        }
    }
}

affordance! {
    pub(crate) take(item: Entity) {
        requires {
            all {
                not(item.has_component(Locus));
                not(item.has_component(Player));
                not(item.has_component(Creature));
            } => "You can't take that.";
            same_locus(Actor, item) => "You don't see that here.";
        }
        effects { item.set_relation(Containment, Actor); }
        gate Open;
        resolution Contested;
        observe ItemObservations via observe_item;
        execute execute_take;
        narrate narrate_take;
    }
}

affordance! {
    pub(crate) drop(item: Entity, destination: Entity) {
        requires {
            item.relation_is(Containment, Actor) => "You aren't carrying that.";
            Actor.at_locus(destination) => "There is nowhere to drop it.";
        }
        effects { item.set_relation(Containment, destination); }
        gate Open;
        resolution Deterministic;
        observe ItemObservations via observe_drop;
        execute execute_drop;
        narrate narrate_drop;
    }
}

affordance! {
    pub(crate) put(item: Entity, container: Entity) {
        requires {
            item.relation_is(Containment, Actor) => "You aren't carrying that.";
            container.has_component(Container) => "You can't put things in that.";
            same_locus(Actor, container) => "You don't see that here.";
        }
        effects { item.set_relation(Containment, container); }
        gate Open;
        resolution Contested;
        observe PutObservations via observe_put;
        execute execute_put;
        narrate narrate_put;
    }
}

affordance! {
    pub(crate) eat(food: Entity) {
        requires {
            all {
                food.relation_is(Containment, Actor);
                food.has_component(Edible);
            } => "You have nothing edible to eat.";
        }
        effects {
            food.remove_component(Edible);
            Actor.set_component(Fed);
        }
        gate Open;
        resolution Deterministic;
        observe ItemObservations via observe_food;
        execute execute_eat;
        narrate narrate_eat;
    }
}

affordance! {
    pub(crate) give(item: Entity, recipient: Entity) {
        requires {
            item.relation_is(Containment, Actor) => "You aren't carrying that.";
            recipient.has_component(GiftRecipient) => "You can't give things to that.";
            distinct(Actor, recipient) => "You already have it.";
            same_locus(Actor, recipient) => "You don't see them here.";
        }
        effects { item.set_relation(Containment, recipient); }
        gate Open;
        resolution Contested;
        observe GiveObservations via observe_give;
        execute execute_give;
        narrate narrate_give;
    }
}

affordance! {
    pub(crate) go(exit: Entity, destination: Entity) {
        requires {
            exit.has_component(Exit) => "You can't go that way.";
            exit.relation_is(LeadsTo, destination) => "You can't go that way.";
            exists(origin: Entity) {
                Actor.at_locus(origin);
                exit.relation_is(LeadsFrom, origin);
            } => "You can't go that way.";
            not(exit.has_component(Locked)) => "It's locked.";
            destination.has_component(Locus) => "You can't go that way.";
        }
        effects { Actor.set_locus(destination); }
        gate Open;
        resolution Contested;
        observe GoObservations via observe_go;
        execute execute_go;
        narrate narrate_go;
    }
}

pub(crate) struct ItemObservations {
    item: String,
    actor: String,
    locus: Option<EntityId>,
}

fn observe_item(world: &World, actor: EntityId, inputs: &TakeInputs) -> ItemObservations {
    item_observations(world, actor, inputs.item)
}

fn observe_drop(world: &World, actor: EntityId, inputs: &DropInputs) -> ItemObservations {
    item_observations(world, actor, inputs.item)
}

fn observe_food(world: &World, actor: EntityId, inputs: &EatInputs) -> ItemObservations {
    item_observations(world, actor, inputs.food)
}

fn item_observations(world: &World, actor: EntityId, item: EntityId) -> ItemObservations {
    ItemObservations {
        item: display_name(world, item),
        actor: display_name(world, actor),
        locus: world.enclosing_locus(actor),
    }
}

fn execute_move<R>(
    ctx: &mut PerformCtx<'_>,
    entity: EntityId,
    into: EntityId,
    results: R,
    refusal: &'static str,
) -> TypedHandlerOutcome<R> {
    match execute(ctx.world, Action::Move { entity, into }) {
        Ok(_) => TypedHandlerOutcome::committed(results),
        Err(ExecError::Relation(RelationError::Cycle { .. })) => {
            TypedHandlerOutcome::refused(refusal)
        }
        Err(error) => {
            tracing::error!(error = %error, "canonical movement hit an unexpected structural error");
            TypedHandlerOutcome::refused(refusal)
        }
    }
}

fn execute_take(ctx: &mut PerformCtx<'_>, inputs: &TakeInputs) -> TypedHandlerOutcome<TakeResults> {
    execute_move(
        ctx,
        inputs.item,
        ctx.actor(),
        TakeResults {},
        "You can't take that.",
    )
}

fn execute_drop(ctx: &mut PerformCtx<'_>, inputs: &DropInputs) -> TypedHandlerOutcome<DropResults> {
    execute_move(
        ctx,
        inputs.item,
        inputs.destination,
        DropResults {},
        "You can't drop that.",
    )
}

fn execute_put(ctx: &mut PerformCtx<'_>, inputs: &PutInputs) -> TypedHandlerOutcome<PutResults> {
    execute_move(
        ctx,
        inputs.item,
        inputs.container,
        PutResults {},
        "You can't put that there.",
    )
}

fn execute_eat(ctx: &mut PerformCtx<'_>, inputs: &EatInputs) -> TypedHandlerOutcome<EatResults> {
    ctx.world
        .remove::<Edible>(inputs.food)
        .expect("the canonical eat guard names live edible food");
    ctx.world
        .insert(ctx.actor(), Fed)
        .expect("the canonical performer supplies a live actor");
    TypedHandlerOutcome::committed(EatResults {})
}

fn narrate_item(
    ctx: &mut NarrationCtx<'_>,
    observations: &ItemObservations,
    first_verb: &str,
    third_verb: &str,
) {
    ctx.emit_self(
        EventKind::Feedback,
        format!("You {first_verb} {}.", observations.item),
    );
    if let Some(locus) = observations.locus {
        ctx.emit_locus_except(
            locus,
            EventKind::Narration,
            format!("{} {third_verb} {}.", observations.actor, observations.item),
            &[ctx.actor()],
        );
    }
}

fn narrate_take(
    ctx: &mut NarrationCtx<'_>,
    _inputs: &TakeInputs,
    _results: &TakeResults,
    observations: &ItemObservations,
) {
    narrate_item(ctx, observations, "take", "takes");
}

fn narrate_drop(
    ctx: &mut NarrationCtx<'_>,
    _inputs: &DropInputs,
    _results: &DropResults,
    observations: &ItemObservations,
) {
    narrate_item(ctx, observations, "drop", "drops");
}

fn narrate_eat(
    ctx: &mut NarrationCtx<'_>,
    _inputs: &EatInputs,
    _results: &EatResults,
    observations: &ItemObservations,
) {
    narrate_item(ctx, observations, "eat", "eats");
}

pub(crate) struct PutObservations {
    item: String,
    container: String,
    actor: String,
    locus: Option<EntityId>,
}

fn observe_put(world: &World, actor: EntityId, inputs: &PutInputs) -> PutObservations {
    PutObservations {
        item: display_name(world, inputs.item),
        container: display_name(world, inputs.container),
        actor: display_name(world, actor),
        locus: world.enclosing_locus(actor),
    }
}

fn narrate_put(
    ctx: &mut NarrationCtx<'_>,
    _inputs: &PutInputs,
    _results: &PutResults,
    observations: &PutObservations,
) {
    ctx.emit_self(
        EventKind::Feedback,
        format!(
            "You put {} in {}.",
            observations.item, observations.container
        ),
    );
    if let Some(locus) = observations.locus {
        ctx.emit_locus_except(
            locus,
            EventKind::Narration,
            format!(
                "{} puts {} in {}.",
                observations.actor, observations.item, observations.container
            ),
            &[ctx.actor()],
        );
    }
}

pub(crate) struct GiveObservations {
    item: String,
    recipient: String,
    actor: String,
    locus: Option<EntityId>,
}

fn observe_give(world: &World, actor: EntityId, inputs: &GiveInputs) -> GiveObservations {
    GiveObservations {
        item: display_name(world, inputs.item),
        recipient: display_name(world, inputs.recipient),
        actor: display_name(world, actor),
        locus: world.enclosing_locus(actor),
    }
}

fn execute_give(ctx: &mut PerformCtx<'_>, inputs: &GiveInputs) -> TypedHandlerOutcome<GiveResults> {
    execute_move(
        ctx,
        inputs.item,
        inputs.recipient,
        GiveResults {},
        "You can't give that away.",
    )
}

fn narrate_give(
    ctx: &mut NarrationCtx<'_>,
    inputs: &GiveInputs,
    _results: &GiveResults,
    observations: &GiveObservations,
) {
    ctx.emit_self(
        EventKind::Feedback,
        format!(
            "You give {} to {}.",
            observations.item, observations.recipient
        ),
    );
    ctx.emit_entity(
        inputs.recipient,
        EventKind::Narration,
        format!("{} gives you {}.", observations.actor, observations.item),
    );
    if let Some(locus) = observations.locus {
        ctx.emit_locus_except(
            locus,
            EventKind::Narration,
            format!(
                "{} gives {} to {}.",
                observations.actor, observations.item, observations.recipient
            ),
            &[ctx.actor(), inputs.recipient],
        );
    }
}

pub(crate) struct GoObservations {
    actor: String,
    direction: String,
    origin: Option<EntityId>,
}

fn observe_go(world: &World, actor: EntityId, inputs: &GoInputs) -> GoObservations {
    GoObservations {
        actor: display_name(world, actor),
        direction: world.name_of(inputs.exit).unwrap_or_else(|| "away".into()),
        origin: world.enclosing_locus(actor),
    }
}

fn execute_go(ctx: &mut PerformCtx<'_>, inputs: &GoInputs) -> TypedHandlerOutcome<GoResults> {
    execute_move(
        ctx,
        ctx.actor(),
        inputs.destination,
        GoResults {},
        "Something blocks the way.",
    )
}

fn narrate_go(
    ctx: &mut NarrationCtx<'_>,
    inputs: &GoInputs,
    _results: &GoResults,
    observations: &GoObservations,
) {
    if let Some(origin) = observations.origin {
        ctx.emit_locus_except(
            origin,
            EventKind::Narration,
            format!("{} leaves {}.", observations.actor, observations.direction),
            &[ctx.actor()],
        );
    }
    ctx.emit_locus_except(
        inputs.destination,
        EventKind::Narration,
        format!("{} arrives.", observations.actor),
        &[ctx.actor()],
    );
    ctx.emit_self(
        EventKind::Feedback,
        format!("You go {}.", observations.direction),
    );
    if let Some(description) = describe_room(ctx.world, ctx.actor()) {
        ctx.emit_self(EventKind::Narration, description);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exits::{LeadsFrom, LeadsTo};
    use crate::kinds::{Exit, Item, Player};
    use musce::action::{Outbound, SystemCtx, Verdict};
    use musce::world::hecs::EntityBuilder;
    use musce::world::{Description, Locus, Name};
    use std::time::SystemTime;

    struct Fixture {
        world: World,
        actor: EntityId,
        item: EntityId,
        origin: EntityId,
        destination: EntityId,
        elsewhere: EntityId,
        exit: EntityId,
    }

    fn spawn(world: &mut World, build: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut entity = EntityBuilder::new();
        build(&mut entity);
        world.spawn(entity)
    }

    fn fixture() -> Fixture {
        let mut world = World::new();
        crate::systems::register(&mut world);
        let origin = spawn(&mut world, |entity| {
            entity.add(Locus);
            entity.add(Description("origin".into()));
        });
        let destination = spawn(&mut world, |entity| {
            entity.add(Locus);
            entity.add(Description("destination".into()));
        });
        let elsewhere = spawn(&mut world, |entity| {
            entity.add(Locus);
            entity.add(Description("elsewhere".into()));
        });
        let actor = spawn(&mut world, |entity| {
            entity.add(Player);
            entity.add(Name("an actor".into()));
        });
        world.move_entity(actor, origin).unwrap();
        let item = spawn(&mut world, |entity| {
            entity.add(Item);
            entity.add(Name("an item".into()));
        });
        world.move_entity(item, origin).unwrap();
        let exit = spawn(&mut world, |entity| {
            entity.add(Exit);
            entity.add(Name("north".into()));
        });
        world.relate::<LeadsFrom>(exit, origin).unwrap();
        world.relate::<LeadsTo>(exit, destination).unwrap();
        Fixture {
            world,
            actor,
            item,
            origin,
            destination,
            elsewhere,
            exit,
        }
    }

    fn perform(world: &mut World, action: &GroundAction) -> PerformOutcome {
        let registry = build(world, &musce::action::CapRegistry::new()).unwrap();
        let mut out: Vec<Outbound> = Vec::new();
        let mut ctx = SystemCtx::new(world, &registry, 1, SystemTime::UNIX_EPOCH, &[], &mut out);
        ctx.perform(&Verdict::guest(), action).unwrap()
    }

    #[test]
    fn direct_take_cannot_bypass_the_perception_boundary() {
        let mut fixture = fixture();
        fixture
            .world
            .move_entity(fixture.item, fixture.elsewhere)
            .unwrap();
        let action = take_action(fixture.actor, fixture.item);

        assert!(matches!(
            perform(&mut fixture.world, &action),
            PerformOutcome::Refused(Refusal::Guard { index: 1, .. })
        ));
        assert_eq!(
            fixture.world.container_of(fixture.item),
            Some(fixture.elsewhere)
        );
    }

    #[test]
    fn direct_drop_cannot_forge_its_destination() {
        let mut fixture = fixture();
        fixture
            .world
            .move_entity(fixture.item, fixture.actor)
            .unwrap();
        let action = drop_action(fixture.actor, fixture.item, fixture.destination);

        assert!(matches!(
            perform(&mut fixture.world, &action),
            PerformOutcome::Refused(Refusal::Guard { index: 1, .. })
        ));
        assert_eq!(
            fixture.world.container_of(fixture.item),
            Some(fixture.actor)
        );
    }

    #[test]
    fn direct_go_cannot_forge_destination_or_origin() {
        let mut fixture = fixture();
        let wrong_destination = go_action(fixture.actor, fixture.exit, fixture.elsewhere);
        assert!(matches!(
            perform(&mut fixture.world, &wrong_destination),
            PerformOutcome::Refused(Refusal::Guard { index: 1, .. })
        ));
        assert_eq!(
            fixture.world.enclosing_locus(fixture.actor),
            Some(fixture.origin)
        );

        fixture
            .world
            .move_entity(fixture.actor, fixture.destination)
            .unwrap();
        let wrong_origin = go_action(fixture.actor, fixture.exit, fixture.destination);
        assert!(matches!(
            perform(&mut fixture.world, &wrong_origin),
            PerformOutcome::Refused(Refusal::Guard { index: 2, .. })
        ));
        assert_eq!(
            fixture.world.enclosing_locus(fixture.actor),
            Some(fixture.destination)
        );
    }
}
