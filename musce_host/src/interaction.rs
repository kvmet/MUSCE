//! App-owned pointing policy and translation between canonical action values and
//! transport DTOs. The host routes these values but defines no app parameters or
//! affordances.

use musce_action::schema::{
    AffordanceId, GroundAction, ParameterBinding as ActionBinding, ParameterId,
    ParameterMode as ActionParameterMode, PartialGrounding, SymbolDomainId, SymbolId, SymbolValue,
    Value, ValueSort,
};
use musce_action::{
    AffordanceRegistry, ClassifiedOffer, OfferProposal, OfferStatus as ActionOfferStatus, Refusal,
    Verdict,
};
use musce_core::{EntityId, World};
use musce_proto::{
    AffordanceValue, InputCandidates as WireCandidates, Offer as WireOffer,
    OfferStatus as WireOfferStatus, ParameterBinding as WireBinding, ParameterDecl,
    ParameterMode as WireParameterMode, ParameterSort, Perform, Performed,
};

/// Read-only app context for pointing interaction policy. The engine supplies the
/// live actor, account verdict, world, and immutable canonical registry together;
/// the app decides exposure, candidates, perception, and reachability without
/// gaining a second execution path.
pub struct InteractionCtx<'a> {
    pub world: &'a World,
    pub affordances: &'a AffordanceRegistry,
    actor: EntityId,
    verdict: &'a Verdict,
}

impl<'a> InteractionCtx<'a> {
    pub fn new(
        world: &'a World,
        affordances: &'a AffordanceRegistry,
        actor: EntityId,
        verdict: &'a Verdict,
    ) -> Self {
        Self {
            world,
            affordances,
            actor,
            verdict,
        }
    }

    pub fn actor(&self) -> EntityId {
        self.actor
    }

    pub fn verdict(&self) -> &Verdict {
        self.verdict
    }
}

/// App-owned policy around the generic pointing front end. `offers` chooses which
/// partial canonical groundings and candidates to expose. `validate` may only
/// narrow a complete client grounding; registry gates and guards still run
/// afterward and cannot be bypassed.
#[derive(Clone, Copy)]
pub struct InteractionPolicy {
    pub offers: for<'a> fn(&InteractionCtx<'a>, EntityId) -> Vec<OfferProposal>,
    pub validate: for<'a> fn(&InteractionCtx<'a>, &GroundAction) -> Result<(), String>,
}

impl InteractionPolicy {
    pub fn none() -> Self {
        Self {
            offers: |_, _| Vec::new(),
            validate: |_, _| Err("You can't do that.".into()),
        }
    }
}

pub(crate) fn parse_wire_id(value: &str) -> Option<EntityId> {
    value.parse::<u64>().ok().map(EntityId)
}

