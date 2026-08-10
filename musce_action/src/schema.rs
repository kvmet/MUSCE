//! Canonical affordance schema and grounding values.
//!
//! These types are additive while the fixed-frame prototype is migrated. Stable
//! symbolic ids cross process and wire boundaries; compact parameter slots index
//! runtime input/result arrays. The two identities are deliberately separate so
//! reordering storage never renames an authored parameter.

use std::fmt;

use musce_core::EntityId;

use crate::{Gate, GaugeDirection, GaugeId};

/// A malformed stable symbolic identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameError {
    kind: &'static str,
    value: String,
    rule: &'static str,
}

impl NameError {
    fn new(kind: &'static str, value: impl Into<String>, rule: &'static str) -> Self {
        Self {
            kind,
            value: value.into(),
            rule,
        }
    }

    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {} {:?}: {}", self.kind, self.value, self.rule)
    }
}

impl std::error::Error for NameError {}

fn validate_name(kind: &'static str, value: impl Into<String>) -> Result<Box<str>, NameError> {
    let value = value.into();
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(NameError::new(
            kind,
            value,
            "names must be nonempty and contain no whitespace or control characters",
        ));
    }
    Ok(value.into_boxed_str())
}

fn validate_label(value: impl Into<String>) -> Result<Box<str>, NameError> {
    let value = value.into();
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(NameError::new(
            "parameter label",
            value,
            "labels must contain visible text and no control characters",
        ));
    }
    Ok(value.into_boxed_str())
}

macro_rules! symbolic_id {
    ($name:ident, $kind:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Box<str>);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, NameError> {
                validate_name($kind, value).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = NameError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = NameError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }
    };
}

symbolic_id!(AffordanceId, "affordance id");
symbolic_id!(ParameterId, "parameter id");
symbolic_id!(LocalId, "local id");
symbolic_id!(RelationId, "relation id");
symbolic_id!(ComponentId, "component id");
symbolic_id!(GaugeRegionId, "gauge region id");
symbolic_id!(SymbolDomainId, "symbol domain id");
symbolic_id!(SymbolId, "symbol id");

/// The runtime kind of a canonical affordance value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ValueSort {
    Entity,
    Text,
    Symbol(SymbolDomainId),
}

/// One member of a registered finite symbolic domain.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SymbolValue {
    domain: SymbolDomainId,
    value: SymbolId,
}

impl SymbolValue {
    pub fn new(domain: SymbolDomainId, value: SymbolId) -> Self {
        Self { domain, value }
    }

    pub fn domain(&self) -> &SymbolDomainId {
        &self.domain
    }

    pub fn value(&self) -> &SymbolId {
        &self.value
    }
}

/// A type-erased input, result, local, or constant value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Value {
    Entity(EntityId),
    Text(Box<str>),
    Symbol(SymbolValue),
}

impl Value {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into().into_boxed_str())
    }

    pub fn sort(&self) -> ValueSort {
        match self {
            Value::Entity(_) => ValueSort::Entity,
            Value::Text(_) => ValueSort::Text,
            Value::Symbol(value) => ValueSort::Symbol(value.domain.clone()),
        }
    }

    pub fn as_entity(&self) -> Option<EntityId> {
        match self {
            Value::Entity(entity) => Some(*entity),
            _ => None,
        }
    }
}

impl From<EntityId> for Value {
    fn from(value: EntityId) -> Self {
        Self::Entity(value)
    }
}

/// Whether a parameter is supplied before execution or produced by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParameterMode {
    Input,
    Result,
}

/// One action-local input or result declaration.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Parameter {
    id: ParameterId,
    label: Box<str>,
    sort: ValueSort,
    mode: ParameterMode,
    slot: u16,
}

impl Parameter {
    pub fn new(
        id: ParameterId,
        label: impl Into<String>,
        sort: ValueSort,
        mode: ParameterMode,
        slot: u16,
    ) -> Result<Self, NameError> {
        Ok(Self {
            id,
            label: validate_label(label)?,
            sort,
            mode,
            slot,
        })
    }

    pub fn id(&self) -> &ParameterId {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn sort(&self) -> &ValueSort {
        &self.sort
    }

    pub fn mode(&self) -> ParameterMode {
        self.mode
    }

    pub fn slot(&self) -> u16 {
        self.slot
    }
}

/// An existential value declaration scoped to one formula.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Local {
    id: LocalId,
    sort: ValueSort,
}

