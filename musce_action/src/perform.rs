//! Immutable registration and shared execution for canonical affordances.
//!
//! Every caller supplies the same grounded action and account-scoped verdict.
//! The performer validates grounding, checks authority and ordered guards, and
//! only then lends the world to the app handler. It never derives authority from
//! the actor entity.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use musce_core::{EntityId, World};
use musce_proto::EventKind;

use crate::GaugeId;
use crate::audience::Outbound;
use crate::caps::Verdict;
use crate::event::Event;
use crate::schema::{
    ActionOutcome, AffordanceId, AffordanceSchema, ComponentId, GroundAction, RelationId,
    Resolution, Value,
};
use crate::state::{EvaluationError, StateActivationError, StateRegistry};

mod validation;
use validation::{effect_index, validate_schema};

#[cfg(test)]
mod tests;

/// A planner-facing state dimension changed by one or more registered effects.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EffectKind {
    Relation(RelationId),
    Component(ComponentId),
    Locus,
    Gauge(GaugeId),
    Existence,
}

/// One schema defect found while assembling the immutable registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaError {
    affordance: AffordanceId,
    issue: String,
}

impl SchemaError {
    fn new(affordance: AffordanceId, issue: impl Into<String>) -> Self {
        Self {
            affordance,
            issue: issue.into(),
        }
    }

    pub fn affordance(&self) -> &AffordanceId {
        &self.affordance
    }

    pub fn issue(&self) -> &str {
        &self.issue
    }
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid affordance {}: {}", self.affordance, self.issue)
    }
}

impl std::error::Error for SchemaError {}

/// Failure while registering or activating the canonical affordance set.
#[derive(Debug)]
pub enum RegistryError {
    DuplicateAffordance(AffordanceId),
    InvalidSchema(SchemaError),
    StateActivation(StateActivationError),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::DuplicateAffordance(id) => {
                write!(f, "affordance {id} is already registered")
            }
            RegistryError::InvalidSchema(error) => error.fmt(f),
            RegistryError::StateActivation(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RegistryError::InvalidSchema(error) => Some(error),
            RegistryError::StateActivation(error) => Some(error),
            RegistryError::DuplicateAffordance(_) => None,
        }
    }
}

impl From<SchemaError> for RegistryError {
    fn from(value: SchemaError) -> Self {
        Self::InvalidSchema(value)
    }
}

impl From<StateActivationError> for RegistryError {
    fn from(value: StateActivationError) -> Self {
        Self::StateActivation(value)
    }
}

/// The result returned by an app implementation after all shared checks pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandlerOutcome {
    Committed(ActionOutcome),
    Refused(Box<str>),
}

impl HandlerOutcome {
    pub fn committed(outcome: ActionOutcome) -> Self {
        Self::Committed(outcome)
    }

    pub fn refused(reason: impl Into<String>) -> Self {
        Self::Refused(reason.into().into_boxed_str())
    }
}

/// Type-erased implementation beneath generated typed handler adapters.
pub type AffordanceHandler = Box<
    dyn for<'a> Fn(&mut PerformCtx<'a>, &GroundAction) -> HandlerOutcome + Send + Sync + 'static,
>;

struct Entry {
    schema: AffordanceSchema,
    handler: AffordanceHandler,
}

/// Mutable boot-time assembler. `build` consumes it so the live registry cannot
/// drift after planners, commands, and offers begin sharing it.
pub struct AffordanceRegistryBuilder {
    state: StateRegistry,
    entries: BTreeMap<AffordanceId, Entry>,
}

impl AffordanceRegistryBuilder {
    pub fn new(state: StateRegistry) -> Self {
        Self {
            state,
            entries: BTreeMap::new(),
        }
    }

    /// Validate and register one schema together with its required implementation.
    pub fn register(
        &mut self,
        schema: AffordanceSchema,
        handler: impl for<'a> Fn(&mut PerformCtx<'a>, &GroundAction) -> HandlerOutcome
        + Send
        + Sync
        + 'static,
    ) -> Result<(), RegistryError> {
        if self.entries.contains_key(schema.id()) {
            return Err(RegistryError::DuplicateAffordance(schema.id().clone()));
        }
        validate_schema(&schema, &self.state)?;
        self.entries.insert(
            schema.id().clone(),
            Entry {
                schema,
                handler: Box::new(handler),
            },
        );
        Ok(())
    }

    /// Prove the typed readers are wired into `world`, construct indexes, and
    /// consume all mutable registration state.
    pub fn build(self, world: &World) -> Result<AffordanceRegistry, RegistryError> {
        self.state.validate_world(world)?;
        let effects = effect_index(self.entries.values().map(|entry| &entry.schema));
        Ok(AffordanceRegistry {
            state: self.state,
            entries: self.entries,
            effects,
        })
    }
}

