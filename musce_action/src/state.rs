//! Registered readers and evaluation for the canonical affordance state algebra.
//!
//! Stored components and relations remain world truth. This registry gives their
//! stable schema ids typed, read-only evaluators; gauges add normalized derived
//! readings and ordered qualitative regions. Unknown vocabulary is always an
//! error, never a false condition.

use std::any::{TypeId, type_name};
use std::collections::{HashMap, HashSet};
use std::fmt;

use musce_core::{EntityId, Id, NamedComponent, Relation, World};

use crate::schema::{
    ActionOutcome, AffordanceSchema, ComponentId, Condition, Formula, GaugeRegionId, GroundAction,
    LocalId, NameError, OptionalEntity, ParameterId, ParameterMode, RelationId, SymbolDomainId,
    SymbolId, SymbolValue, Term, Value, ValueSort,
};
use crate::{GaugeId, GaugeLevel, GaugeTarget};

mod validation;
use validation::{validate_gauge_name, validate_regions};

#[cfg(test)]
mod tests;

type RelationReader = Box<dyn Fn(&World, EntityId) -> Option<EntityId> + Send + Sync>;
type ComponentReader = Box<dyn Fn(&World, EntityId) -> bool + Send + Sync>;
type GaugeReader = Box<dyn Fn(&World, EntityId) -> Option<GaugeLevel> + Send + Sync>;
type WorldCheck = fn(&World) -> bool;

struct RelationEntry {
    rust_type: TypeId,
    type_name: &'static str,
    read: RelationReader,
    registered: WorldCheck,
}

struct ComponentEntry {
    rust_type: TypeId,
    type_name: &'static str,
    read: ComponentReader,
    registered: WorldCheck,
}

struct GaugeEntry {
    read: GaugeReader,
    regions: Box<[GaugeRegion]>,
    by_id: HashMap<GaugeRegionId, usize>,
}

/// One named inclusive interval in a gauge's total qualitative ordering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GaugeRegion {
    id: GaugeRegionId,
    target: GaugeTarget,
}

impl GaugeRegion {
    pub fn new(id: GaugeRegionId, target: GaugeTarget) -> Self {
        Self { id, target }
    }

    pub fn id(&self) -> &GaugeRegionId {
        &self.id
    }

    pub fn target(&self) -> GaugeTarget {
        self.target
    }
}

/// A point reading plus its registered qualitative region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GaugeReading {
    level: GaugeLevel,
    region: GaugeRegionId,
    ordinal: usize,
}

impl GaugeReading {
    pub fn level(&self) -> GaugeLevel {
        self.level
    }

    pub fn region(&self) -> &GaugeRegionId {
        &self.region
    }

    pub fn ordinal(&self) -> usize {
        self.ordinal
    }
}

/// Failure while assembling stable state vocabulary.
#[derive(Debug)]
pub enum StateRegistrationError {
    Name(NameError),
    IdCollision {
        kind: &'static str,
        id: String,
        existing: &'static str,
        attempted: &'static str,
    },
    DuplicateGauge(String),
    InvalidGauge {
        gauge: String,
        reason: String,
    },
    DuplicateSymbolDomain(String),
    InvalidSymbolDomain {
        domain: String,
        reason: String,
    },
}

impl From<NameError> for StateRegistrationError {
    fn from(value: NameError) -> Self {
        Self::Name(value)
    }
}

impl fmt::Display for StateRegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateRegistrationError::Name(error) => error.fmt(f),
            StateRegistrationError::IdCollision {
                kind,
                id,
                existing,
                attempted,
            } => write!(
                f,
                "{kind} id {id:?} is already bound to {existing}, not {attempted}"
            ),
            StateRegistrationError::DuplicateGauge(id) => {
                write!(f, "gauge id {id:?} is already registered")
            }
            StateRegistrationError::InvalidGauge { gauge, reason } => {
                write!(f, "invalid gauge {gauge:?}: {reason}")
            }
            StateRegistrationError::DuplicateSymbolDomain(id) => {
                write!(f, "symbol domain {id:?} is already registered")
            }
            StateRegistrationError::InvalidSymbolDomain { domain, reason } => {
                write!(f, "invalid symbol domain {domain:?}: {reason}")
            }
        }
    }
}

impl std::error::Error for StateRegistrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StateRegistrationError::Name(error) => Some(error),
            _ => None,
        }
    }
}

/// State readers whose typed world registration is missing at activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateActivationError {
    missing: Box<[String]>,
}

impl StateActivationError {
    pub fn missing(&self) -> &[String] {
        &self.missing
    }
}

