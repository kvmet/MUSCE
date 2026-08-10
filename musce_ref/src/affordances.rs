//! The reference game's canonical affordance vocabulary. Definitions here own
//! applicability, advertised effects, typed execution, and post-commit narration;
//! verbs only resolve text into their typed inputs.

use musce::action::schema::{
    ActionOutcome, AffordanceId, AffordanceSchema, Condition, Effect, Formula, GroundAction, Guard,
    Local, LocalId, OptionalEntity, Parameter, ParameterId, ParameterMode, Resolution, Term, Value,
    ValueSort,
};
use musce::action::state::StateRegistry;
use musce::action::{
    Action, AdapterError, AffordanceDefinition, AffordanceRegistry, AffordanceRegistryBuilder,
    ExecError, Gate, NarrationCtx, PerformCtx, RegistryError, TypedHandlerOutcome, execute,
};
use musce::wire::EventKind;
use musce::world::{Containment, EntityId, RelationError, World};

use crate::kinds::GiftRecipient;
use crate::names::display_name;

pub(crate) const GIVE: &str = "give";

pub(crate) fn build(
    world: &World,
    _caps: &musce::action::CapRegistry,
) -> Result<AffordanceRegistry, RegistryError> {
    let mut state = StateRegistry::new();
    let containment = state.register_relation::<Containment>()?;
    let recipient = state.register_component::<GiftRecipient>()?;
    let mut builder = AffordanceRegistryBuilder::new(state);
    builder.register_typed(Give {
        containment,
        recipient,
    })?;
    builder.build(world)
}

pub(crate) fn give_action(actor: EntityId, item: EntityId, recipient: EntityId) -> GroundAction {
    GroundAction::new(
        AffordanceId::new(GIVE).expect("static affordance id"),
        actor,
        vec![Value::Entity(item), Value::Entity(recipient)],
    )
}

struct Give {
    containment: musce::action::schema::RelationId,
    recipient: musce::action::schema::ComponentId,
}

#[derive(Clone, Copy)]
struct GiveInputs {
    item: EntityId,
    recipient: EntityId,
}

struct GiveObservations {
    item: String,
    recipient: String,
    actor: String,
    locus: Option<EntityId>,
}

impl AffordanceDefinition for Give {
    type Inputs = GiveInputs;
    type Results = ();
    type Observations = GiveObservations;

    fn schema(&self) -> AffordanceSchema {
        let item = ParameterId::new("item").expect("static parameter id");
        let recipient = ParameterId::new("recipient").expect("static parameter id");
        let locus = LocalId::new("locus").expect("static local id");
        AffordanceSchema::new(
            AffordanceId::new(GIVE).expect("static affordance id"),
            "Give",
            vec![
                Parameter::new(
                    item.clone(),
                    "item",
                    ValueSort::Entity,
                    ParameterMode::Input,
                    0,
                )
                .expect("static parameter"),
                Parameter::new(
                    recipient.clone(),
                    "recipient",
                    ValueSort::Entity,
                    ParameterMode::Input,
                    1,
                )
                .expect("static parameter"),
            ],
            vec![
                Guard::new(
                    Formula::all(vec![Condition::RelationTarget {
                        source: Term::Input(item.clone()),
                        relation: self.containment.clone(),
                        target: OptionalEntity::Is(Term::Actor),
                    }]),
                    "You aren't carrying that.",
                ),
                Guard::new(
                    Formula::all(vec![Condition::ComponentPresent {
                        entity: Term::Input(recipient.clone()),
                        component: self.recipient.clone(),
                        present: true,
                    }]),
                    "You can't give things to that.",
                ),
                Guard::new(
                    Formula::all(vec![Condition::Distinct {
                        left: Term::Actor,
                        right: Term::Input(recipient.clone()),
                    }]),
                    "You already have it.",
                ),
                Guard::new(
                    Formula::new(
                        vec![Local::new(locus.clone(), ValueSort::Entity)],
                        vec![
                            Condition::LocusOf {
                                entity: Term::Actor,
                                locus: OptionalEntity::Is(Term::Local(locus.clone())),
                            },
                            Condition::LocusOf {
                                entity: Term::Input(recipient.clone()),
                                locus: OptionalEntity::Is(Term::Local(locus)),
                            },
                        ],
                    ),
                    "You don't see them here.",
                ),
            ],
            vec![Effect::SetRelation {
                source: Term::Input(item),
                relation: self.containment.clone(),
                target: Term::Input(recipient),
            }],
            Gate::Open,
            // An adversarial world can put the recipient below the held item, making
            // the structural move cyclic. The closed guards cannot express
            // transitive ancestry, so that reachable refusal is declared honestly.
            Resolution::Contested,
        )
    }

    fn decode_inputs(&self, action: &GroundAction) -> Result<Self::Inputs, AdapterError> {
        let [item, recipient] = action.inputs() else {
            return Err(AdapterError::new("give expected item and recipient inputs"));
        };
        Ok(GiveInputs {
            item: item
                .as_entity()
                .ok_or_else(|| AdapterError::new("give item was not an entity"))?,
            recipient: recipient
                .as_entity()
                .ok_or_else(|| AdapterError::new("give recipient was not an entity"))?,
        })
    }

    fn observe(&self, world: &World, actor: EntityId, inputs: &Self::Inputs) -> Self::Observations {
        GiveObservations {
            item: display_name(world, inputs.item),
            recipient: display_name(world, inputs.recipient),
            actor: display_name(world, actor),
            locus: world.enclosing_locus(actor),
        }
    }

    fn execute(
        &self,
        ctx: &mut PerformCtx<'_>,
        inputs: &Self::Inputs,
    ) -> TypedHandlerOutcome<Self::Results> {
        match execute(
            ctx.world,
            Action::Move {
                entity: inputs.item,
                into: inputs.recipient,
            },
        ) {
            Ok(_) => TypedHandlerOutcome::committed(()),
            Err(ExecError::Relation(RelationError::Cycle { .. })) => {
                TypedHandlerOutcome::refused("You can't give that away.")
            }
            Err(error) => {
                tracing::error!(error = %error, "canonical give hit an unexpected structural error");
                TypedHandlerOutcome::refused("You can't give that away.")
            }
        }
    }

    fn encode_results(&self, _results: &Self::Results) -> ActionOutcome {
        ActionOutcome::empty()
    }

    fn narrate(
        &self,
        ctx: &mut NarrationCtx<'_>,
        inputs: &Self::Inputs,
        _results: &Self::Results,
        observations: &Self::Observations,
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
}
