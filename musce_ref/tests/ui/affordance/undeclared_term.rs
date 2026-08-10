use musce::affordance;

affordance! {
    sample(item: Entity) {
        requires {
            missing.has_component(Item) => "missing";
        }
        effects {}
        gate Open;
        resolution Deterministic;
        execute execute_sample;
    }
}

fn main() {}