/// The app's immutable canonical affordance vocabulary and execution path.
pub struct AffordanceRegistry {
    state: StateRegistry,
    entries: BTreeMap<AffordanceId, Entry>,
    effects: HashMap<EffectKind, Box<[AffordanceId]>>,
}

impl AffordanceRegistry {
    pub fn schema(&self, id: &AffordanceId) -> Option<&AffordanceSchema> {
        self.entries.get(id).map(|entry| &entry.schema)
    }

    /// Registered schemas in stable symbolic-id order.
    pub fn schemas(&self) -> impl Iterator<Item = &AffordanceSchema> {
        self.entries.values().map(|entry| &entry.schema)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn state(&self) -> &StateRegistry {
        &self.state
    }

    pub fn affordances_affecting(&self, kind: &EffectKind) -> &[AffordanceId] {
        self.effects.get(kind).map(Box::as_ref).unwrap_or(&[])
    }

    /// Execute through the one gate/grounding/guard/handler contract. Kept inside
    /// the crate so app entry points must borrow it through `Ctx` or `SystemCtx`.
    pub(crate) fn perform(
        &self,
        world: &mut World,
        out: &mut Vec<Outbound>,
        verdict: &Verdict,
        action: &GroundAction,
    ) -> Result<PerformOutcome, PerformError> {
        let entry = self
            .entries
            .get(action.affordance())
            .ok_or_else(|| PerformError::UnknownAffordance(action.affordance().clone()))?;

        validate_grounding(&entry.schema, &self.state, world, action)?;

        if !entry.schema.gate().permits(verdict) {
            return Ok(PerformOutcome::Refused(Refusal::Gate));
        }

        for (index, guard) in entry.schema.guards().iter().enumerate() {
            if !self
                .state
                .evaluate(guard.formula(), &entry.schema, action, None, world)?
            {
                return Ok(PerformOutcome::Refused(Refusal::Guard {
                    index,
                    reason: guard.reason().into(),
                }));
            }
        }

        let (outcome, pending_out) = {
            let mut pending_out = Vec::new();
            let mut ctx = PerformCtx::new(world, &mut pending_out, verdict, action.actor());
            let outcome = (entry.handler)(&mut ctx, action);
            (outcome, pending_out)
        };
        match outcome {
            HandlerOutcome::Committed(outcome) => {
                validate_results(&entry.schema, &self.state, &outcome)?;
                out.extend(pending_out);
                Ok(PerformOutcome::Committed(outcome))
            }
            HandlerOutcome::Refused(reason)
                if entry.schema.resolution() == Resolution::Deterministic =>
            {
                Err(PerformError::DeterministicRefusal {
                    affordance: action.affordance().clone(),
                    reason,
                })
            }
            HandlerOutcome::Refused(reason) => {
                Ok(PerformOutcome::Refused(Refusal::Resolution { reason }))
            }
        }
    }
}

/// The deliberately small capability lent to an app implementation after all
/// shared checks pass. Actor and verdict are read-only; authority is never
/// reconstructed from world components. Output is staged and becomes visible
/// only if the handler commits with valid result bindings.
pub struct PerformCtx<'a> {
    pub world: &'a mut World,
    out: &'a mut Vec<Outbound>,
    verdict: &'a Verdict,
    actor: EntityId,
}

impl<'a> PerformCtx<'a> {
    fn new(
        world: &'a mut World,
        out: &'a mut Vec<Outbound>,
        verdict: &'a Verdict,
        actor: EntityId,
    ) -> Self {
        Self {
            world,
            out,
            verdict,
            actor,
        }
    }

    pub fn actor(&self) -> EntityId {
        self.actor
    }

    pub fn verdict(&self) -> &Verdict {
        self.verdict
    }

    pub fn permits(&self, cap: crate::CapId) -> bool {
        self.verdict.permits(cap)
    }

    pub fn is_su(&self) -> bool {
        self.verdict.is_su()
    }

    pub fn emit_self(&mut self, kind: EventKind, text: impl Into<String>) {
        self.emit_entity(self.actor, kind, text);
    }

    pub fn emit_entity(&mut self, entity: EntityId, kind: EventKind, text: impl Into<String>) {
        self.out
            .push(Outbound::new(Event::to_entity(entity, kind, text)));
    }

    pub fn emit_locus(&mut self, locus: EntityId, kind: EventKind, text: impl Into<String>) {
        self.out
            .push(Outbound::new(Event::to_locus(locus, kind, text)));
    }

    pub fn emit_locus_except(
        &mut self,
        locus: EntityId,
        kind: EventKind,
        text: impl Into<String>,
        exclude: &[EntityId],
    ) {
        self.out.push(Outbound::excluding(
            Event::to_locus(locus, kind, text),
            exclude.to_vec(),
        ));
    }
}

/// A normal non-commit from the shared checks or a non-deterministic handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Gate,
    Guard { index: usize, reason: Box<str> },
    Resolution { reason: Box<str> },
}

/// A completed canonical attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PerformOutcome {
    Committed(ActionOutcome),
    Refused(Refusal),
}