pub(crate) fn grounding_from_wire(perform: Perform) -> Result<PartialGrounding, String> {
    let affordance = AffordanceId::new(perform.affordance).map_err(|error| error.to_string())?;
    let bindings = perform
        .inputs
        .into_iter()
        .map(|binding| {
            Ok(ActionBinding::new(
                ParameterId::new(binding.parameter).map_err(|error| error.to_string())?,
                value_from_wire(binding.value)?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(PartialGrounding::new(affordance, bindings))
}

fn value_from_wire(value: AffordanceValue) -> Result<Value, String> {
    match value {
        AffordanceValue::Entity { id } => parse_wire_id(&id)
            .map(Value::Entity)
            .ok_or_else(|| format!("invalid entity id {id:?}")),
        AffordanceValue::Text { text } => Ok(Value::text(text)),
        AffordanceValue::Symbol { domain, value } => Ok(Value::Symbol(SymbolValue::new(
            SymbolDomainId::new(domain).map_err(|error| error.to_string())?,
            SymbolId::new(value).map_err(|error| error.to_string())?,
        ))),
    }
}

fn value_to_wire(value: &Value) -> AffordanceValue {
    match value {
        Value::Entity(entity) => AffordanceValue::Entity {
            id: entity.0.to_string(),
        },
        Value::Text(text) => AffordanceValue::Text {
            text: text.to_string(),
        },
        Value::Symbol(symbol) => AffordanceValue::Symbol {
            domain: symbol.domain().to_string(),
            value: symbol.value().to_string(),
        },
    }
}

pub(crate) fn classified_to_wire(
    registry: &AffordanceRegistry,
    offer: ClassifiedOffer,
) -> WireOffer {
    let grounding = offer.proposal().grounding();
    let schema = registry
        .schema(grounding.affordance())
        .expect("classified offer retains its registered schema");
    let parameters = schema
        .parameters()
        .iter()
        .map(|parameter| ParameterDecl {
            id: parameter.id().to_string(),
            label: parameter.label().to_string(),
            sort: match parameter.sort() {
                ValueSort::Entity => ParameterSort::Entity,
                ValueSort::Text => ParameterSort::Text,
                ValueSort::Symbol(domain) => ParameterSort::Symbol {
                    domain: domain.to_string(),
                },
            },
            mode: match parameter.mode() {
                ActionParameterMode::Input => WireParameterMode::Input,
                ActionParameterMode::Result => WireParameterMode::Result,
            },
        })
        .collect();
    let bindings = grounding
        .bindings()
        .iter()
        .map(|binding| WireBinding {
            parameter: binding.parameter().to_string(),
            value: value_to_wire(binding.value()),
        })
        .collect();
    let candidates = offer
        .proposal()
        .candidates()
        .iter()
        .map(|set| WireCandidates {
            parameter: set.parameter().to_string(),
            values: set.values().iter().map(value_to_wire).collect(),
        })
        .collect();
    let status = match offer.status() {
        ActionOfferStatus::Available => WireOfferStatus::Available,
        ActionOfferStatus::Needs { parameters } => WireOfferStatus::Needs {
            parameters: parameters.iter().map(ToString::to_string).collect(),
        },
        ActionOfferStatus::Vetoed { reason } => WireOfferStatus::Vetoed {
            reason: reason.to_string(),
        },
    };
    WireOffer {
        affordance: schema.id().to_string(),
        display_name: schema.display_name().to_string(),
        parameters,
        bindings,
        candidates,
        status,
    }
}

pub(crate) fn performed_to_wire(
    registry: &AffordanceRegistry,
    action: &GroundAction,
    outcome: &musce_action::schema::ActionOutcome,
) -> Performed {
    let schema = registry
        .schema(action.affordance())
        .expect("performed action retains its registered schema");
    let results = schema
        .results()
        .map(|parameter| WireBinding {
            parameter: parameter.id().to_string(),
            value: value_to_wire(&outcome.results()[parameter.slot() as usize]),
        })
        .collect();
    Performed {
        affordance: action.affordance().to_string(),
        results,
    }
}

pub(crate) fn refusal_text(refusal: Refusal) -> String {
    match refusal {
        Refusal::Gate => "You aren't allowed to do that.".into(),
        Refusal::Guard { reason, .. } | Refusal::Resolution { reason } => reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_action::schema::{
        ActionOutcome, AffordanceSchema, Parameter, ParameterMode, Resolution,
    };
    use musce_action::state::StateRegistry;
    use musce_action::{AffordanceRegistryBuilder, Gate, HandlerOutcome};

    #[test]
    fn wire_grounding_preserves_named_values_of_every_sort() {
        let grounding = grounding_from_wire(Perform {
            affordance: "configure".into(),
            inputs: vec![
                WireBinding {
                    parameter: "target".into(),
                    value: AffordanceValue::Entity { id: "42".into() },
                },
                WireBinding {
                    parameter: "label".into(),
                    value: AffordanceValue::Text {
                        text: "hello".into(),
                    },
                },
                WireBinding {
                    parameter: "mode".into(),
                    value: AffordanceValue::Symbol {
                        domain: "modes".into(),
                        value: "quiet".into(),
                    },
                },
            ],
        })
        .unwrap();

        assert_eq!(grounding.affordance().as_str(), "configure");
        assert_eq!(
            grounding.bindings()[0].value(),
            &Value::Entity(EntityId(42))
        );
        assert_eq!(grounding.bindings()[1].value(), &Value::text("hello"));
        let Value::Symbol(symbol) = grounding.bindings()[2].value() else {
            panic!("symbol binding changed sort");
        };
        assert_eq!(symbol.domain().as_str(), "modes");
        assert_eq!(symbol.value().as_str(), "quiet");
    }

    #[test]
    fn performed_results_recover_declared_ids_from_dense_slots() {
        let world = World::new();
        let mut builder = AffordanceRegistryBuilder::new(StateRegistry::new());
        builder
            .register(
                AffordanceSchema::new(
                    AffordanceId::new("speak").unwrap(),
                    "Speak",
                    vec![
                        Parameter::new(
                            ParameterId::new("reply").unwrap(),
                            "reply",
                            ValueSort::Text,
                            ParameterMode::Result,
                            0,
                        )
                        .unwrap(),
                    ],
                    Vec::new(),
                    Vec::new(),
                    Gate::Open,
                    Resolution::Deterministic,
                ),
                |_, _| HandlerOutcome::committed(ActionOutcome::new(vec![Value::text("hello")])),
            )
            .unwrap();
        let registry = builder.build(&world).unwrap();
        let action =
            GroundAction::new(AffordanceId::new("speak").unwrap(), EntityId(1), Vec::new());
        let performed = performed_to_wire(
            &registry,
            &action,
            &ActionOutcome::new(vec![Value::text("hello")]),
        );

        assert_eq!(performed.affordance, "speak");
        assert_eq!(performed.results.len(), 1);
        assert_eq!(performed.results[0].parameter, "reply");
        assert_eq!(
            performed.results[0].value,
            AffordanceValue::Text {
                text: "hello".into()
            }
        );
    }
}
