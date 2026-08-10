use std::collections::{HashMap, HashSet};

use crate::GaugeDirection;
use crate::schema::{
    AffordanceId, AffordanceSchema, Condition, Effect, Formula, OptionalEntity, ParameterMode,
    Resolution, Term, Value, ValueSort,
};
use crate::state::StateRegistry;

use super::{EffectKind, SchemaError};

pub(super) fn validate_schema(
    schema: &AffordanceSchema,
    state: &StateRegistry,
) -> Result<(), SchemaError> {
    let fail = |issue: String| SchemaError::new(schema.id().clone(), issue);
    if schema.display_name().trim().is_empty()
        || schema.display_name().chars().any(char::is_control)
    {
        return Err(fail(
            "display name must contain visible text and no control characters".into(),
        ));
    }

    let mut ids = HashSet::new();
    let mut input_slots = HashSet::new();
    let mut result_slots = HashSet::new();
    for parameter in schema.parameters() {
        if !ids.insert(parameter.id().clone()) {
            return Err(fail(format!("duplicate parameter id {}", parameter.id())));
        }
        validate_sort(parameter.sort(), state).map_err(&fail)?;
        let slots = match parameter.mode() {
            ParameterMode::Input => &mut input_slots,
            ParameterMode::Result => &mut result_slots,
        };
        if !slots.insert(parameter.slot()) {
            return Err(fail(format!(
                "duplicate {:?} slot {}",
                parameter.mode(),
                parameter.slot()
            )));
        }
    }
    validate_dense_slots(ParameterMode::Input, &input_slots).map_err(&fail)?;
    validate_dense_slots(ParameterMode::Result, &result_slots).map_err(&fail)?;

    for (index, guard) in schema.guards().iter().enumerate() {
        if guard.reason().trim().is_empty() || guard.reason().chars().any(char::is_control) {
            return Err(fail(format!(
                "guard {index} reason must contain visible text and no control characters"
            )));
        }
        validate_formula(guard.formula(), schema, state).map_err(&fail)?;
    }
    validate_effects(schema, state).map_err(&fail)
}

fn validate_dense_slots(mode: ParameterMode, slots: &HashSet<u16>) -> Result<(), String> {
    for expected in 0..slots.len() {
        let expected = u16::try_from(expected)
            .map_err(|_| format!("{mode:?} signature exceeds the u16 slot space"))?;
        if !slots.contains(&expected) {
            return Err(format!(
                "{mode:?} slots must be dense from zero; missing {expected}"
            ));
        }
    }
    Ok(())
}

fn validate_formula(
    formula: &Formula,
    schema: &AffordanceSchema,
    state: &StateRegistry,
) -> Result<(), String> {
    let mut locals = HashMap::new();
    for local in formula.locals() {
        if locals.insert(local.id().clone(), local.sort()).is_some() {
            return Err(format!("duplicate local {}", local.id()));
        }
        if *local.sort() == ValueSort::Text {
            return Err(format!("local {} has non-enumerable Text sort", local.id()));
        }
        validate_sort(local.sort(), state)?;
    }

    for condition in formula.conditions() {
        validate_condition(condition, schema, state, &locals)?;
    }
    Ok(())
}

fn validate_condition(
    condition: &Condition,
    schema: &AffordanceSchema,
    state: &StateRegistry,
    locals: &HashMap<crate::schema::LocalId, &ValueSort>,
) -> Result<(), String> {
    match condition {
        Condition::RelationTarget {
            source,
            relation,
            target,
        } => {
            require_entity(source, TermScope::Guard(locals), schema, state)?;
            if !state.has_relation(relation) {
                return Err(format!("unknown relation {relation}"));
            }
            validate_optional_entity(target, schema, state, locals)
        }
        Condition::ComponentPresent {
            entity, component, ..
        } => {
            require_entity(entity, TermScope::Guard(locals), schema, state)?;
            if !state.has_component(component) {
                return Err(format!("unknown component {component}"));
            }
            Ok(())
        }
        Condition::LocusOf { entity, locus } => {
            require_entity(entity, TermScope::Guard(locals), schema, state)?;
            validate_optional_entity(locus, schema, state, locals)
        }
        Condition::GaugeAtLeast {
            entity,
            gauge,
            region,
        }
        | Condition::GaugeAtMost {
            entity,
            gauge,
            region,
        } => {
            require_entity(entity, TermScope::Guard(locals), schema, state)?;
            if !state.has_gauge(gauge) {
                return Err(format!("unknown gauge {}", gauge.as_str()));
            }
            if !state.has_gauge_region(gauge, region) {
                return Err(format!(
                    "unknown region {region} for gauge {}",
                    gauge.as_str()
                ));
            }
            Ok(())
        }
        Condition::Exists { entity, exists } => {
            require_entity(entity, TermScope::Guard(locals), schema, state)?;
            if *exists && matches!(entity, Term::Input(_)) {
                return Err("positive Exists on an entity input repeats grounding liveness".into());
            }
            Ok(())
        }
        Condition::Distinct { left, right } => {
            let left = term_sort(left, TermScope::Guard(locals), schema, state)?;
            let right = term_sort(right, TermScope::Guard(locals), schema, state)?;
            if left != right {
                return Err(format!(
                    "Distinct compares incompatible sorts {left:?} and {right:?}"
                ));
            }
            Ok(())
        }
    }
}

