use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use musce_core::hecs::EntityBuilder;
use musce_core::{EntityId, Locus, World};
use musce_proto::{ConnectionId, EventKind};

use crate::schema::{
    ActionOutcome, AffordanceId, AffordanceSchema, ComponentId, Condition, Effect, Formula,
    GroundAction, Guard, OptionalEntity, Parameter, ParameterId, ParameterMode, Resolution, Term,
    Value, ValueSort,
};
use crate::state::StateRegistry;
use crate::{Audience, Caller, CapRegistry, Ctx, Gate, SystemCtx, Verdict};

use super::*;

fn id(value: &str) -> AffordanceId {
    AffordanceId::new(value).unwrap()
}

fn parameter(value: &str, mode: ParameterMode, sort: ValueSort, slot: u16) -> Parameter {
    Parameter::new(ParameterId::new(value).unwrap(), value, sort, mode, slot).unwrap()
}

fn schema(
    name: &str,
    parameters: Vec<Parameter>,
    guards: Vec<Guard>,
    effects: Vec<Effect>,
    gate: Gate,
    resolution: Resolution,
) -> AffordanceSchema {
    AffordanceSchema::new(
        id(name),
        name,
        parameters,
        guards,
        effects,
        gate,
        resolution,
    )
}

fn actor(world: &mut World) -> EntityId {
    world.spawn(EntityBuilder::new())
}

fn empty_handler(_ctx: &mut PerformCtx<'_>, _action: &GroundAction) -> HandlerOutcome {
    HandlerOutcome::committed(ActionOutcome::empty())
}

#[test]
fn registration_rejects_malformed_and_unknown_schema_state() {
    let world = World::new();
    let mut builder = AffordanceRegistryBuilder::new(StateRegistry::new());
    let bad_slot = schema(
        "bad_slot",
        vec![parameter(
            "item",
            ParameterMode::Input,
            ValueSort::Entity,
            1,
        )],
        Vec::new(),
        Vec::new(),
        Gate::Open,
        Resolution::Deterministic,
    );
    assert!(matches!(
        builder.register(bad_slot, empty_handler),
        Err(RegistryError::InvalidSchema(_))
    ));

    let unknown_component = schema(
        "unknown_component",
        Vec::new(),
        vec![Guard::new(
            Formula::all(vec![Condition::ComponentPresent {
                entity: Term::Actor,
                component: ComponentId::new("typo").unwrap(),
                present: true,
            }]),
            "No.",
        )],
        Vec::new(),
        Gate::Open,
        Resolution::Deterministic,
    );
    assert!(matches!(
        builder.register(unknown_component, empty_handler),
        Err(RegistryError::InvalidSchema(_))
    ));
    assert!(builder.build(&world).is_ok());
}

#[test]
fn registration_rejects_guard_results_and_conflicting_effects() {
    let mut state = StateRegistry::new();
    let locus = state.register_component::<Locus>().unwrap();
    let result = ParameterId::new("made").unwrap();
    let result_in_guard = schema(
        "result_guard",
        vec![parameter(
            "made",
            ParameterMode::Result,
            ValueSort::Entity,
            0,
        )],
        vec![Guard::new(
            Formula::all(vec![Condition::Exists {
                entity: Term::Result(result),
                exists: false,
            }]),
            "No.",
        )],
        Vec::new(),
        Gate::Open,
        Resolution::Deterministic,
    );
    let mut builder = AffordanceRegistryBuilder::new(state);
    assert!(matches!(
        builder.register(result_in_guard, empty_handler),
        Err(RegistryError::InvalidSchema(_))
    ));

    let conflicting = schema(
        "conflicting",
        Vec::new(),
        Vec::new(),
        vec![
            Effect::SetComponent {
                entity: Term::Actor,
                component: locus.clone(),
            },
            Effect::RemoveComponent {
                entity: Term::Actor,
                component: locus,
            },
        ],
        Gate::Open,
        Resolution::Deterministic,
    );
    assert!(matches!(
        builder.register(conflicting, empty_handler),
        Err(RegistryError::InvalidSchema(_))
    ));
}

