use super::*;
use musce_core::hecs::EntityBuilder;
use musce_core::{Cascade, Containment, Description, Locus};

use crate::Gate;
use crate::schema::{AffordanceId, Guard, Parameter, Resolution};

fn schema(parameters: Vec<Parameter>) -> AffordanceSchema {
    AffordanceSchema::new(
        AffordanceId::new("test").unwrap(),
        "Test",
        parameters,
        Vec::<Guard>::new(),
        Vec::new(),
        Gate::Open,
        Resolution::Deterministic,
    )
}

fn input(name: &str, slot: u16) -> Parameter {
    Parameter::new(
        ParameterId::new(name).unwrap(),
        name,
        ValueSort::Entity,
        ParameterMode::Input,
        slot,
    )
    .unwrap()
}

fn entity(world: &mut World) -> EntityId {
    world.spawn(EntityBuilder::new())
}

#[test]
fn evaluates_relations_components_locus_and_existence() {
    let mut world = World::new();
    let room = {
        let mut builder = EntityBuilder::new();
        builder.add(Locus);
        world.spawn(builder)
    };
    let bag = entity(&mut world);
    let coin = entity(&mut world);
    world.move_entity(bag, room).unwrap();
    world.move_entity(coin, bag).unwrap();

    let mut state = StateRegistry::new();
    let containment = state.register_relation::<Containment>().unwrap();
    let locus = state.register_component::<Locus>().unwrap();
    state.validate_world(&world).unwrap();

    let item = ParameterId::new("item").unwrap();
    let schema = schema(vec![input("item", 0)]);
    let action = GroundAction::new(
        AffordanceId::new("test").unwrap(),
        bag,
        vec![Value::Entity(coin)],
    );
    let witness = LocalId::new("room").unwrap();
    let formula = Formula::new(
        vec![crate::schema::Local::new(
            witness.clone(),
            ValueSort::Entity,
        )],
        vec![
            Condition::RelationTarget {
                source: Term::Input(item.clone()),
                relation: containment,
                target: OptionalEntity::Is(Term::Actor),
            },
            Condition::LocusOf {
                entity: Term::Input(item.clone()),
                locus: OptionalEntity::Is(Term::Local(witness.clone())),
            },
            Condition::ComponentPresent {
                entity: Term::Local(witness),
                component: locus,
                present: true,
            },
            Condition::Exists {
                entity: Term::Input(item),
                exists: true,
            },
        ],
    );

    assert!(
        state
            .evaluate(&formula, &schema, &action, None, &world)
            .unwrap()
    );
}

#[test]
fn gauge_thresholds_use_registered_region_order() {
    let mut world = World::new();
    let actor = {
        let mut builder = EntityBuilder::new();
        builder.add(Description("1234567890".into()));
        world.spawn(builder)
    };
    let gauge = GaugeId::new("description_length");
    let short = GaugeRegionId::new("short").unwrap();
    let long = GaugeRegionId::new("long").unwrap();
    let mut state = StateRegistry::new();
    state
        .register_gauge(
            gauge.clone(),
            vec![
                GaugeRegion::new(short.clone(), GaugeTarget::at_most(GaugeLevel::new(9))),
                GaugeRegion::new(long.clone(), GaugeTarget::at_least(GaugeLevel::new(10))),
            ],
            |world, entity| {
                world
                    .get::<Description>(entity)
                    .map(|description| GaugeLevel::new(description.0.len().min(255) as u8))
            },
        )
        .unwrap();
    let schema = schema(Vec::new());
    let action = GroundAction::new(AffordanceId::new("test").unwrap(), actor, Vec::new());

    assert!(
        state
            .evaluate(
                &Formula::all(vec![Condition::GaugeAtLeast {
                    entity: Term::Actor,
                    gauge: gauge.clone(),
                    region: long.clone(),
                }]),
                &schema,
                &action,
                None,
                &world,
            )
            .unwrap()
    );
    assert!(
        !state
            .evaluate(
                &Formula::all(vec![Condition::GaugeAtMost {
                    entity: Term::Actor,
                    gauge,
                    region: short,
                }]),
                &schema,
                &action,
                None,
                &world,
            )
            .unwrap()
    );
}

#[test]
fn gauge_regions_must_form_a_total_nonoverlapping_cover() {
    let mut state = StateRegistry::new();
    let result = state.register_gauge(
        GaugeId::new("broken"),
        vec![GaugeRegion::new(
            GaugeRegionId::new("middle").unwrap(),
            GaugeTarget::between(GaugeLevel::new(10), GaugeLevel::new(20)).unwrap(),
        )],
        |_world, _entity| Some(GaugeLevel::new(10)),
    );
    assert!(matches!(
        result,
        Err(StateRegistrationError::InvalidGauge { .. })
    ));

    let result = state.register_gauge(
        GaugeId::new("overlap_at_max"),
        vec![
            GaugeRegion::new(
                GaugeRegionId::new("all").unwrap(),
                GaugeTarget::between(GaugeLevel::MIN, GaugeLevel::MAX).unwrap(),
            ),
            GaugeRegion::new(
                GaugeRegionId::new("extra").unwrap(),
                GaugeTarget::at(GaugeLevel::MAX),
            ),
        ],
        |_world, _entity| Some(GaugeLevel::MAX),
    );
    assert!(matches!(
        result,
        Err(StateRegistrationError::InvalidGauge { .. })
    ));
}

struct Unregistered;

impl Relation for Unregistered {
    const ACYCLIC: bool = false;
    const ON_TARGET_DESPAWN: Cascade = Cascade::Detach;
    const TARGET_TAG: &'static str = "unregistered";
}

#[test]
fn activation_fails_when_the_world_omits_a_typed_reader() {
    let world = World::new();
    let mut state = StateRegistry::new();
    state.register_relation::<Unregistered>().unwrap();
    let error = state.validate_world(&world).unwrap_err();
    assert!(error.missing()[0].contains("unregistered"));
}

#[test]
fn unknown_vocabulary_is_an_error_not_false() {
    let world = World::new();
    let actor = EntityId(1);
    let schema = schema(Vec::new());
    let action = GroundAction::new(AffordanceId::new("test").unwrap(), actor, Vec::new());
    let formula = Formula::all(vec![Condition::ComponentPresent {
        entity: Term::Actor,
        component: ComponentId::new("typo").unwrap(),
        present: true,
    }]);
    assert!(matches!(
        StateRegistry::new().evaluate(&formula, &schema, &action, None, &world),
        Err(EvaluationError::UnknownComponent(_))
    ));
}