fn validate_optional_entity(
    value: &OptionalEntity,
    schema: &AffordanceSchema,
    state: &StateRegistry,
    locals: &HashMap<crate::schema::LocalId, &ValueSort>,
) -> Result<(), String> {
    match value {
        OptionalEntity::Is(term) | OptionalEntity::IsNot(term) => {
            require_entity(term, TermScope::Guard(locals), schema, state)
        }
        OptionalEntity::IsUnset => Ok(()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Slot {
    Relation(Term, crate::schema::RelationId),
    Component(Term, crate::schema::ComponentId),
    Locus(Term),
    Gauge(Term, crate::GaugeId),
    Existence(Term),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Assignment {
    Entity(Option<Term>),
    Present(bool),
    Direction(GaugeDirection),
}

fn validate_effects(schema: &AffordanceSchema, state: &StateRegistry) -> Result<(), String> {
    let mut assignments = HashMap::new();
    let mut creates = HashSet::new();
    for effect in schema.effects() {
        let (slot, assignment) = match effect {
            Effect::SetRelation {
                source,
                relation,
                target,
            } => {
                require_entity(source, TermScope::Effect, schema, state)?;
                require_entity(target, TermScope::Effect, schema, state)?;
                if !state.has_relation(relation) {
                    return Err(format!("unknown relation {relation}"));
                }
                (
                    Slot::Relation(source.clone(), relation.clone()),
                    Assignment::Entity(Some(target.clone())),
                )
            }
            Effect::ClearRelation { source, relation } => {
                require_entity(source, TermScope::Effect, schema, state)?;
                if !state.has_relation(relation) {
                    return Err(format!("unknown relation {relation}"));
                }
                (
                    Slot::Relation(source.clone(), relation.clone()),
                    Assignment::Entity(None),
                )
            }
            Effect::SetComponent { entity, component } => {
                require_entity(entity, TermScope::Effect, schema, state)?;
                if !state.has_component(component) {
                    return Err(format!("unknown component {component}"));
                }
                (
                    Slot::Component(entity.clone(), component.clone()),
                    Assignment::Present(true),
                )
            }
            Effect::RemoveComponent { entity, component } => {
                require_entity(entity, TermScope::Effect, schema, state)?;
                if !state.has_component(component) {
                    return Err(format!("unknown component {component}"));
                }
                (
                    Slot::Component(entity.clone(), component.clone()),
                    Assignment::Present(false),
                )
            }
            Effect::SetLocus { entity, locus } => {
                require_entity(entity, TermScope::Effect, schema, state)?;
                require_entity(locus, TermScope::Effect, schema, state)?;
                (
                    Slot::Locus(entity.clone()),
                    Assignment::Entity(Some(locus.clone())),
                )
            }
            Effect::ClearLocus { entity } => {
                require_entity(entity, TermScope::Effect, schema, state)?;
                (Slot::Locus(entity.clone()), Assignment::Entity(None))
            }
            Effect::ShiftGauge {
                entity,
                gauge,
                direction,
            } => {
                require_entity(entity, TermScope::Effect, schema, state)?;
                if !state.has_gauge(gauge) {
                    return Err(format!("unknown gauge {}", gauge.as_str()));
                }
                (
                    Slot::Gauge(entity.clone(), gauge.clone()),
                    Assignment::Direction(*direction),
                )
            }
            Effect::Create { result } => {
                let parameter = schema
                    .parameter(result)
                    .ok_or_else(|| format!("Create refers to undeclared result {result}"))?;
                if parameter.mode() != ParameterMode::Result
                    || *parameter.sort() != ValueSort::Entity
                {
                    return Err(format!("Create target {result} must be an entity result"));
                }
                if !creates.insert(result.clone()) {
                    return Err(format!("result {result} is created more than once"));
                }
                (
                    Slot::Existence(Term::Result(result.clone())),
                    Assignment::Present(true),
                )
            }
            Effect::Destroy { entity } => {
                require_entity(entity, TermScope::Effect, schema, state)?;
                (Slot::Existence(entity.clone()), Assignment::Present(false))
            }
        };
        if let Some(existing) = assignments.insert(slot.clone(), assignment.clone())
            && existing != assignment
        {
            return Err(format!("effects make incompatible assignments to {slot:?}"));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum TermScope<'a> {
    Guard(&'a HashMap<crate::schema::LocalId, &'a ValueSort>),
    Effect,
}

fn require_entity(
    term: &Term,
    scope: TermScope<'_>,
    schema: &AffordanceSchema,
    state: &StateRegistry,
) -> Result<(), String> {
    let sort = term_sort(term, scope, schema, state)?;
    if sort != ValueSort::Entity {
        return Err(format!("state position requires Entity, got {sort:?}"));
    }
    Ok(())
}

fn term_sort(
    term: &Term,
    scope: TermScope<'_>,
    schema: &AffordanceSchema,
    state: &StateRegistry,
) -> Result<ValueSort, String> {
    match term {
        Term::Actor => Ok(ValueSort::Entity),
        Term::Input(id) => {
            let parameter = schema
                .parameter(id)
                .ok_or_else(|| format!("undeclared input {id}"))?;
            if parameter.mode() != ParameterMode::Input {
                return Err(format!("{id} is not an input parameter"));
            }
            Ok(parameter.sort().clone())
        }
        Term::Result(id) => {
            if matches!(scope, TermScope::Guard(_)) {
                return Err(format!("result {id} is not available in guards"));
            }
            let parameter = schema
                .parameter(id)
                .ok_or_else(|| format!("undeclared result {id}"))?;
            if parameter.mode() != ParameterMode::Result {
                return Err(format!("{id} is not a result parameter"));
            }
            Ok(parameter.sort().clone())
        }
        Term::Local(id) => match scope {
            TermScope::Guard(locals) => locals
                .get(id)
                .map(|sort| (*sort).clone())
                .ok_or_else(|| format!("undeclared local {id}")),
            TermScope::Effect => Err(format!("local {id} cannot escape into effects")),
        },
        Term::Constant(value) => {
            if let Value::Symbol(symbol) = value
                && !state.has_symbol(symbol)
            {
                return Err(format!(
                    "unknown symbol {} in domain {}",
                    symbol.value(),
                    symbol.domain()
                ));
            }
            Ok(value.sort())
        }
    }
}

fn validate_sort(sort: &ValueSort, state: &StateRegistry) -> Result<(), String> {
    if let ValueSort::Symbol(domain) = sort
        && !state.has_symbol_domain(domain)
    {
        return Err(format!("unknown symbol domain {domain}"));
    }
    Ok(())
}

pub(super) fn effect_index<'a>(
    schemas: impl Iterator<Item = &'a AffordanceSchema>,
) -> HashMap<EffectKind, Box<[AffordanceId]>> {
    let mut index: HashMap<EffectKind, Vec<AffordanceId>> = HashMap::new();
    for schema in schemas {
        if schema.resolution() == Resolution::Opaque {
            continue;
        }
        let mut seen = HashSet::new();
        for effect in schema.effects() {
            if effect_contains_result(effect) {
                continue;
            }
            let kind = match effect {
                Effect::SetRelation { relation, .. } | Effect::ClearRelation { relation, .. } => {
                    EffectKind::Relation(relation.clone())
                }
                Effect::SetComponent { component, .. }
                | Effect::RemoveComponent { component, .. } => {
                    EffectKind::Component(component.clone())
                }
                Effect::SetLocus { .. } | Effect::ClearLocus { .. } => EffectKind::Locus,
                Effect::ShiftGauge { gauge, .. } => EffectKind::Gauge(gauge.clone()),
                Effect::Create { .. } | Effect::Destroy { .. } => EffectKind::Existence,
            };
            if seen.insert(kind.clone()) {
                index.entry(kind).or_default().push(schema.id().clone());
            }
        }
    }
    index
        .into_iter()
        .map(|(kind, mut ids)| {
            ids.sort();
            (kind, ids.into_boxed_slice())
        })
        .collect()
}

fn effect_contains_result(effect: &Effect) -> bool {
    match effect {
        Effect::SetRelation { source, target, .. } => {
            term_is_result(source) || term_is_result(target)
        }
        Effect::ClearRelation { source, .. }
        | Effect::SetComponent { entity: source, .. }
        | Effect::RemoveComponent { entity: source, .. }
        | Effect::ClearLocus { entity: source }
        | Effect::ShiftGauge { entity: source, .. }
        | Effect::Destroy { entity: source } => term_is_result(source),
        Effect::SetLocus { entity, locus } => term_is_result(entity) || term_is_result(locus),
        Effect::Create { .. } => true,
    }
}

fn term_is_result(term: &Term) -> bool {
    matches!(term, Term::Result(_))
}
