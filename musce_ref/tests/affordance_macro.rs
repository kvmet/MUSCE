use musce::action::schema::{Resolution, Value, ValueSort};
use musce::action::state::StateRegistry;
use musce::action::{AffordanceDefinition, CapRegistry, Gate, PerformCtx, TypedHandlerOutcome};
use musce::affordance;
use musce::world::{Containment, EntityId, Locus};

affordance! {
    speak(text: Text) -> (echo: Text) {
        requires {}
        effects {}
        gate Cap(speak_cap);
        resolution Deterministic;
        execute execute_speak;
    }
}

fn execute_speak(
    _ctx: &mut PerformCtx<'_>,
    inputs: &SpeakInputs,
) -> TypedHandlerOutcome<SpeakResults> {
    TypedHandlerOutcome::committed(SpeakResults {
        echo: inputs.text.clone(),
    })
}

affordance! {
    ping() {
        requires {}
        effects {}
        gate Open;
        resolution Opaque;
        execute execute_ping;
    }
}

fn execute_ping(
    _ctx: &mut PerformCtx<'_>,
    _inputs: &PingInputs,
) -> TypedHandlerOutcome<PingResults> {
    TypedHandlerOutcome::committed(PingResults {})
}

affordance! {
    closed_vocabulary(
        item: Entity,
        target: Entity,
        missing: Entity,
    ) -> (created: Entity) {
        requires {
            item.relation_is(Containment, Actor) => "relation";
            item.relation_is_not(Containment, target) => "relation inequality";
            item.has_no_relation(Containment) => "unset relation";
            item.has_component(Locus) => "component";
            item.has_no_component(Locus) => "absent component";
            item.at_locus(target) => "locus";
            item.not_at_locus(target) => "other locus";
            item.has_no_locus() => "unset locus";
            item.gauge_at_least("health", "hale") => "low gauge";
            item.gauge_at_most("health", "hale") => "high gauge";
            missing.does_not_exist() => "present";
            distinct(item, target) => "same";
            same_locus(item, target) => "far";
            all {
                item.has_component(Locus);
                not(target.has_component(Locus));
            } => "all";
            exists(locus: Entity) {
                item.at_locus(locus);
                target.at_locus(locus);
            } => "far";
        }
        effects {
            item.set_relation(Containment, target);
            item.clear_relation(Containment);
            item.set_component(Locus);
            item.remove_component(Locus);
            item.set_locus(target);
            item.clear_locus();
            item.shift_gauge("health", Up);
            created.create();
            item.destroy();
        }
        gate Open;
        resolution Opaque;
        execute execute_closed_vocabulary;
    }
}

fn execute_closed_vocabulary(
    _ctx: &mut PerformCtx<'_>,
    _inputs: &ClosedVocabularyInputs,
) -> TypedHandlerOutcome<ClosedVocabularyResults> {
    TypedHandlerOutcome::refused("compile-only declaration")
}

#[test]
fn generated_types_preserve_slots_sorts_and_runtime_gate() {
    let mut caps = CapRegistry::new();
    let speak_cap = caps.register("speak");
    let definition = Speak::register(&mut StateRegistry::new(), speak_cap).unwrap();
    let schema = definition.schema();

    assert_eq!(schema.gate(), Gate::Cap(speak_cap));
    assert_eq!(schema.resolution(), Resolution::Deterministic);
    assert_eq!(schema.inputs().next().unwrap().sort(), &ValueSort::Text);
    assert_eq!(schema.results().next().unwrap().sort(), &ValueSort::Text);

    let action = speak_action(EntityId(4), "hello".to_owned());
    let inputs = definition.decode_inputs(&action).unwrap();
    assert_eq!(inputs.text, "hello");
}

#[test]
fn empty_signatures_generate_ground_actions_and_unit_observations() {
    let definition = Ping::register(&mut StateRegistry::new()).unwrap();
    assert_eq!(definition.schema().resolution(), Resolution::Opaque);

    let action = ping_action(EntityId(9));
    assert!(action.inputs().is_empty());
    assert_eq!(definition.decode_inputs(&action).unwrap(), PingInputs {});
    assert_eq!(Value::text("unused").sort(), ValueSort::Text);
}

#[test]
fn every_closed_condition_and_effect_form_emits_canonical_rust() {
    let definition = ClosedVocabulary::register(&mut StateRegistry::new()).unwrap();
    let schema = definition.schema();
    assert_eq!(schema.guards().len(), 15);
    assert_eq!(schema.effects().len(), 9);
}
