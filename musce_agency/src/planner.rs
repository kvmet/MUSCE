//! Bounded backward regression over canonical affordance schemas.

use std::collections::HashMap;

use musce_action::schema::{
    AffordanceId, AffordanceSchema, Condition, Effect, Formula, GroundAction, LocalId,
    OptionalEntity, ParameterId, Resolution, Term, Value, ValueSort,
};
use musce_action::{AffordanceRegistry, Gate};
use musce_core::{EntityId, World};

use crate::{Cost, CostModel};

/// One executable plan beat. Canonical ground actions are used directly so no
/// planner-only frame or app lowering step can drift from execution.
pub type Step = GroundAction;

/// A transient ordered action sequence.
pub type Plan = Vec<Step>;

const MAX_DEPTH: usize = 8;
const MAX_SETTLED: usize = 10_000;

/// A planner retains only cost policy. Registry and world are per-query borrows,
/// allowing a caller to release them before executing the returned action.
pub struct Planner<'a> {
    cost: &'a dyn CostModel,
}

impl<'a> Planner<'a> {
    pub fn new(cost: &'a dyn CostModel) -> Self {
        Self { cost }
    }

    pub fn plan(
        &self,
        registry: &AffordanceRegistry,
        actor: EntityId,
        goal: &Formula,
        known: &[EntityId],
        world: &World,
    ) -> Option<Plan> {
        self.plan_excluding(registry, actor, goal, known, world, &[])
    }

    pub fn plan_excluding(
        &self,
        registry: &AffordanceRegistry,
        actor: EntityId,
        goal: &Formula,
        known: &[EntityId],
        world: &World,
        excluded: &[Step],
    ) -> Option<Plan> {
        let mut settled = 0;
        ground_goals(goal, known)
            .into_iter()
            .filter_map(|conditions| {
                let mut visited = HashMap::new();
                regress(
                    Search {
                        registry,
                        actor,
                        known,
                        world,
                        excluded,
                        cost: self.cost,
                    },
                    conditions,
                    Vec::new(),
                    0,
                    &mut settled,
                    &mut visited,
                )
            })
            .min_by_key(|(cost, _)| *cost)
            .map(|(_, plan)| plan)
    }

    pub fn goal_holds(
        &self,
        registry: &AffordanceRegistry,
        actor: EntityId,
        goal: &Formula,
        known: &[EntityId],
        world: &World,
    ) -> bool {
        ground_goals(goal, known).into_iter().any(|conditions| {
            conditions
                .iter()
                .all(|condition| holds(registry, actor, condition, world))
        })
    }
}

#[derive(Clone, Copy)]
struct Search<'a> {
    registry: &'a AffordanceRegistry,
    actor: EntityId,
    known: &'a [EntityId],
    world: &'a World,
    excluded: &'a [Step],
    cost: &'a dyn CostModel,
}