impl fmt::Display for StateActivationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "affordance state vocabulary requires unregistered world types: {}",
            self.missing.join(", ")
        )
    }
}

impl std::error::Error for StateActivationError {}

/// A malformed grounding, term, or state reference during evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluationError {
    UnknownRelation(RelationId),
    UnknownComponent(ComponentId),
    UnknownGauge(GaugeId),
    UnknownGaugeRegion {
        gauge: GaugeId,
        region: GaugeRegionId,
    },
    UnknownSymbolDomain(SymbolDomainId),
    UnknownParameter(ParameterId),
    WrongParameterMode {
        parameter: ParameterId,
        expected: ParameterMode,
        actual: ParameterMode,
    },
    MissingParameterValue {
        parameter: ParameterId,
        mode: ParameterMode,
        slot: u16,
    },
    WrongValueSort {
        parameter: ParameterId,
        expected: ValueSort,
        actual: ValueSort,
    },
    UndeclaredLocal(LocalId),
    NonEnumerableLocal(LocalId),
    ExpectedEntity(ValueSort),
    GaugeLevelOutsideRegions {
        gauge: GaugeId,
        level: GaugeLevel,
    },
}

impl fmt::Display for EvaluationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvaluationError::UnknownRelation(id) => write!(f, "unknown relation id {id}"),
            EvaluationError::UnknownComponent(id) => write!(f, "unknown component id {id}"),
            EvaluationError::UnknownGauge(id) => write!(f, "unknown gauge id {}", id.as_str()),
            EvaluationError::UnknownGaugeRegion { gauge, region } => {
                write!(f, "unknown region {region} for gauge {}", gauge.as_str())
            }
            EvaluationError::UnknownSymbolDomain(id) => {
                write!(f, "unknown symbol domain {id}")
            }
            EvaluationError::UnknownParameter(id) => write!(f, "unknown parameter id {id}"),
            EvaluationError::WrongParameterMode {
                parameter,
                expected,
                actual,
            } => write!(
                f,
                "parameter {parameter} has mode {actual:?}, expected {expected:?}"
            ),
            EvaluationError::MissingParameterValue {
                parameter,
                mode,
                slot,
            } => write!(
                f,
                "missing {mode:?} value for parameter {parameter} at slot {slot}"
            ),
            EvaluationError::WrongValueSort {
                parameter,
                expected,
                actual,
            } => write!(
                f,
                "parameter {parameter} has value sort {actual:?}, expected {expected:?}"
            ),
            EvaluationError::UndeclaredLocal(id) => write!(f, "undeclared local {id}"),
            EvaluationError::NonEnumerableLocal(id) => {
                write!(f, "local {id} has a non-enumerable Text domain")
            }
            EvaluationError::ExpectedEntity(actual) => {
                write!(f, "state position requires Entity, got {actual:?}")
            }
            EvaluationError::GaugeLevelOutsideRegions { gauge, level } => write!(
                f,
                "gauge {} returned level {} outside its registered regions",
                gauge.as_str(),
                level.get()
            ),
        }
    }
}

impl std::error::Error for EvaluationError {}

/// The app-assembled read vocabulary for canonical conditions.
#[derive(Default)]
pub struct StateRegistry {
    relations: HashMap<RelationId, RelationEntry>,
    components: HashMap<ComponentId, ComponentEntry>,
    gauges: HashMap<GaugeId, GaugeEntry>,
    symbols: HashMap<SymbolDomainId, Box<[SymbolId]>>,
}

