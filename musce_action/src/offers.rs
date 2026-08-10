//! Partial grounding and offer classification for non-text front ends.
//!
//! The engine validates canonical ids, modes, sorts, liveness, gates, and every
//! guard whose inputs are ground. The app supplies only exposure policy: which
//! partial groundings to propose and which candidate values to present.

use std::collections::HashSet;
use std::fmt;

use musce_core::{EntityId, World};

use crate::caps::Verdict;
use crate::perform::AffordanceRegistry;
use crate::schema::{
    AffordanceId, Condition, Formula, GroundAction, OptionalEntity, ParameterId, ParameterMode,
    PartialGrounding, Term, Value, ValueSort,
};
use crate::state::EvaluationError;

/// App-supplied presentation candidates for one unbound input. These are hints,
/// not authority: a perform request is independently validated by app policy and
/// by the canonical performer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputCandidates {
    parameter: ParameterId,
    values: Box<[Value]>,
}

impl InputCandidates {
    pub fn new(parameter: ParameterId, values: impl Into<Box<[Value]>>) -> Self {
        Self {
            parameter,
            values: values.into(),
        }
    }

    pub fn parameter(&self) -> &ParameterId {
        &self.parameter
    }

    pub fn values(&self) -> &[Value] {
        &self.values
    }
}

/// One app-selected affordance exposure: a canonical partial grounding plus
/// optional candidate values for its missing inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferProposal {
    grounding: PartialGrounding,
    candidates: Box<[InputCandidates]>,
}

impl OfferProposal {
    pub fn new(grounding: PartialGrounding, candidates: impl Into<Box<[InputCandidates]>>) -> Self {
        Self {
            grounding,
            candidates: candidates.into(),
        }
    }

    pub fn grounding(&self) -> &PartialGrounding {
        &self.grounding
    }

    pub fn candidates(&self) -> &[InputCandidates] {
        &self.candidates
    }
}

/// Engine classification of an app proposal under the caller's live authority
/// and world state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OfferStatus {
    Available,
    Needs { parameters: Box<[ParameterId]> },
    Vetoed { reason: Box<str> },
}

/// A validated proposal and its current classification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassifiedOffer {
    proposal: OfferProposal,
    status: OfferStatus,
}

impl ClassifiedOffer {
    pub fn proposal(&self) -> &OfferProposal {
        &self.proposal
    }

    pub fn status(&self) -> &OfferStatus {
        &self.status
    }
}

/// Invalid app proposal or client grounding. This is structural input drift, not
/// an ordinary guard refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroundingError {
    UnknownAffordance(AffordanceId),
    DeadActor(EntityId),
    UnknownParameter(ParameterId),
    NotInput(ParameterId),
    DuplicateBinding(ParameterId),
    WrongSort {
        parameter: ParameterId,
        expected: ValueSort,
        actual: ValueSort,
    },
    DeadEntity {
        parameter: ParameterId,
        entity: EntityId,
    },
    UnknownSymbol(ParameterId),
    MissingInputs(Box<[ParameterId]>),
    DuplicateCandidates(ParameterId),
    CandidatesForBoundInput(ParameterId),
    Evaluation(EvaluationError),
}

impl fmt::Display for GroundingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAffordance(id) => write!(f, "unknown affordance {id}"),
            Self::DeadActor(actor) => write!(f, "offer actor {actor:?} is not live"),
            Self::UnknownParameter(id) => write!(f, "unknown parameter {id}"),
            Self::NotInput(id) => write!(f, "parameter {id} is not an input"),
            Self::DuplicateBinding(id) => write!(f, "input {id} is bound more than once"),
            Self::WrongSort {
                parameter,
                expected,
                actual,
            } => write!(
                f,
                "input {parameter} has sort {actual:?}, expected {expected:?}"
            ),
            Self::DeadEntity { parameter, entity } => {
                write!(f, "input {parameter} names dead entity {entity:?}")
            }
            Self::UnknownSymbol(id) => write!(f, "input {id} names an unknown symbol"),
            Self::MissingInputs(ids) => write!(
                f,
                "grounding is missing inputs {}",
                ids.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::DuplicateCandidates(id) => {
                write!(f, "input {id} has more than one candidate set")
            }
            Self::CandidatesForBoundInput(id) => {
                write!(f, "bound input {id} also supplies candidates")
            }
            Self::Evaluation(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for GroundingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Evaluation(error) => Some(error),
            _ => None,
        }
    }
}