/// Malformed or stale grounding, evaluation failure, or handler contract drift.
#[derive(Debug)]
pub enum PerformError {
    UnknownAffordance(AffordanceId),
    ActorMismatch {
        caller: EntityId,
        action: EntityId,
    },
    DeadActor(EntityId),
    InputCount {
        expected: usize,
        actual: usize,
    },
    WrongInputSort {
        slot: usize,
        expected: crate::schema::ValueSort,
        actual: crate::schema::ValueSort,
    },
    DeadInput {
        slot: usize,
        entity: EntityId,
    },
    UnknownSymbol {
        slot: usize,
    },
    Evaluation(EvaluationError),
    ResultCount {
        expected: usize,
        actual: usize,
    },
    WrongResultSort {
        slot: usize,
        expected: crate::schema::ValueSort,
        actual: crate::schema::ValueSort,
    },
    UnknownResultSymbol {
        slot: usize,
    },
    DeterministicRefusal {
        affordance: AffordanceId,
        reason: Box<str>,
    },
}

impl fmt::Display for PerformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PerformError::UnknownAffordance(id) => write!(f, "unknown affordance {id}"),
            PerformError::ActorMismatch { caller, action } => {
                write!(f, "caller actor {caller:?} cannot perform as {action:?}")
            }
            PerformError::DeadActor(actor) => write!(f, "action actor {actor:?} is not live"),
            PerformError::InputCount { expected, actual } => {
                write!(f, "grounding has {actual} inputs, expected {expected}")
            }
            PerformError::WrongInputSort {
                slot,
                expected,
                actual,
            } => write!(
                f,
                "input slot {slot} has sort {actual:?}, expected {expected:?}"
            ),
            PerformError::DeadInput { slot, entity } => {
                write!(
                    f,
                    "entity input at slot {slot} names dead entity {entity:?}"
                )
            }
            PerformError::UnknownSymbol { slot } => write!(
                f,
                "symbol input at slot {slot} is not in its registered domain"
            ),
            PerformError::Evaluation(error) => error.fmt(f),
            PerformError::ResultCount { expected, actual } => {
                write!(f, "handler returned {actual} results, expected {expected}")
            }
            PerformError::WrongResultSort {
                slot,
                expected,
                actual,
            } => write!(
                f,
                "result slot {slot} has sort {actual:?}, expected {expected:?}"
            ),
            PerformError::UnknownResultSymbol { slot } => write!(
                f,
                "symbol result at slot {slot} is not in its registered domain"
            ),
            PerformError::DeterministicRefusal { affordance, reason } => write!(
                f,
                "deterministic affordance {affordance} refused after its guards passed: {reason}"
            ),
        }
    }
}

impl std::error::Error for PerformError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PerformError::Evaluation(error) => Some(error),
            _ => None,
        }
    }
}

impl From<EvaluationError> for PerformError {
    fn from(value: EvaluationError) -> Self {
        Self::Evaluation(value)
    }
}

fn validate_grounding(
    schema: &AffordanceSchema,
    state: &StateRegistry,
    world: &World,
    action: &GroundAction,
) -> Result<(), PerformError> {
    if !world.contains(action.actor()) {
        return Err(PerformError::DeadActor(action.actor()));
    }
    let expected = schema.inputs().count();
    if action.inputs().len() != expected {
        return Err(PerformError::InputCount {
            expected,
            actual: action.inputs().len(),
        });
    }
    for parameter in schema.inputs() {
        let slot = parameter.slot() as usize;
        let value = &action.inputs()[slot];
        if value.sort() != *parameter.sort() {
            return Err(PerformError::WrongInputSort {
                slot,
                expected: parameter.sort().clone(),
                actual: value.sort(),
            });
        }
        if let Value::Entity(entity) = value
            && !world.contains(*entity)
        {
            return Err(PerformError::DeadInput {
                slot,
                entity: *entity,
            });
        }
        if matches!(value, Value::Symbol(symbol) if !state.has_symbol(symbol)) {
            return Err(PerformError::UnknownSymbol { slot });
        }
    }
    Ok(())
}

fn validate_results(
    schema: &AffordanceSchema,
    state: &StateRegistry,
    outcome: &ActionOutcome,
) -> Result<(), PerformError> {
    let expected = schema.results().count();
    if outcome.results().len() != expected {
        return Err(PerformError::ResultCount {
            expected,
            actual: outcome.results().len(),
        });
    }
    for parameter in schema.results() {
        let slot = parameter.slot() as usize;
        let value = &outcome.results()[slot];
        if value.sort() != *parameter.sort() {
            return Err(PerformError::WrongResultSort {
                slot,
                expected: parameter.sort().clone(),
                actual: value.sort(),
            });
        }
        if matches!(value, Value::Symbol(symbol) if !state.has_symbol(symbol)) {
            return Err(PerformError::UnknownResultSymbol { slot });
        }
    }
    Ok(())
}