impl StateRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a typed relation reader under its stable persisted target tag.
    pub fn register_relation<R: Relation>(&mut self) -> Result<RelationId, StateRegistrationError> {
        let id = RelationId::new(R::TARGET_TAG)?;
        let rust_type = TypeId::of::<R>();
        if let Some(existing) = self.relations.get(&id) {
            if existing.rust_type == rust_type {
                return Ok(id);
            }
            return Err(StateRegistrationError::IdCollision {
                kind: "relation",
                id: id.to_string(),
                existing: existing.type_name,
                attempted: type_name::<R>(),
            });
        }
        self.relations.insert(
            id.clone(),
            RelationEntry {
                rust_type,
                type_name: type_name::<R>(),
                read: Box::new(|world, source| world.target_of::<R>(source)),
                registered: |world| world.is_relation_registered::<R>(),
            },
        );
        Ok(id)
    }

    /// Register a typed component-presence reader under its persisted tag.
    pub fn register_component<C: NamedComponent>(
        &mut self,
    ) -> Result<ComponentId, StateRegistrationError> {
        let id = ComponentId::new(C::TAG)?;
        let rust_type = TypeId::of::<C>();
        if let Some(existing) = self.components.get(&id) {
            if existing.rust_type == rust_type {
                return Ok(id);
            }
            return Err(StateRegistrationError::IdCollision {
                kind: "component",
                id: id.to_string(),
                existing: existing.type_name,
                attempted: type_name::<C>(),
            });
        }
        self.components.insert(
            id.clone(),
            ComponentEntry {
                rust_type,
                type_name: type_name::<C>(),
                read: Box::new(|world, entity| world.has::<C>(entity)),
                registered: |world| world.is_component_registered::<C>(),
            },
        );
        Ok(id)
    }

    /// Register a normalized point evaluator and its total ordered region cover.
    pub fn register_gauge(
        &mut self,
        id: GaugeId,
        regions: impl Into<Box<[GaugeRegion]>>,
        read: impl Fn(&World, EntityId) -> Option<GaugeLevel> + Send + Sync + 'static,
    ) -> Result<(), StateRegistrationError> {
        validate_gauge_name(&id)?;
        if self.gauges.contains_key(&id) {
            return Err(StateRegistrationError::DuplicateGauge(
                id.as_str().to_string(),
            ));
        }
        let regions = regions.into();
        let by_id = validate_regions(&id, &regions)?;
        self.gauges.insert(
            id,
            GaugeEntry {
                read: Box::new(read),
                regions,
                by_id,
            },
        );
        Ok(())
    }

    /// Register a finite symbolic input/local domain.
    pub fn register_symbol_domain(
        &mut self,
        id: SymbolDomainId,
        members: impl Into<Box<[SymbolId]>>,
    ) -> Result<(), StateRegistrationError> {
        if self.symbols.contains_key(&id) {
            return Err(StateRegistrationError::DuplicateSymbolDomain(
                id.to_string(),
            ));
        }
        let members = members.into();
        if members.is_empty() {
            return Err(StateRegistrationError::InvalidSymbolDomain {
                domain: id.to_string(),
                reason: "a finite domain must contain at least one member".into(),
            });
        }
        let mut unique = HashSet::new();
        if let Some(duplicate) = members
            .iter()
            .find(|member| !unique.insert((*member).clone()))
        {
            return Err(StateRegistrationError::InvalidSymbolDomain {
                domain: id.to_string(),
                reason: format!("duplicate member {duplicate}"),
            });
        }
        self.symbols.insert(id, members);
        Ok(())
    }

    pub fn has_relation(&self, id: &RelationId) -> bool {
        self.relations.contains_key(id)
    }

    pub fn has_component(&self, id: &ComponentId) -> bool {
        self.components.contains_key(id)
    }

    pub fn has_gauge(&self, id: &GaugeId) -> bool {
        self.gauges.contains_key(id)
    }

    pub fn has_gauge_region(&self, gauge: &GaugeId, region: &GaugeRegionId) -> bool {
        self.gauges
            .get(gauge)
            .is_some_and(|entry| entry.by_id.contains_key(region))
    }

    pub fn has_symbol_domain(&self, id: &SymbolDomainId) -> bool {
        self.symbols.contains_key(id)
    }

    pub fn has_symbol(&self, value: &SymbolValue) -> bool {
        self.symbols
            .get(value.domain())
            .is_some_and(|members| members.contains(value.value()))
    }

    pub(crate) fn first_symbol(&self, domain: &SymbolDomainId) -> Option<SymbolValue> {
        self.symbols
            .get(domain)
            .and_then(|members| members.first())
            .cloned()
            .map(|member| SymbolValue::new(domain.clone(), member))
    }

    /// Prove that every typed state reader was also registered on this world.
    pub fn validate_world(&self, world: &World) -> Result<(), StateActivationError> {
        let mut missing = Vec::new();
        for (id, entry) in &self.relations {
            if !(entry.registered)(world) {
                missing.push(format!("relation {id} ({})", entry.type_name));
            }
        }
        for (id, entry) in &self.components {
            if !(entry.registered)(world) {
                missing.push(format!("component {id} ({})", entry.type_name));
            }
        }
        if missing.is_empty() {
            Ok(())
        } else {
            missing.sort();
            Err(StateActivationError {
                missing: missing.into_boxed_slice(),
            })
        }
    }

    /// Read a gauge and project it into its registered qualitative region.
    pub fn read_gauge(
        &self,
        gauge: &GaugeId,
        world: &World,
        entity: EntityId,
    ) -> Result<Option<GaugeReading>, EvaluationError> {
        let entry = self
            .gauges
            .get(gauge)
            .ok_or_else(|| EvaluationError::UnknownGauge(gauge.clone()))?;
        let Some(level) = (entry.read)(world, entity) else {
            return Ok(None);
        };
        let Some((ordinal, region)) = entry
            .regions
            .iter()
            .enumerate()
            .find(|(_, region)| region.target().contains(level))
        else {
            return Err(EvaluationError::GaugeLevelOutsideRegions {
                gauge: gauge.clone(),
                level,
            });
        };
        Ok(Some(GaugeReading {
            level,
            region: region.id().clone(),
            ordinal,
        }))
    }

    /// Evaluate a canonical formula against one grounded action and optional
    /// successful results. Formula locals are existentially enumerated from the
    /// live world or their registered finite symbol domain.
    pub fn evaluate(
        &self,
        formula: &Formula,
        schema: &AffordanceSchema,
        action: &GroundAction,
        outcome: Option<&ActionOutcome>,
        world: &World,
    ) -> Result<bool, EvaluationError> {
        let mut locals = HashMap::new();
        self.evaluate_locals(0, formula, schema, action, outcome, world, &mut locals)
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_locals(
        &self,
        at: usize,
        formula: &Formula,
        schema: &AffordanceSchema,
        action: &GroundAction,
        outcome: Option<&ActionOutcome>,
        world: &World,
        locals: &mut HashMap<LocalId, Value>,
    ) -> Result<bool, EvaluationError> {
        let Some(local) = formula.locals().get(at) else {
            for condition in formula.conditions() {
                if !self.evaluate_condition(condition, schema, action, outcome, world, locals)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        };

        let candidates = self.local_candidates(local.id(), local.sort(), world)?;
        for candidate in candidates {
            locals.insert(local.id().clone(), candidate);
            if self.evaluate_locals(at + 1, formula, schema, action, outcome, world, locals)? {
                locals.remove(local.id());
                return Ok(true);
            }
        }
        locals.remove(local.id());
        Ok(false)
    }

    fn local_candidates(
        &self,
        id: &LocalId,
        sort: &ValueSort,
        world: &World,
    ) -> Result<Vec<Value>, EvaluationError> {
        match sort {
            ValueSort::Entity => Ok(world
                .query::<&Id>()
                .iter()
                .map(|id| Value::Entity(id.0))
                .collect()),
            ValueSort::Text => Err(EvaluationError::NonEnumerableLocal(id.clone())),
            ValueSort::Symbol(domain) => self
                .symbols
                .get(domain)
                .map(|members| {
                    members
                        .iter()
                        .cloned()
                        .map(|member| Value::Symbol(SymbolValue::new(domain.clone(), member)))
                        .collect()
                })
                .ok_or_else(|| EvaluationError::UnknownSymbolDomain(domain.clone())),
        }
    }

    fn evaluate_condition(
        &self,
        condition: &Condition,
        schema: &AffordanceSchema,
        action: &GroundAction,
        outcome: Option<&ActionOutcome>,
        world: &World,
        locals: &HashMap<LocalId, Value>,
    ) -> Result<bool, EvaluationError> {
        match condition {
            Condition::RelationTarget {
                source,
                relation,
                target,
            } => {
                let source = self.entity(source, schema, action, outcome, locals)?;
                let entry = self
                    .relations
                    .get(relation)
                    .ok_or_else(|| EvaluationError::UnknownRelation(relation.clone()))?;
                let actual = (entry.read)(world, source);
                self.optional_entity(actual, target, schema, action, outcome, locals)
            }
            Condition::ComponentPresent {
                entity,
                component,
                present,
            } => {
                let entity = self.entity(entity, schema, action, outcome, locals)?;
                let entry = self
                    .components
                    .get(component)
                    .ok_or_else(|| EvaluationError::UnknownComponent(component.clone()))?;
                Ok((entry.read)(world, entity) == *present)
            }
            Condition::LocusOf { entity, locus } => {
                let entity = self.entity(entity, schema, action, outcome, locals)?;
                self.optional_entity(
                    world.enclosing_locus(entity),
                    locus,
                    schema,
                    action,
                    outcome,
                    locals,
                )
            }
            Condition::GaugeAtLeast {
                entity,
                gauge,
                region,
            } => self.gauge_threshold(
                entity, gauge, region, true, schema, action, outcome, world, locals,
            ),
            Condition::GaugeAtMost {
                entity,
                gauge,
                region,
            } => self.gauge_threshold(
                entity, gauge, region, false, schema, action, outcome, world, locals,
            ),
            Condition::Exists { entity, exists } => {
                let entity = self.entity(entity, schema, action, outcome, locals)?;
                Ok(world.contains(entity) == *exists)
            }
            Condition::Distinct { left, right } => Ok(self
                .resolve(left, schema, action, outcome, locals)?
                != self.resolve(right, schema, action, outcome, locals)?),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn gauge_threshold(
        &self,
        entity: &Term,
        gauge: &GaugeId,
        region: &GaugeRegionId,
        at_least: bool,
        schema: &AffordanceSchema,
        action: &GroundAction,
        outcome: Option<&ActionOutcome>,
        world: &World,
        locals: &HashMap<LocalId, Value>,
    ) -> Result<bool, EvaluationError> {
        let entity = self.entity(entity, schema, action, outcome, locals)?;
        let entry = self
            .gauges
            .get(gauge)
            .ok_or_else(|| EvaluationError::UnknownGauge(gauge.clone()))?;
        let target = entry.by_id.get(region).copied().ok_or_else(|| {
            EvaluationError::UnknownGaugeRegion {
                gauge: gauge.clone(),
                region: region.clone(),
            }
        })?;
        let Some(reading) = self.read_gauge(gauge, world, entity)? else {
            return Ok(false);
        };
        Ok(if at_least {
            reading.ordinal() >= target
        } else {
            reading.ordinal() <= target
        })
    }

    fn optional_entity(
        &self,
        actual: Option<EntityId>,
        constraint: &OptionalEntity,
        schema: &AffordanceSchema,
        action: &GroundAction,
        outcome: Option<&ActionOutcome>,
        locals: &HashMap<LocalId, Value>,
    ) -> Result<bool, EvaluationError> {
        match constraint {
            OptionalEntity::Is(term) => {
                Ok(actual == Some(self.entity(term, schema, action, outcome, locals)?))
            }
            OptionalEntity::IsUnset => Ok(actual.is_none()),
            OptionalEntity::IsNot(term) => {
                Ok(actual != Some(self.entity(term, schema, action, outcome, locals)?))
            }
        }
    }

    fn entity(
        &self,
        term: &Term,
        schema: &AffordanceSchema,
        action: &GroundAction,
        outcome: Option<&ActionOutcome>,
        locals: &HashMap<LocalId, Value>,
    ) -> Result<EntityId, EvaluationError> {
        let value = self.resolve(term, schema, action, outcome, locals)?;
        value
            .as_entity()
            .ok_or_else(|| EvaluationError::ExpectedEntity(value.sort()))
    }

    fn resolve(
        &self,
        term: &Term,
        schema: &AffordanceSchema,
        action: &GroundAction,
        outcome: Option<&ActionOutcome>,
        locals: &HashMap<LocalId, Value>,
    ) -> Result<Value, EvaluationError> {
        match term {
            Term::Actor => Ok(Value::Entity(action.actor())),
            Term::Input(id) => parameter_value(id, ParameterMode::Input, schema, action.inputs()),
            Term::Result(id) => parameter_value(
                id,
                ParameterMode::Result,
                schema,
                outcome.map(ActionOutcome::results).unwrap_or(&[]),
            ),
            Term::Local(id) => locals
                .get(id)
                .cloned()
                .ok_or_else(|| EvaluationError::UndeclaredLocal(id.clone())),
            Term::Constant(value) => Ok(value.clone()),
        }
    }
}

fn parameter_value(
    id: &ParameterId,
    expected_mode: ParameterMode,
    schema: &AffordanceSchema,
    values: &[Value],
) -> Result<Value, EvaluationError> {
    let parameter = schema
        .parameter(id)
        .ok_or_else(|| EvaluationError::UnknownParameter(id.clone()))?;
    if parameter.mode() != expected_mode {
        return Err(EvaluationError::WrongParameterMode {
            parameter: id.clone(),
            expected: expected_mode,
            actual: parameter.mode(),
        });
    }
    let value = values
        .get(parameter.slot() as usize)
        .cloned()
        .ok_or_else(|| EvaluationError::MissingParameterValue {
            parameter: id.clone(),
            mode: expected_mode,
            slot: parameter.slot(),
        })?;
    if value.sort() != *parameter.sort() {
        return Err(EvaluationError::WrongValueSort {
            parameter: id.clone(),
            expected: parameter.sort().clone(),
            actual: value.sort(),
        });
    }
    Ok(value)
}