impl From<EvaluationError> for GroundingError {
    fn from(value: EvaluationError) -> Self {
        Self::Evaluation(value)
    }
}

impl AffordanceRegistry {
    /// Validate and classify an app-selected partial grounding. The app controls
    /// exposure and candidates; the engine controls canonical interpretation.
    pub fn classify_offer(
        &self,
        world: &World,
        verdict: &Verdict,
        actor: EntityId,
        proposal: OfferProposal,
    ) -> Result<ClassifiedOffer, GroundingError> {
        let (schema, slots, bound, missing) =
            self.validate_bindings(world, actor, proposal.grounding())?;
        validate_candidates(self, world, schema, &bound, proposal.candidates())?;

        let status = if !schema.gate().permits(verdict) {
            OfferStatus::Vetoed {
                reason: "You aren't allowed to do that.".into(),
            }
        } else {
            let mut veto = None;
            for guard in schema.guards() {
                if !formula_is_ground(guard.formula(), &bound) {
                    continue;
                }
                let action = GroundAction::new(schema.id().clone(), actor, slots.clone());
                if !self
                    .state()
                    .evaluate(guard.formula(), schema, &action, None, world)?
                {
                    veto = Some(guard.reason().into());
                    break;
                }
            }
            match veto {
                Some(reason) => OfferStatus::Vetoed { reason },
                None => classify_after_guards(missing),
            }
        };

        Ok(ClassifiedOffer { proposal, status })
    }

    /// Turn a complete named binding set into the canonical dense-slot action.
    /// This is shared by wire/script front ends; performer validation still runs
    /// afterward immediately before mutation.
    pub fn ground_action(
        &self,
        world: &World,
        actor: EntityId,
        grounding: &PartialGrounding,
    ) -> Result<GroundAction, GroundingError> {
        let (schema, slots, _bound, missing) = self.validate_bindings(world, actor, grounding)?;
        if !missing.is_empty() {
            return Err(GroundingError::MissingInputs(missing.into_boxed_slice()));
        }
        Ok(GroundAction::new(schema.id().clone(), actor, slots))
    }

    fn validate_bindings<'a>(
        &'a self,
        world: &World,
        actor: EntityId,
        grounding: &PartialGrounding,
    ) -> Result<
        (
            &'a crate::schema::AffordanceSchema,
            Vec<Value>,
            HashSet<ParameterId>,
            Vec<ParameterId>,
        ),
        GroundingError,
    > {
        if !world.contains(actor) {
            return Err(GroundingError::DeadActor(actor));
        }
        let schema = self
            .schema(grounding.affordance())
            .ok_or_else(|| GroundingError::UnknownAffordance(grounding.affordance().clone()))?;
        let input_count = schema.inputs().count();
        let mut values: Vec<Option<Value>> = vec![None; input_count];
        let mut bound = HashSet::new();
        for binding in grounding.bindings() {
            let id = binding.parameter();
            let parameter = schema
                .parameter(id)
                .ok_or_else(|| GroundingError::UnknownParameter(id.clone()))?;
            if parameter.mode() != ParameterMode::Input {
                return Err(GroundingError::NotInput(id.clone()));
            }
            if !bound.insert(id.clone()) {
                return Err(GroundingError::DuplicateBinding(id.clone()));
            }
            validate_value(self, world, id, parameter.sort(), binding.value())?;
            values[parameter.slot() as usize] = Some(binding.value().clone());
        }

        let missing: Vec<_> = schema
            .inputs()
            .filter(|parameter| values[parameter.slot() as usize].is_none())
            .map(|parameter| parameter.id().clone())
            .collect();
        // A ground guard never reads an unbound parameter. Fill the unused dense
        // slots with a sort-valid value so the existing evaluator can retain its
        // single canonical action input. Dependency analysis below is the proof.
        let slots = (0..input_count)
            .map(|slot| {
                values[slot].clone().unwrap_or_else(|| {
                    let parameter = schema
                        .inputs()
                        .find(|parameter| parameter.slot() as usize == slot)
                        .expect("validated input slots are dense");
                    placeholder(parameter.sort(), actor, self)
                })
            })
            .collect();
        Ok((schema, slots, bound, missing))
    }
}

