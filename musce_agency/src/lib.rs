//! Optional canonical affordance planning, arbitration, and pursuit.

use musce_action::schema::{AffordanceSchema, GroundAction};
use musce_core::{EntityId, World};

mod arbiter;
mod driver;
mod planner;

pub use arbiter::{Arbiter, Goal, Urgency};
pub use driver::{Beat, Driver, Next, Progress};
pub use planner::{Plan, Planner, Step};

pub type Cost = u32;

pub trait CostModel {
    fn cost(
        &self,
        actor: EntityId,
        affordance: &AffordanceSchema,
        grounding: &GroundAction,
        world: &World,
    ) -> Cost;
}

pub struct UnitCost;

impl CostModel for UnitCost {
    fn cost(
        &self,
        _actor: EntityId,
        _affordance: &AffordanceSchema,
        _grounding: &GroundAction,
        _world: &World,
    ) -> Cost {
        1
    }
}
