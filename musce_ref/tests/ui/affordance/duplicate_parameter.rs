use musce::affordance;

affordance! {
    sample(item: Entity) -> (item: Entity) {
        requires {}
        effects {}
        gate Open;
        resolution Deterministic;
        execute execute_sample;
    }
}

fn main() {}