impl Local {
    pub fn new(id: LocalId, sort: ValueSort) -> Self {
        Self { id, sort }
    }

    pub fn id(&self) -> &LocalId {
        &self.id
    }

    pub fn sort(&self) -> &ValueSort {
        &self.sort
    }
}

/// A value reference inside a condition or effect.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Term {
    Actor,
    Input(ParameterId),
    Result(ParameterId),
    Local(LocalId),
    Constant(Value),
}

/// A constraint on an optional entity-valued functional slot.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OptionalEntity {
    Is(Term),
    IsUnset,
    IsNot(Term),
}

/// One truth-valued member of the closed affordance state algebra.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Condition {
    RelationTarget {
        source: Term,
        relation: RelationId,
        target: OptionalEntity,
    },
    ComponentPresent {
        entity: Term,
        component: ComponentId,
        present: bool,
    },
    LocusOf {
        entity: Term,
        locus: OptionalEntity,
    },
    GaugeAtLeast {
        entity: Term,
        gauge: GaugeId,
        region: GaugeRegionId,
    },
    GaugeAtMost {
        entity: Term,
        gauge: GaugeId,
        region: GaugeRegionId,
    },
    Exists {
        entity: Term,
        exists: bool,
    },
    Distinct {
        left: Term,
        right: Term,
    },
}

/// A conjunction with formula-local existential witnesses.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Formula {
    locals: Box<[Local]>,
    conditions: Box<[Condition]>,
}

impl Formula {
    pub fn new(locals: impl Into<Box<[Local]>>, conditions: impl Into<Box<[Condition]>>) -> Self {
        Self {
            locals: locals.into(),
            conditions: conditions.into(),
        }
    }

    pub fn all(conditions: impl Into<Box<[Condition]>>) -> Self {
        Self::new(Vec::new().into_boxed_slice(), conditions)
    }

    pub fn locals(&self) -> &[Local] {
        &self.locals
    }

    pub fn conditions(&self) -> &[Condition] {
        &self.conditions
    }
}

/// One unconditional successful transition promised by an affordance.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    SetRelation {
        source: Term,
        relation: RelationId,
        target: Term,
    },
    ClearRelation {
        source: Term,
        relation: RelationId,
    },
    SetComponent {
        entity: Term,
        component: ComponentId,
    },
    RemoveComponent {
        entity: Term,
        component: ComponentId,
    },
    SetLocus {
        entity: Term,
        locus: Term,
    },
    ClearLocus {
        entity: Term,
    },
    ShiftGauge {
        entity: Term,
        gauge: GaugeId,
        direction: GaugeDirection,
    },
    Create {
        result: ParameterId,
    },
    Destroy {
        entity: Term,
    },
}

/// What true guards promise about resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resolution {
    Deterministic,
    Contested,
    Opaque,
}

/// One ordered player-facing applicability requirement.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Guard {
    formula: Formula,
    reason: Box<str>,
}

impl Guard {
    pub fn new(formula: Formula, reason: impl Into<String>) -> Self {
        Self {
            formula,
            reason: reason.into().into_boxed_str(),
        }
    }