#[test]
fn registration_rejects_positive_liveness_guards_on_entity_inputs() {
    let item = ParameterId::new("item").unwrap();
    let schema = schema(
        "redundant_liveness",
        vec![parameter(
            "item",
            ParameterMode::Input,
            ValueSort::Entity,
            0,
        )],
        vec![Guard::new(
            Formula::all(vec![Condition::Exists {
                entity: Term::Input(item),
                exists: true,
            }]),
            "Gone.",
        )],
        Vec::new(),
        Gate::Open,
        Resolution::Deterministic,
    );
    assert!(matches!(
        AffordanceRegistryBuilder::new(StateRegistry::new()).register(schema, empty_handler),
        Err(RegistryError::InvalidSchema(_))
    ));
}

#[test]
fn grounding_and_gate_checks_run_before_guards_and_handler() {
    let mut world = World::new();
    let actor = actor(&mut world);
    let dead = EntityId(999);
    let mut caps = CapRegistry::new();
    let cap = caps.register("touch");
    let called = Arc::new(AtomicUsize::new(0));
    let called_by_handler = Arc::clone(&called);
    let schema = schema(
        "touch",
        vec![parameter(
            "item",
            ParameterMode::Input,
            ValueSort::Entity,
            0,
        )],
        Vec::new(),
        Vec::new(),
        Gate::Cap(cap),
        Resolution::Deterministic,
    );
    let mut builder = AffordanceRegistryBuilder::new(StateRegistry::new());
    builder
        .register(schema, move |_ctx, _action| {
            called_by_handler.fetch_add(1, Ordering::SeqCst);
            HandlerOutcome::committed(ActionOutcome::empty())
        })
        .unwrap();
    let registry = builder.build(&world).unwrap();
    let action = GroundAction::new(id("touch"), actor, vec![Value::Entity(dead)]);

    assert!(matches!(
        registry.perform(&mut world, &mut Vec::new(), &Verdict::guest(), &action),
        Err(PerformError::DeadInput { .. })
    ));
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[test]
fn gate_precedes_ordered_guard_refusal() {
    let mut world = World::new();
    let actor = actor(&mut world);
    let mut state = StateRegistry::new();
    let locus = state.register_component::<Locus>().unwrap();
    let mut caps = CapRegistry::new();
    let cap = caps.register("enter");
    let guards = vec![
        Guard::new(
            Formula::all(vec![Condition::ComponentPresent {
                entity: Term::Actor,
                component: locus.clone(),
                present: true,
            }]),
            "First failure.",
        ),
        Guard::new(
            Formula::all(vec![Condition::ComponentPresent {
                entity: Term::Actor,
                component: locus,
                present: true,
            }]),
            "Second failure.",
        ),
    ];
    let mut builder = AffordanceRegistryBuilder::new(state);
    builder
        .register(
            schema(
                "enter",
                Vec::new(),
                guards,
                Vec::new(),
                Gate::Cap(cap),
                Resolution::Deterministic,
            ),
            empty_handler,
        )
        .unwrap();
    let registry = builder.build(&world).unwrap();
    let action = GroundAction::new(id("enter"), actor, Vec::new());
    let mut out = Vec::new();

    assert_eq!(
        registry
            .perform(&mut world, &mut out, &Verdict::guest(), &action)
            .unwrap(),
        PerformOutcome::Refused(Refusal::Gate)
    );
    let granted = Verdict::new([cap].into_iter().collect(), false);
    assert_eq!(
        registry
            .perform(&mut world, &mut out, &granted, &action)
            .unwrap(),
        PerformOutcome::Refused(Refusal::Guard {
            index: 0,
            reason: "First failure.".into(),
        })
    );
}

#[test]
fn deterministic_refusal_is_contract_drift_but_contested_refusal_is_normal() {
    let mut world = World::new();
    let actor = actor(&mut world);
    let mut builder = AffordanceRegistryBuilder::new(StateRegistry::new());
    builder
        .register(
            schema(
                "certain",
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Gate::Open,
                Resolution::Deterministic,
            ),
            |_ctx, _action| HandlerOutcome::refused("late veto"),
        )
        .unwrap();
    builder
        .register(
            schema(
                "contest",
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Gate::Open,
                Resolution::Contested,
            ),
            |_ctx, _action| HandlerOutcome::refused("lost"),
        )
        .unwrap();
    let registry = builder.build(&world).unwrap();

    assert!(matches!(
        registry.perform(
            &mut world,
            &mut Vec::new(),
            &Verdict::guest(),
            &GroundAction::new(id("certain"), actor, Vec::new()),
        ),
        Err(PerformError::DeterministicRefusal { .. })
    ));
    assert!(matches!(
        registry.perform(
            &mut world,
            &mut Vec::new(),
            &Verdict::guest(),
            &GroundAction::new(id("contest"), actor, Vec::new()),
        ),
        Ok(PerformOutcome::Refused(Refusal::Resolution { .. }))
    ));
}

#[test]
fn committed_results_are_checked_and_self_output_is_entity_addressed() {
    let mut world = World::new();
    let actor = actor(&mut world);
    let mut builder = AffordanceRegistryBuilder::new(StateRegistry::new());
    builder
        .register(
            schema(
                "speak",
                vec![parameter("echo", ParameterMode::Result, ValueSort::Text, 0)],
                Vec::new(),
                Vec::new(),
                Gate::Open,
                Resolution::Deterministic,
            ),
            |ctx, _action| {
                ctx.emit_self(EventKind::Narration, "hello");
                HandlerOutcome::committed(ActionOutcome::new(vec![Value::text("hello")]))
            },
        )
        .unwrap();
    let registry = builder.build(&world).unwrap();
    let mut out = Vec::new();
    let outcome = registry
        .perform(
            &mut world,
            &mut out,
            &Verdict::guest(),
            &GroundAction::new(id("speak"), actor, Vec::new()),
        )
        .unwrap();

    assert!(matches!(out[0].event.to, Audience::Entity(entity) if entity == actor));
    assert!(matches!(outcome, PerformOutcome::Committed(_)));
}

#[test]
fn malformed_handler_results_are_contract_errors() {
    let mut world = World::new();
    let actor = actor(&mut world);
    let mut builder = AffordanceRegistryBuilder::new(StateRegistry::new());
    builder
        .register(
            schema(
                "bad_result",
                vec![parameter("text", ParameterMode::Result, ValueSort::Text, 0)],
                Vec::new(),
                Vec::new(),
                Gate::Open,
                Resolution::Deterministic,
            ),
            |ctx, _action| {
                ctx.emit_self(EventKind::Narration, "must not escape");
                HandlerOutcome::committed(ActionOutcome::new(vec![Value::Entity(EntityId(4))]))
            },
        )
        .unwrap();
    let registry = builder.build(&world).unwrap();

    let mut out = Vec::new();
    assert!(matches!(
        registry.perform(
            &mut world,
            &mut out,
            &Verdict::guest(),
            &GroundAction::new(id("bad_result"), actor, Vec::new()),
        ),
        Err(PerformError::WrongResultSort { .. })
    ));
    assert!(out.is_empty());
}

#[test]
fn duplicate_affordance_ids_are_rejected() {
    let mut builder = AffordanceRegistryBuilder::new(StateRegistry::new());
    for first in [true, false] {
        let result = builder.register(
            schema(
                "same",
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Gate::Open,
                Resolution::Deterministic,
            ),
            empty_handler,
        );
        assert_eq!(result.is_ok(), first);
        if !first {
            assert!(matches!(result, Err(RegistryError::DuplicateAffordance(_))));
        }
    }
}

#[test]
fn player_context_cannot_substitute_another_actor() {
    let mut world = World::new();
    let caller_actor = actor(&mut world);
    let other_actor = actor(&mut world);
    let mut builder = AffordanceRegistryBuilder::new(StateRegistry::new());
    builder
        .register(
            schema(
                "wait",
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Gate::Open,
                Resolution::Deterministic,
            ),
            empty_handler,
        )
        .unwrap();
    let registry = builder.build(&world).unwrap();
    let verdict = Verdict::guest();
    let caller = Caller::new(caller_actor, ConnectionId(1), &verdict);
    let mut out = Vec::new();
    let mut ctx = Ctx::new(&mut world, caller, &mut out);

    assert!(matches!(
        ctx.perform(
            &registry,
            &GroundAction::new(id("wait"), other_actor, Vec::new())
        ),
        Err(PerformError::ActorMismatch { .. })
    ));
}

#[test]
fn system_context_requires_an_explicit_verdict_for_autonomous_actions() {
    let mut world = World::new();
    let actor = actor(&mut world);
    let mut caps = CapRegistry::new();
    let cap = caps.register("autonomous");
    let mut builder = AffordanceRegistryBuilder::new(StateRegistry::new());
    builder
        .register(
            schema(
                "autonomous",
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Gate::Cap(cap),
                Resolution::Deterministic,
            ),
            move |ctx, _action| {
                assert!(ctx.permits(cap));
                HandlerOutcome::committed(ActionOutcome::empty())
            },
        )
        .unwrap();
    let registry = builder.build(&world).unwrap();
    let action = GroundAction::new(id("autonomous"), actor, Vec::new());
    let facts = Vec::new();
    let mut out = Vec::new();
    let granted = Verdict::new([cap].into_iter().collect(), false);
    let mut ctx = SystemCtx::new(&mut world, 1, SystemTime::now(), &facts, &mut out);

    assert_eq!(
        ctx.perform(&registry, &Verdict::guest(), &action).unwrap(),
        PerformOutcome::Refused(Refusal::Gate)
    );
    assert!(matches!(
        ctx.perform(&registry, &granted, &action),
        Ok(PerformOutcome::Committed(_))
    ));
}

#[test]
fn effect_index_excludes_opaque_and_fresh_result_effects() {
    let mut world = World::new();
    let actor = actor(&mut world);
    let mut builder = AffordanceRegistryBuilder::new(StateRegistry::new());
    builder
        .register(
            schema(
                "move",
                vec![parameter(
                    "destination",
                    ParameterMode::Input,
                    ValueSort::Entity,
                    0,
                )],
                Vec::new(),
                vec![Effect::SetLocus {
                    entity: Term::Actor,
                    locus: Term::Input(ParameterId::new("destination").unwrap()),
                }],
                Gate::Open,
                Resolution::Deterministic,
            ),
            empty_handler,
        )
        .unwrap();
    builder
        .register(
            schema(
                "teleport_secretly",
                vec![parameter(
                    "destination",
                    ParameterMode::Input,
                    ValueSort::Entity,
                    0,
                )],
                Vec::new(),
                vec![Effect::SetLocus {
                    entity: Term::Actor,
                    locus: Term::Input(ParameterId::new("destination").unwrap()),
                }],
                Gate::Open,
                Resolution::Opaque,
            ),
            empty_handler,
        )
        .unwrap();
    builder
        .register(
            schema(
                "make",
                vec![parameter(
                    "made",
                    ParameterMode::Result,
                    ValueSort::Entity,
                    0,
                )],
                Vec::new(),
                vec![Effect::Create {
                    result: ParameterId::new("made").unwrap(),
                }],
                Gate::Open,
                Resolution::Deterministic,
            ),
            empty_handler,
        )
        .unwrap();
    let registry = builder.build(&world).unwrap();

    assert_eq!(
        registry.affordances_affecting(&EffectKind::Locus),
        [id("move")]
    );
    assert!(
        registry
            .affordances_affecting(&EffectKind::Existence)
            .is_empty()
    );
    assert_eq!(registry.schema(&id("move")).unwrap().id(), &id("move"));
    assert_eq!(
        registry
            .schemas()
            .map(|schema| schema.id().as_str())
            .collect::<Vec<_>>(),
        ["make", "move", "teleport_secretly"]
    );
    assert!(world.contains(actor));
}

#[test]
fn optional_entity_terms_are_sort_checked_at_registration() {
    let bad = schema(
        "bad_optional",
        vec![parameter("text", ParameterMode::Input, ValueSort::Text, 0)],
        vec![Guard::new(
            Formula::all(vec![Condition::LocusOf {
                entity: Term::Actor,
                locus: OptionalEntity::Is(Term::Input(ParameterId::new("text").unwrap())),
            }]),
            "No.",
        )],
        Vec::new(),
        Gate::Open,
        Resolution::Deterministic,
    );
    assert!(matches!(
        AffordanceRegistryBuilder::new(StateRegistry::new()).register(bad, empty_handler),
        Err(RegistryError::InvalidSchema(_))
    ));
}
