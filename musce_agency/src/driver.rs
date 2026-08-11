//! Scheduler-independent canonical pursuit state.

use musce_action::AffordanceRegistry;
use musce_action::schema::{Formula, GroundAction};
use musce_core::{EntityId, World};

use crate::Planner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beat {
    Committed,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Achieved,
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Action(GroundAction),
    Complete(Progress),
}

const MAX_REPLANS: usize = 64;

/// Persistent logical state for one pursuit. It owns no world or registry borrow,
/// so a scheduler may ask for a beat, execute it through its context, then report
/// the outcome on the same or a later tick.
pub struct Driver {
    actor: EntityId,
    goal: Formula,
    remaining: Vec<GroundAction>,
    excluded: Vec<GroundAction>,
    pending: Option<GroundAction>,
    replans: usize,
}

impl Driver {
    pub fn new(actor: EntityId, goal: Formula) -> Self {
        Self {
            actor,
            goal,
            remaining: Vec::new(),
            excluded: Vec::new(),
            pending: None,
            replans: 0,
        }
    }

    pub fn next(
        &mut self,
        planner: &Planner<'_>,
        registry: &AffordanceRegistry,
        known: &[EntityId],
        world: &World,
    ) -> Next {
        assert!(
            self.pending.is_none(),
            "record the pending beat before requesting another"
        );
        if let Some(action) = self.remaining.first().cloned() {
            self.remaining.remove(0);
            self.pending = Some(action.clone());
            return Next::Action(action);
        }
        if planner.goal_holds(registry, self.actor, &self.goal, known, world) {
            return Next::Complete(Progress::Achieved);
        }
        if self.replans >= MAX_REPLANS {
            return Next::Complete(Progress::Abandoned);
        }
        self.replans += 1;
        let Some(plan) = planner.plan_excluding(
            registry,
            self.actor,
            &self.goal,
            known,
            world,
            &self.excluded,
        ) else {
            return Next::Complete(Progress::Abandoned);
        };
        if plan.is_empty() {
            tracing::error!("planner returned an empty plan for a false live goal");
            return Next::Complete(Progress::Abandoned);
        }
        self.remaining = plan;
        self.next(planner, registry, known, world)
    }

    pub fn record(&mut self, beat: Beat) {
        let action = self.pending.take().expect("no pursuit beat is pending");
        if beat == Beat::Refused {
            self.excluded.push(action);
            self.remaining.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_action::schema::{AffordanceId, Value};

    fn action(name: &str, input: EntityId) -> GroundAction {
        GroundAction::new(
            AffordanceId::new(name).unwrap(),
            EntityId(1),
            vec![Value::Entity(input)],
        )
    }

    #[test]
    fn refusal_excludes_the_exact_pending_grounding_and_discards_the_stale_tail() {
        let refused = action("try", EntityId(2));
        let stale = action("finish", EntityId(3));
        let mut pursuit = Driver::new(EntityId(1), Formula::default());
        pursuit.pending = Some(refused.clone());
        pursuit.remaining.push(stale);

        pursuit.record(Beat::Refused);

        assert_eq!(pursuit.excluded, vec![refused]);
        assert!(pursuit.remaining.is_empty());
        assert!(pursuit.pending.is_none());
    }

    #[test]
    fn commitment_preserves_the_remaining_plan() {
        let pending = action("start", EntityId(2));
        let next = action("finish", EntityId(3));
        let mut pursuit = Driver::new(EntityId(1), Formula::default());
        pursuit.pending = Some(pending);
        pursuit.remaining.push(next.clone());

        pursuit.record(Beat::Committed);

        assert!(pursuit.excluded.is_empty());
        assert_eq!(pursuit.remaining, vec![next]);
    }
}