#[allow(clippy::too_many_arguments)]
fn regress(
    search: Search<'_>,
    conditions: Vec<Condition>,
    plan: Plan,
    cost: Cost,
    settled: &mut usize,
    visited: &mut HashMap<Vec<Condition>, Cost>,
) -> Option<(Cost, Plan)> {
    if plan.len() > MAX_DEPTH || *settled >= MAX_SETTLED {
        return None;
    }
    if visited
        .get(&conditions)
        .is_some_and(|settled_cost| *settled_cost <= cost)
    {
        return None;
    }
    visited.insert(conditions.clone(), cost);
    *settled += 1;

    let Some((at, goal)) = conditions
        .iter()
        .enumerate()
        .find(|(_, condition)| !holds(search.registry, search.actor, condition, search.world))
    else {
        return Some((cost, plan));
    };

    let mut best = None;
    for schema in search.registry.schemas() {
        if schema.resolution() == Resolution::Opaque {
            continue;
        }
        for effect in schema.effects() {
            let Some(seed) = match_effect(effect, goal, search.actor) else {
                continue;
            };
            for grounding in ground_inputs(schema, seed, &conditions, search) {
                let action = GroundAction::new(
                    schema.id().clone(),
                    search.actor,
                    input_values(schema, &grounding),
                );
                if search.excluded.contains(&action) {
                    continue;
                }

                let mut next = conditions.clone();
                next.remove(at);
                let mut applicable = true;
                for guard in schema.guards() {
                    match search.registry.state().evaluate(
                        guard.formula(),
                        schema,
                        &action,
                        None,
                        search.world,
                    ) {
                        Ok(true) => {}
                        Ok(false) if guard.formula().locals().is_empty() => {
                            for condition in guard.formula().conditions() {
                                let Some(condition) =
                                    ground_condition(condition, schema, &grounding, search.actor)
                                else {
                                    applicable = false;
                                    break;
                                };
                                if !next.contains(&condition) {
                                    next.push(condition);
                                }
                            }
                        }
                        Ok(false) | Err(_) => applicable = false,
                    }
                    if !applicable {
                        break;
                    }
                }
                if !applicable {
                    continue;
                }

                let mut next_plan = plan.clone();
                next_plan.insert(0, action.clone());
                let next_cost = cost.saturating_add(search.cost.cost(
                    search.actor,
                    schema,
                    &action,
                    search.world,
                ));
                if let Some(candidate) =
                    regress(search, next, next_plan, next_cost, settled, visited)
                    && best
                        .as_ref()
                        .is_none_or(|(best_cost, _)| candidate.0 < *best_cost)
                {
                    best = Some(candidate);
                }
            }
        }
    }
    best
}

fn ground_goals(goal: &Formula, known: &[EntityId]) -> Vec<Vec<Condition>> {
    fn visit(
        at: usize,
        goal: &Formula,
        known: &[EntityId],
        bindings: &mut HashMap<LocalId, Value>,
        out: &mut Vec<Vec<Condition>>,
    ) {
        let Some(local) = goal.locals().get(at) else {
            let conditions = goal
                .conditions()
                .iter()
                .map(|condition| substitute_locals(condition, bindings))
                .collect::<Option<Vec<_>>>();
            if let Some(conditions) = conditions {
                out.push(conditions);
            }
            return;
        };
        if local.sort() != &ValueSort::Entity {
            return;
        }
        for candidate in known {
            bindings.insert(local.id().clone(), Value::Entity(*candidate));
            visit(at + 1, goal, known, bindings, out);
        }
        bindings.remove(local.id());
    }

    let mut out = Vec::new();
    visit(0, goal, known, &mut HashMap::new(), &mut out);
    out
}

fn substitute_locals(
    condition: &Condition,
    bindings: &HashMap<musce_action::schema::LocalId, Value>,
) -> Option<Condition> {
    map_condition(condition, &mut |term| match term {
        Term::Local(id) => bindings.get(id).cloned().map(Term::Constant),
        other => Some(other.clone()),
    })
}

fn holds(
    registry: &AffordanceRegistry,
    actor: EntityId,
    condition: &Condition,
    world: &World,
) -> bool {
    let schema = AffordanceSchema::new(
        AffordanceId::new("agency:goal").expect("static id"),
        "agency goal",
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Gate::Open,
        Resolution::Opaque,
    );
    let action = GroundAction::new(schema.id().clone(), actor, Vec::new());
    registry
        .state()
        .evaluate(
            &Formula::all(vec![condition.clone()]),
            &schema,
            &action,
            None,
            world,
        )
        .unwrap_or(false)
}