fn classify_after_guards(missing: Vec<ParameterId>) -> OfferStatus {
    if missing.is_empty() {
        OfferStatus::Available
    } else {
        OfferStatus::Needs {
            parameters: missing.into_boxed_slice(),
        }
    }
}

fn validate_candidates(
    registry: &AffordanceRegistry,
    world: &World,
    schema: &crate::schema::AffordanceSchema,
    bound: &HashSet<ParameterId>,
    candidates: &[InputCandidates],
) -> Result<(), GroundingError> {
    let mut seen = HashSet::new();
    for set in candidates {
        let id = set.parameter();
        let parameter = schema
            .parameter(id)
            .ok_or_else(|| GroundingError::UnknownParameter(id.clone()))?;
        if parameter.mode() != ParameterMode::Input {
            return Err(GroundingError::NotInput(id.clone()));
        }
        if !seen.insert(id.clone()) {
            return Err(GroundingError::DuplicateCandidates(id.clone()));
        }
        if bound.contains(id) {
            return Err(GroundingError::CandidatesForBoundInput(id.clone()));
        }
        for value in set.values() {
            validate_value(registry, world, id, parameter.sort(), value)?;
        }
    }
    Ok(())
}

fn validate_value(
    registry: &AffordanceRegistry,
    world: &World,
    parameter: &ParameterId,
    expected: &ValueSort,
    value: &Value,
) -> Result<(), GroundingError> {
    let actual = value.sort();
    if actual != *expected {
        return Err(GroundingError::WrongSort {
            parameter: parameter.clone(),
            expected: expected.clone(),
            actual,
        });
    }
    match value {
        Value::Entity(entity) if !world.contains(*entity) => Err(GroundingError::DeadEntity {
            parameter: parameter.clone(),
            entity: *entity,
        }),
        Value::Symbol(symbol) if !registry.state().has_symbol(symbol) => {
            Err(GroundingError::UnknownSymbol(parameter.clone()))
        }
        _ => Ok(()),
    }
}

fn placeholder(sort: &ValueSort, actor: EntityId, registry: &AffordanceRegistry) -> Value {
    match sort {
        ValueSort::Entity => Value::Entity(actor),
        ValueSort::Text => Value::text(""),
        ValueSort::Symbol(domain) => registry
            .state()
            .first_symbol(domain)
            .map(Value::Symbol)
            .expect("activated schema symbol domain is nonempty"),
    }
}

fn formula_is_ground(formula: &Formula, bound: &HashSet<ParameterId>) -> bool {
    formula
        .conditions()
        .iter()
        .flat_map(condition_terms)
        .all(|term| !matches!(term, Term::Input(id) if !bound.contains(id)))
}

fn condition_terms(condition: &Condition) -> Vec<&Term> {
    match condition {
        Condition::RelationTarget { source, target, .. } => {
            let mut terms = vec![source];
            if let OptionalEntity::Is(term) | OptionalEntity::IsNot(term) = target {
                terms.push(term);
            }
            terms
        }
        Condition::ComponentPresent { entity, .. }
        | Condition::GaugeAtLeast { entity, .. }
        | Condition::GaugeAtMost { entity, .. }
        | Condition::Exists { entity, .. } => vec![entity],
        Condition::LocusOf { entity, locus } => {
            let mut terms = vec![entity];
            if let OptionalEntity::Is(term) | OptionalEntity::IsNot(term) = locus {
                terms.push(term);
            }
            terms
        }
        Condition::Distinct { left, right } => vec![left, right],
    }
}

#[cfg(test)]
mod tests;
