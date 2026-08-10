//! The reference game's canonical affordance vocabulary. Definitions here own
//! applicability, advertised effects, typed execution, and post-commit narration;
//! verbs only resolve text into their typed inputs.

use musce::action::state::StateRegistry;
use musce::action::{
    Action, AffordanceRegistry, AffordanceRegistryBuilder, ExecError, NarrationCtx, PerformCtx,
    RegistryError, TypedHandlerOutcome, execute,
};
use musce::affordance;
use musce::wire::EventKind;
use musce::world::{Containment, EntityId, RelationError, World};

use crate::kinds::GiftRecipient;
use crate::names::display_name;

pub(crate) fn build(
    world: &World,
    _caps: &musce::action::CapRegistry,
) -> Result<AffordanceRegistry, RegistryError> {
    let mut state = StateRegistry::new();
    let give = Give::register(&mut state)?;
    let mut builder = AffordanceRegistryBuilder::new(state);
    builder.register_typed(give)?;
    builder.build(world)
}

affordance! {
    pub(crate) give(
        item: Entity,
        recipient: Entity,
    ) {
        requires {
            item.relation_is(Containment, Actor) => "You aren't carrying that.";
            recipient.has_component(GiftRecipient) => "You can't give things to that.";
            distinct(Actor, recipient) => "You already have it.";
            same_locus(Actor, recipient) => "You don't see them here.";
        }

        effects {
            item.set_relation(Containment, recipient);
        }

        gate Open;
        // A recipient below the held item makes the structural move cyclic;
        // transitive ancestry is intentionally outside the closed guard algebra.
        resolution Contested;
        observe GiveObservations via observe_give;
        execute execute_give;
        narrate narrate_give;
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
    match execute(
        ctx.world,
        Action::Move {
            entity: inputs.item,
            into: inputs.recipient,
        },
    ) {
        Ok(_) => TypedHandlerOutcome::committed(GiveResults {}),
        Err(ExecError::Relation(RelationError::Cycle { .. })) => {
            TypedHandlerOutcome::refused("You can't give that away.")
        }
        Err(error) => {
            tracing::error!(error = %error, "canonical give hit an unexpected structural error");
            TypedHandlerOutcome::refused("You can't give that away.")
        }
    }
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