fn match_effect(
    effect: &Effect,
    goal: &Condition,
    actor: EntityId,
) -> Option<HashMap<ParameterId, Value>> {
    let mut bindings = HashMap::new();
    let matched = match (effect, goal) {
        (
            Effect::SetRelation {
                source,
                relation,
                target,
            },
            Condition::RelationTarget {
                source: wanted_source,
                relation: wanted_relation,
                target: OptionalEntity::Is(wanted_target),
            },
        ) if relation == wanted_relation => {
            unify(source, wanted_source, actor, &mut bindings)
                && unify(target, wanted_target, actor, &mut bindings)
        }
        (
            Effect::ClearRelation { source, relation },
            Condition::RelationTarget {
                source: wanted_source,
                relation: wanted_relation,
                target: OptionalEntity::IsUnset,
            },
        ) if relation == wanted_relation => unify(source, wanted_source, actor, &mut bindings),
        (
            Effect::SetComponent { entity, component },
            Condition::ComponentPresent {
                entity: wanted,
                component: wanted_component,
                present: true,
            },
        ) if component == wanted_component => unify(entity, wanted, actor, &mut bindings),
        (
            Effect::RemoveComponent { entity, component },
            Condition::ComponentPresent {
                entity: wanted,
                component: wanted_component,
                present: false,
            },
        ) if component == wanted_component => unify(entity, wanted, actor, &mut bindings),
        (
            Effect::SetLocus { entity, locus },
            Condition::LocusOf {
                entity: wanted_entity,
                locus: OptionalEntity::Is(wanted_locus),
            },
        ) => {
            unify(entity, wanted_entity, actor, &mut bindings)
                && unify(locus, wanted_locus, actor, &mut bindings)
        }
        (
            Effect::ClearLocus { entity },
            Condition::LocusOf {
                entity: wanted,
                locus: OptionalEntity::IsUnset,
            },
        ) => unify(entity, wanted, actor, &mut bindings),
        (
            Effect::Destroy { entity },
            Condition::Exists {
                entity: wanted,
                exists: false,
            },
        ) => unify(entity, wanted, actor, &mut bindings),
        _ => false,
    };
    matched.then_some(bindings)
}

fn unify(
    offered: &Term,
    wanted: &Term,
    actor: EntityId,
    bindings: &mut HashMap<ParameterId, Value>,
) -> bool {
    let Some(wanted) = ground_term(wanted, actor) else {
        return false;
    };
    match offered {
        Term::Actor => wanted == Value::Entity(actor),
        Term::Input(id) => match bindings.get(id) {
            Some(bound) => *bound == wanted,
            None => {
                bindings.insert(id.clone(), wanted);
                true
            }
        },
        Term::Constant(value) => *value == wanted,
        Term::Result(_) | Term::Local(_) => false,
    }
}

fn ground_inputs(
    schema: &AffordanceSchema,
    seed: HashMap<ParameterId, Value>,
    conditions: &[Condition],
    search: Search<'_>,
) -> Vec<HashMap<ParameterId, Value>> {
    fn visit(
        at: usize,
        inputs: &[&musce_action::schema::Parameter],
        candidates: &[Value],
        bindings: &mut HashMap<ParameterId, Value>,
        out: &mut Vec<HashMap<ParameterId, Value>>,
    ) {
        let Some(parameter) = inputs.get(at) else {
            out.push(bindings.clone());
            return;
        };
        if let Some(bound) = bindings.get(parameter.id()) {
            if bound.sort() == *parameter.sort() {
                visit(at + 1, inputs, candidates, bindings, out);
            }
            return;
        }
        for candidate in candidates
            .iter()
            .filter(|value| value.sort() == *parameter.sort())
        {
            bindings.insert(parameter.id().clone(), candidate.clone());
            visit(at + 1, inputs, candidates, bindings, out);
        }
        bindings.remove(parameter.id());
    }

    let mut entities: Vec<EntityId> = search.known.to_vec();
    for condition in conditions {
        collect_entities(condition, &mut entities);
    }
    entities.sort();
    entities.dedup();
    let candidates: Vec<Value> = entities.into_iter().map(Value::Entity).collect();
    let inputs: Vec<_> = schema.inputs().collect();
    let mut out = Vec::new();
    visit(0, &inputs, &candidates, &mut seed.clone(), &mut out);
    out
}

fn input_values(schema: &AffordanceSchema, bindings: &HashMap<ParameterId, Value>) -> Vec<Value> {
    let mut values = vec![Value::Entity(EntityId(0)); schema.inputs().count()];
    for parameter in schema.inputs() {
        values[parameter.slot() as usize] = bindings
            .get(parameter.id())
            .expect("ground_inputs binds every input")
            .clone();
    }
    values
}

