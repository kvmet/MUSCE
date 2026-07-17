//! `musce_agency`: the optional planner side of the agency subsystem (the
//! `CostModel` seam and the `bind_var` binding primitive now; the planner and
//! arbiter later) that a game consumes. The affordance vocabulary it plans over
//! (`Term` / `Predicate` / `Clause` / `Affordance` / `Frame` / `WorldModel`)
//! lives in the engine, non-optional, in `musce_action`, because a verb-gate is a
//! dispatch concern independent of planning; this crate re-exports it for
//! planner-facing consumers. Game content, the concrete affordances and the
//! relation/component vocabulary their predicates name, lives in the consumer
//! crate, never here. See `docs/architecture/agency/` and
//! `docs/architecture/affordances.md`.

use musce_core::{EntityId, World};

pub use musce_action::{
    Affordance, Clause, Frame, Guard, Literal, Predicate, Term, Var, WorldModel,
};

/// A plan cost the planner minimizes. Integer for MVP; widening to a fractional
/// type for scaled costs (distance, effort) is a localized change to this alias
/// and the `CostModel` impls.
pub type Cost = u32;

/// The game-supplied cost policy. The planner obtains an affordance's cost by
/// calling this, never by reading a field, so a flat cost, a distance-scaled
/// cost, and a per-actor *learned* cost (build step 6) are all the same seam.
/// The generic crate ships only the trivial [`UnitCost`]; a real game supplies
/// its own model in the consumer crate.
pub trait CostModel {
    fn cost(&self, actor: EntityId, affordance: &Affordance, world: &World) -> Cost;
}

/// Every affordance costs one. The trivial baseline a game replaces, and the
/// static reference the learned model of build step 6 is measured against.
pub struct UnitCost;

impl CostModel for UnitCost {
    fn cost(&self, _actor: EntityId, _affordance: &Affordance, _world: &World) -> Cost {
        1
    }
}

/// Enumerate the candidates that, substituted for the free variable `var`,
/// satisfy every literal of `constraint` under `model`. The shared binding
/// primitive: a plan step uses it to fill a fungible slot from the actor's known
/// entities, and the planner reuses it when regressing an existential clause
/// (`∃x. tag(x, Food)`), branching one grounded plan per match.
///
/// `constraint` is expected already frame-bound, so `var` is its one remaining
/// free variable. A candidate that leaves any *other* variable free is rejected:
/// an unbound term cannot hold, so a partially-ground clause is never counted as
/// satisfied. An empty result is a meaningful answer ("nothing known fits"), the
/// signal that later regresses to a find/search step, not an error.
pub fn bind_var(
    var: &Var,
    constraint: &Clause,
    candidates: &[EntityId],
    world: &World,
    model: &dyn WorldModel,
) -> Vec<EntityId> {
    candidates
        .iter()
        .copied()
        .filter(|&id| {
            constraint
                .substitute(var, id)
                .0
                .iter()
                .all(|l| l.holds(world, model))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_cost_is_one() {
        let take = Affordance {
            name: "take".into(),
            guards: Vec::new(),
            effect: Clause::default(),
        };
        assert_eq!(UnitCost.cost(EntityId(1), &take, &World::new()), 1);
    }
}
