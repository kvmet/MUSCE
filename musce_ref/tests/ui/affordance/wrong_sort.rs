use musce::affordance;

affordance! {
    sample(text: Text) {
        requires {
            text.has_component(Item) => "missing";
        }
        effects {}
        gate Open;
        resolution Deterministic;
        execute execute_sample;
    }
}

fn main() {}