fn ground_condition(
    condition: &Condition,
    schema: &AffordanceSchema,
    bindings: &HashMap<ParameterId, Value>,
    actor: EntityId,
) -> Option<Condition> {
    map_condition(condition, &mut |term| match term {
        Term::Actor => Some(Term::Actor),
        Term::Input(id) => bindings.get(id).cloned().map(Term::Constant),
        Term::Constant(value) => Some(Term::Constant(value.clone())),
        Term::Result(_) | Term::Local(_) => None,
    })
    .filter(|_| schema.inputs().all(|p| bindings.contains_key(p.id())))
    .map(|condition| normalize_actor(condition, actor))
}

fn normalize_actor(condition: Condition, actor: EntityId) -> Condition {
    map_condition(&condition, &mut |term| match term {
        Term::Actor => Some(Term::Constant(Value::Entity(actor))),
        other => Some(other.clone()),
    })
    .expect("normalization is total")
}

fn map_condition(
    condition: &Condition,
    term: &mut impl FnMut(&Term) -> Option<Term>,
) -> Option<Condition> {
    let optional = |value: &OptionalEntity, term: &mut dyn FnMut(&Term) -> Option<Term>| {
        Some(match value {
            OptionalEntity::Is(value) => OptionalEntity::Is(term(value)?),
            OptionalEntity::IsNot(value) => OptionalEntity::IsNot(term(value)?),
            OptionalEntity::IsUnset => OptionalEntity::IsUnset,
        })
    };
    Some(match condition {
        Condition::RelationTarget {
            source,
            relation,
            target,
        } => Condition::RelationTarget {
            source: term(source)?,
            relation: relation.clone(),
            target: optional(target, term)?,
        },
        Condition::ComponentPresent {
            entity,
            component,
            present,
        } => Condition::ComponentPresent {
            entity: term(entity)?,
            component: component.clone(),
            present: *present,
        },
        Condition::LocusOf { entity, locus } => Condition::LocusOf {
            entity: term(entity)?,
            locus: optional(locus, term)?,
        },
        Condition::GaugeAtLeast {
            entity,
            gauge,
            region,
        } => Condition::GaugeAtLeast {
            entity: term(entity)?,
            gauge: gauge.clone(),
            region: region.clone(),
        },
        Condition::GaugeAtMost {
            entity,
            gauge,
            region,
        } => Condition::GaugeAtMost {
            entity: term(entity)?,
            gauge: gauge.clone(),
            region: region.clone(),
        },
        Condition::Exists { entity, exists } => Condition::Exists {
            entity: term(entity)?,
            exists: *exists,
        },
        Condition::Distinct { left, right } => Condition::Distinct {
            left: term(left)?,
            right: term(right)?,
        },
    })
}

fn ground_term(term: &Term, actor: EntityId) -> Option<Value> {
    match term {
        Term::Actor => Some(Value::Entity(actor)),
        Term::Constant(value) => Some(value.clone()),
        Term::Input(_) | Term::Result(_) | Term::Local(_) => None,
    }
}