    pub fn formula(&self) -> &Formula {
        &self.formula
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The declarative, implementation-independent shape of one affordance.
#[derive(Clone, Debug)]
pub struct AffordanceSchema {
    id: AffordanceId,
    display_name: Box<str>,
    parameters: Box<[Parameter]>,
    guards: Box<[Guard]>,
    effects: Box<[Effect]>,
    gate: Gate,
    resolution: Resolution,
}

impl AffordanceSchema {
    pub fn new(
        id: AffordanceId,
        display_name: impl Into<String>,
        parameters: impl Into<Box<[Parameter]>>,
        guards: impl Into<Box<[Guard]>>,
        effects: impl Into<Box<[Effect]>>,
        gate: Gate,
        resolution: Resolution,
    ) -> Self {
        Self {
            id,
            display_name: display_name.into().into_boxed_str(),
            parameters: parameters.into(),
            guards: guards.into(),
            effects: effects.into(),
            gate,
            resolution,
        }
    }

    pub fn id(&self) -> &AffordanceId {
        &self.id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn parameters(&self) -> &[Parameter] {
        &self.parameters
    }

    pub fn guards(&self) -> &[Guard] {
        &self.guards
    }

    pub fn effects(&self) -> &[Effect] {
        &self.effects
    }

    pub fn gate(&self) -> Gate {
        self.gate
    }

    pub fn resolution(&self) -> Resolution {
        self.resolution
    }

    pub fn parameter(&self, id: &ParameterId) -> Option<&Parameter> {
        self.parameters
            .iter()
            .find(|parameter| parameter.id() == id)
    }

    pub fn inputs(&self) -> impl Iterator<Item = &Parameter> {
        self.parameters
            .iter()
            .filter(|parameter| parameter.mode() == ParameterMode::Input)
    }

    pub fn results(&self) -> impl Iterator<Item = &Parameter> {
        self.parameters
            .iter()
            .filter(|parameter| parameter.mode() == ParameterMode::Result)
    }
}

/// One parameter/value pair used by partial grounding and the target wire.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ParameterBinding {
    parameter: ParameterId,
    value: Value,
}

impl ParameterBinding {
    pub fn new(parameter: ParameterId, value: Value) -> Self {
        Self { parameter, value }
    }

    pub fn parameter(&self) -> &ParameterId {
        &self.parameter
    }

    pub fn value(&self) -> &Value {
        &self.value
    }
}

/// An affordance plus any input bindings a front end has supplied so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialGrounding {
    affordance: AffordanceId,
    bindings: Box<[ParameterBinding]>,
}

impl PartialGrounding {
    pub fn new(affordance: AffordanceId, bindings: impl Into<Box<[ParameterBinding]>>) -> Self {
        Self {
            affordance,
            bindings: bindings.into(),
        }
    }

    pub fn affordance(&self) -> &AffordanceId {
        &self.affordance
    }

    pub fn bindings(&self) -> &[ParameterBinding] {
        &self.bindings
    }
}

/// A complete action occurrence. Actor is privileged and never an input slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroundAction {
    affordance: AffordanceId,
    actor: EntityId,
    inputs: Box<[Value]>,
}

impl GroundAction {
    pub fn new(affordance: AffordanceId, actor: EntityId, inputs: impl Into<Box<[Value]>>) -> Self {
        Self {
            affordance,
            actor,
            inputs: inputs.into(),
        }
    }

    pub fn affordance(&self) -> &AffordanceId {
        &self.affordance
    }

    pub fn actor(&self) -> EntityId {
        self.actor
    }

    pub fn inputs(&self) -> &[Value] {
        &self.inputs
    }
}

/// Values produced by a successful action, ordered by result slot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionOutcome {
    results: Box<[Value]>,
}

impl ActionOutcome {
    pub fn new(results: impl Into<Box<[Value]>>) -> Self {
        Self {
            results: results.into(),
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn results(&self) -> &[Value] {
        &self.results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> ParameterId {
        ParameterId::new(value).unwrap()
    }

    #[test]
    fn stable_ids_reject_ambiguous_names() {
        assert!(AffordanceId::new("").is_err());
        assert!(AffordanceId::new("two words").is_err());
        assert!(AffordanceId::new("core:take").is_ok());
        assert!(ParameterId::new("destination").is_ok());
    }

    #[test]
    fn symbol_values_retain_their_domain() {
        let direction = SymbolDomainId::new("direction").unwrap();
        let north = SymbolId::new("north").unwrap();
        let value = Value::Symbol(SymbolValue::new(direction.clone(), north));
        assert_eq!(value.sort(), ValueSort::Symbol(direction));
    }

    #[test]
    fn actor_is_not_an_input_slot() {
        let action = GroundAction::new(
            AffordanceId::new("take").unwrap(),
            EntityId(7),
            vec![Value::Entity(EntityId(9))],
        );
        assert_eq!(action.actor(), EntityId(7));
        assert_eq!(action.inputs(), [Value::Entity(EntityId(9))]);
    }

    #[test]
    fn parameter_identity_and_dense_slot_are_independent() {
        let parameter = Parameter::new(
            id("destination"),
            "Destination room",
            ValueSort::Entity,
            ParameterMode::Input,
            2,
        )
        .unwrap();
        assert_eq!(parameter.id().as_str(), "destination");
        assert_eq!(parameter.label(), "Destination room");
        assert_eq!(parameter.slot(), 2);
    }

    #[test]
    fn partial_grounding_names_each_binding() {
        let partial = PartialGrounding::new(
            AffordanceId::new("put").unwrap(),
            vec![ParameterBinding::new(
                id("container"),
                Value::Entity(EntityId(4)),
            )],
        );
        assert_eq!(partial.bindings()[0].parameter().as_str(), "container");
    }
}
