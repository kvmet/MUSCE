use musce_core::hecs::EntityBuilder;
use musce_core::{Containment, EntityId, Locus, World};

use crate::schema::{
    AffordanceId, AffordanceSchema, ComponentId, Condition, Formula, Guard, OptionalEntity,
    Parameter, ParameterBinding, ParameterId, ParameterMode, PartialGrounding, RelationId,
    Resolution, Term, Value, ValueSort,
};
use crate::state::StateRegistry;
use crate::{
    AffordanceRegistryBuilder, CapRegistry, Gate, HandlerOutcome, InputCandidates, OfferProposal,
    OfferStatus, Verdict,
};

fn id(value: &str) -> ParameterId {
    ParameterId::new(value).unwrap()
}

fn fixture(
    gate: Gate,
) -> (
    World,
    crate::AffordanceRegistry,
    EntityId,
    EntityId,
    EntityId,
) {
    let mut world = World::new();
    let actor = world.spawn(EntityBuilder::new());
    let item = world.spawn(EntityBuilder::new());
    let recipient = {
        let mut b = EntityBuilder::new();
        b.add(Locus);
        world.spawn(b)
    };
    world.move_entity(item, actor).unwrap();

    let mut state = StateRegistry::new();
    let containment: RelationId = state.register_relation::<Containment>().unwrap();
    let recipient_component: ComponentId = state.register_component::<Locus>().unwrap();
    let item_id = id("item");
    let recipient_id = id("recipient");
    let schema = AffordanceSchema::new(
        AffordanceId::new("give").unwrap(),
        "Give",
        vec![
            // Declaration order is intentionally not dense-slot order. Generic
            // grounding must key by slot, never positional declaration order.
            Parameter::new(
                recipient_id.clone(),
                "recipient",
                ValueSort::Entity,
                ParameterMode::Input,
                1,
            )
            .unwrap(),
            Parameter::new(
                item_id.clone(),
                "item",
                ValueSort::Entity,
                ParameterMode::Input,
                0,
            )
            .unwrap(),
        ],
        vec![
            Guard::new(
                Formula::all(vec![Condition::RelationTarget {
                    source: Term::Input(item_id),
                    relation: containment,
                    target: OptionalEntity::Is(Term::Actor),
                }]),
                "not held",
            ),
            Guard::new(
                Formula::all(vec![Condition::ComponentPresent {
                    entity: Term::Input(recipient_id),
                    component: recipient_component,
                    present: true,
                }]),
                "not a recipient",
            ),
        ],
        Vec::new(),
        gate,
        Resolution::Deterministic,
    );
    let mut builder = AffordanceRegistryBuilder::new(state);
    builder
        .register(schema, |_, _| {
            HandlerOutcome::committed(crate::schema::ActionOutcome::empty())
        })
        .unwrap();
    let registry = builder.build(&world).unwrap();
    (world, registry, actor, item, recipient)
}

fn partial(bindings: Vec<ParameterBinding>) -> crate::schema::PartialGrounding {
    PartialGrounding::new(AffordanceId::new("give").unwrap(), bindings)
}

#[test]
fn a_ground_guard_can_veto_while_an_earlier_guard_is_unbound() {
    let (mut world, registry, actor, _item, recipient) = fixture(Gate::Open);
    let wrong = world.spawn(EntityBuilder::new());

    let vetoed = registry
        .classify_offer(
            &world,
            &Verdict::guest(),
            actor,
            OfferProposal::new(
                partial(vec![ParameterBinding::new(
                    id("recipient"),
                    Value::Entity(wrong),
                )]),
                Vec::new(),
            ),
        )
        .unwrap();
    assert!(matches!(
        vetoed.status(),
        OfferStatus::Vetoed { reason } if &**reason == "not a recipient"
    ));

    let needs = registry
        .classify_offer(
            &world,
            &Verdict::guest(),
            actor,
            OfferProposal::new(
                partial(vec![ParameterBinding::new(
                    id("recipient"),
                    Value::Entity(recipient),
                )]),
                Vec::new(),
            ),
        )
        .unwrap();
    assert!(matches!(
        needs.status(),
        OfferStatus::Needs { parameters } if parameters.as_ref() == [id("item")]
    ));
}

#[test]
fn complete_grounding_is_dense_and_available() {
    let (world, registry, actor, item, recipient) = fixture(Gate::Open);
    let grounding = partial(vec![
        ParameterBinding::new(id("recipient"), Value::Entity(recipient)),
        ParameterBinding::new(id("item"), Value::Entity(item)),
    ]);
    let offer = registry
        .classify_offer(
            &world,
            &Verdict::guest(),
            actor,
            OfferProposal::new(grounding.clone(), Vec::new()),
        )
        .unwrap();
    assert_eq!(offer.status(), &OfferStatus::Available);

    let action = registry.ground_action(&world, actor, &grounding).unwrap();
    assert_eq!(
        action.inputs(),
        &[Value::Entity(item), Value::Entity(recipient)]
    );
}

#[test]
fn candidate_sets_are_typed_live_and_only_for_missing_inputs() {
    let (mut world, registry, actor, item, recipient) = fixture(Gate::Open);
    let dead = world.spawn(EntityBuilder::new());
    world.despawn(dead);
    let grounding = partial(vec![ParameterBinding::new(
        id("recipient"),
        Value::Entity(recipient),
    )]);

    let valid = OfferProposal::new(
        grounding.clone(),
        vec![InputCandidates::new(id("item"), vec![Value::Entity(item)])],
    );
    assert!(
        registry
            .classify_offer(&world, &Verdict::guest(), actor, valid)
            .is_ok()
    );

    let bound_candidates = OfferProposal::new(
        grounding.clone(),
        vec![InputCandidates::new(
            id("recipient"),
            vec![Value::Entity(recipient)],
        )],
    );
    assert!(matches!(
        registry.classify_offer(&world, &Verdict::guest(), actor, bound_candidates),
        Err(super::GroundingError::CandidatesForBoundInput(_))
    ));

    let dead_candidates = OfferProposal::new(
        grounding,
        vec![InputCandidates::new(id("item"), vec![Value::Entity(dead)])],
    );
    assert!(matches!(
        registry.classify_offer(&world, &Verdict::guest(), actor, dead_candidates),
        Err(super::GroundingError::DeadEntity { .. })
    ));
}

#[test]
fn account_gate_precedes_missing_inputs() {
    let mut caps = CapRegistry::new();
    let cap = caps.register("give");
    let (world, registry, actor, _item, _recipient) = fixture(Gate::Cap(cap));
    let offer = registry
        .classify_offer(
            &world,
            &Verdict::guest(),
            actor,
            OfferProposal::new(partial(Vec::new()), Vec::new()),
        )
        .unwrap();
    assert!(matches!(offer.status(), OfferStatus::Vetoed { .. }));
}