fn collect_entities(condition: &Condition, out: &mut Vec<EntityId>) {
    let mut collect = |term: &Term| {
        if let Term::Constant(Value::Entity(entity)) = term {
            out.push(*entity);
        }
    };
    match condition {
        Condition::RelationTarget { source, target, .. } => {
            collect(source);
            if let OptionalEntity::Is(value) | OptionalEntity::IsNot(value) = target {
                collect(value);
            }
        }
        Condition::ComponentPresent { entity, .. }
        | Condition::GaugeAtLeast { entity, .. }
        | Condition::GaugeAtMost { entity, .. }
        | Condition::Exists { entity, .. } => collect(entity),
        Condition::LocusOf { entity, locus } => {
            collect(entity);
            if let OptionalEntity::Is(value) | OptionalEntity::IsNot(value) = locus {
                collect(value);
            }
        }
        Condition::Distinct { left, right } => {
            collect(left);
            collect(right);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UnitCost;
    use musce_action::schema::{
        ActionOutcome, ComponentId, Guard, Parameter, ParameterMode, RelationId,
    };
    use musce_action::state::StateRegistry;
    use musce_action::{AffordanceRegistryBuilder, HandlerOutcome};
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Containment, Locus, NamedComponent, Relation};

    struct Fixture {
        world: World,
        registry: AffordanceRegistry,
        actor: EntityId,
        first: EntityId,
        second: EntityId,
        goal: Formula,
    }

    fn fixture(held: bool) -> Fixture {
        let mut world = World::new();
        let actor = world.spawn(EntityBuilder::new());
        let first = world.spawn(EntityBuilder::new());
        let second = world.spawn(EntityBuilder::new());
        if held {
            world.move_entity(first, actor).unwrap();
            world.move_entity(second, actor).unwrap();
        }

        let relation = RelationId::new(Containment::TARGET_TAG).unwrap();
        let marker = ComponentId::new(Locus::TAG).unwrap();
        let item = ParameterId::new("item").unwrap();
        let food = ParameterId::new("food").unwrap();

        let take = AffordanceSchema::new(
            AffordanceId::new("take").unwrap(),
            "take",
            vec![
                Parameter::new(
                    item.clone(),
                    "item",
                    ValueSort::Entity,
                    ParameterMode::Input,
                    0,
                )
                .unwrap(),
            ],
            Vec::new(),
            vec![Effect::SetRelation {
                source: Term::Input(item),
                relation: relation.clone(),
                target: Term::Actor,
            }],
            Gate::Open,
            Resolution::Deterministic,
        );
        let eat = AffordanceSchema::new(
            AffordanceId::new("eat").unwrap(),
            "eat",
            vec![
                Parameter::new(
                    food.clone(),
                    "food",
                    ValueSort::Entity,
                    ParameterMode::Input,
                    0,
                )
                .unwrap(),
            ],
            vec![Guard::new(
                Formula::all(vec![Condition::RelationTarget {
                    source: Term::Input(food),
                    relation,
                    target: OptionalEntity::Is(Term::Actor),
                }]),
                "not held",
            )],
            vec![Effect::SetComponent {
                entity: Term::Actor,
                component: marker.clone(),
            }],
            Gate::Open,
            Resolution::Contested,
        );

        let mut state = StateRegistry::new();
        state.register_relation::<Containment>().unwrap();
        state.register_component::<Locus>().unwrap();
        let mut builder = AffordanceRegistryBuilder::new(state);
        builder
            .register(take, |_, _| {
                HandlerOutcome::committed(ActionOutcome::empty())
            })
            .unwrap();
        builder
            .register(
                eat,
                |_, _| HandlerOutcome::committed(ActionOutcome::empty()),
            )
            .unwrap();
        let registry = builder.build(&world).unwrap();
        let goal = Formula::all(vec![Condition::ComponentPresent {
            entity: Term::Actor,
            component: marker,
            present: true,
        }]);

        Fixture {
            world,
            registry,
            actor,
            first,
            second,
            goal,
        }
    }

    #[test]
    fn regression_emits_canonical_ground_actions() {
        let fixture = fixture(false);
        let costs = UnitCost;
        let plan = Planner::new(&costs)
            .plan(
                &fixture.registry,
                fixture.actor,
                &fixture.goal,
                &[fixture.first],
                &fixture.world,
            )
            .unwrap();

        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].affordance().as_str(), "take");
        assert_eq!(plan[1].affordance().as_str(), "eat");
        assert_eq!(plan[0].inputs(), &[Value::Entity(fixture.first)]);
        assert_eq!(plan[1].inputs(), &[Value::Entity(fixture.first)]);
    }

    #[test]
    fn excluding_one_grounding_preserves_another() {
        let fixture = fixture(true);
        let costs = UnitCost;
        let planner = Planner::new(&costs);
        let first_plan = planner
            .plan(
                &fixture.registry,
                fixture.actor,
                &fixture.goal,
                &[fixture.first, fixture.second],
                &fixture.world,
            )
            .unwrap();
        let replacement = planner
            .plan_excluding(
                &fixture.registry,
                fixture.actor,
                &fixture.goal,
                &[fixture.first, fixture.second],
                &fixture.world,
                &first_plan,
            )
            .unwrap();

        assert_eq!(replacement.len(), 1);
        assert_ne!(replacement[0].inputs(), first_plan[0].inputs());
    }
}
