use musce::affordance;

affordance! {
    sample() -> (product: Entity) {
        requires {
            product.has_component(Item) => "missing";
        }
        effects {
            product.create();
        }
        gate Open;
        resolution Deterministic;
        execute execute_sample;
    }
}

fn main() {}
