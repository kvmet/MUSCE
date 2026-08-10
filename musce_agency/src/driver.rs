//! The execution driver: run a committed goal's plan beat by beat, replanning
//! around a beat that vetoes. This is the "why doesn't it retry the same failed
//! action forever" half of the answer (the planner's internal bounds are the
//! other): a vetoed step is added to an exclusion set the next plan must route
//! around, so the driver either finds another way or abandons the goal.
//!
//! [`pursue`](Driver::pursue) runs a whole plan to completion in one call, off any
//! tick loop. Wiring it into the sim thread as a per-agent, one-beat-per-tick
//! system is a later, separate concern (it touches scheduling, not this logic).
//! See `docs/architecture/agency/execution.md`.

use musce_core::{EntityId, World};

use musce_action::Clause;

use crate::{Planner, Step};

/// One beat's outcome: the grounded action committed, or a rule refused it. The
/// generic result the replan loop reads, so the loop never names an app's richer
/// outcome type; the app maps its own (`musce_ref::Outcome`) onto this in the
/// closure it hands [`pursue`](Driver::pursue). The reason for a refusal is not
/// carried: the loop excludes the step regardless of why it vetoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beat {
    Committed,
    Refused,
}

/// The result of pursuing a goal to a stopping point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// A whole plan's beats committed and the goal holds in the resulting world.
    /// An already-true goal (the empty plan) counts, so this doubles as the
    /// arbiter's "satisfied, release it" signal.
    Achieved,
    /// Every beat committed, but the goal is false afterward. This exposes
    /// interference or a broken affordance effect contract instead of reporting a
    /// false success.
    Unmet,
    /// No plan survived the exclusion set: the goal is unreachable given what the
    /// agent knows and the steps that have vetoed. The caller releases the goal.
    Abandoned,
}

/// The most times [`pursue`](Driver::pursue) will replan before giving up. The
/// exclusion set grows by at least one distinct step each replan and the step space
/// over a finite `known` set is finite, so pursuit terminates by exhaustion on its
/// own; this only backstops a pathological table, mirroring the planner's own
/// bounds.
const MAX_REPLANS: usize = 64;

/// Runs a committed goal's plan, replanning around a vetoed beat. Wraps a
/// [`Planner`]; the arbiter's chosen goal flows into [`pursue`](Driver::pursue).
pub struct Driver<'a> {
    planner: &'a Planner<'a>,
}

impl<'a> Driver<'a> {
    pub fn new(planner: &'a Planner<'a>) -> Self {
        Driver { planner }
    }

    /// Plan for `goal`, run each step through `run` (the app's lowering of a step
    /// to its grounded action), and on a vetoed beat exclude that step and replan
    /// from the now-current world. Returns [`Progress::Achieved`] only when the goal
    /// holds after a whole plan commits, [`Progress::Unmet`] when committed beats
    /// leave it false, and [`Progress::Abandoned`] when replanning runs dry.
    ///
    /// `run` receives the live `&mut World` and the step to perform, and returns
    /// whether it committed. Replanning reads the world *after* the beats that
    /// already committed, so a plan whose early steps landed is not redone; the
    /// planner sees their effects as already true.
    pub fn pursue(
        &self,
        actor: EntityId,
        goal: &Clause,
        known: &[EntityId],
        world: &mut World,
        mut run: impl FnMut(&mut World, &Step) -> Beat,
    ) -> Progress {
        let mut excluded: Vec<Step> = Vec::new();
        for _ in 0..MAX_REPLANS {
            let Some(plan) = self
                .planner
                .plan_excluding(actor, goal, known, world, &excluded)
            else {
                return Progress::Abandoned;
            };
            let mut vetoed = None;
            for step in &plan {
                match run(world, step) {
                    Beat::Committed => {}
                    Beat::Refused => {
                        vetoed = Some(step.clone());
                        break;
                    }
                }
            }
            match vetoed {
                None if self.planner.goal_holds(actor, goal, known, world) => {
                    return Progress::Achieved;
                }
                None => {
                    tracing::warn!("every planned beat committed but the goal remains false");
                    return Progress::Unmet;
                }
                Some(step) => excluded.push(step),
            }
        }
        Progress::Abandoned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Guard, UnitCost};
    use musce_action::{Affordance, Predicate, Term, WorldModel};
    use std::cell::RefCell;
    use std::collections::HashSet;

    // The same ground-fact stub the planner tests use: `holds` answers from fixed
    // relations and tags, ignoring the empty `World`. The driver's `run` closure
    // decides a beat's outcome from the step alone, so these facts drive planning
    // while the closure drives execution; the two are independent here by design,
    // which is exactly why the mutating end-to-end check lives in `musce_ref`.
    #[derive(Default)]
    struct Facts {
        relations: RefCell<HashSet<(EntityId, EntityId, String)>>,
        tags: RefCell<HashSet<(EntityId, String)>>,
    }
    impl Facts {
        fn add_tag(&self, entity: EntityId, tag: &str) {
            self.tags.borrow_mut().insert((entity, tag.into()));
        }

        fn commit(&self, step: &Step) {
            let Some(object) = step.frame.object else {
                return;
            };
            let destination = match step.affordance.name.as_str() {
                "take" => step.frame.actor,
                "put" => step.frame.target.unwrap(),
                _ => return,
            };
            let mut relations = self.relations.borrow_mut();
            relations
                .retain(|(source, _, kind)| *source != object || kind.as_str() != "contained_by");
            relations.insert((object, destination, "contained_by".into()));
        }
    }
    impl WorldModel for Facts {
        fn holds(&self, predicate: &Predicate, _world: &World) -> bool {
            match predicate {
                Predicate::Related { a, b, kind } => match (as_const(a), as_const(b)) {
                    (Some(a), Some(b)) => self.relations.borrow().contains(&(a, b, kind.clone())),
                    _ => false,
                },
                Predicate::Tag { e, comp } => match as_const(e) {
                    Some(e) => self.tags.borrow().contains(&(e, comp.clone())),
                    None => false,
                },
            }
        }
    }
    fn as_const(term: &Term) -> Option<EntityId> {
        match term {
            Term::Const(id) => Some(*id),
            Term::Var(_) => None,
        }
    }

    fn take() -> Affordance {
        Affordance {
            name: "take".into(),
            guards: Vec::new(),
            effect: Clause(vec![
                Predicate::Related {
                    a: Term::var("object"),
                    b: Term::var("actor"),
                    kind: "contained_by".into(),
                }
                .into(),
            ]),
        }
    }
    fn put() -> Affordance {
        Affordance {
            name: "put".into(),
            guards: vec![
                Guard {
                    clause: Clause(vec![
                        Predicate::Related {
                            a: Term::var("object"),
                            b: Term::var("actor"),
                            kind: "contained_by".into(),
                        }
                        .into(),
                    ]),
                    reason: "You aren't carrying that.",
                },
                Guard {
                    clause: Clause(vec![
                        Predicate::Tag {
                            e: Term::var("target"),
                            comp: "container".into(),
                        }
                        .into(),
                    ]),
                    reason: "You can't put things in that.",
                },
            ],
            effect: Clause(vec![
                Predicate::Related {
                    a: Term::var("object"),
                    b: Term::var("target"),
                    kind: "contained_by".into(),
                }
                .into(),
            ]),
        }
    }

    const ACTOR: EntityId = EntityId(1);
    const COIN: EntityId = EntityId(2);
    const CHEST: EntityId = EntityId(3);
    const CHEST_B: EntityId = EntityId(5);

    // ∃t. related(coin, t, contained_by) ∧ tag(t, container): the coin in some
    // container. Existential over the target, so binding chooses which chest.
    fn coin_in_a_container() -> Clause {
        Clause(vec![
            Predicate::Related {
                a: Term::Const(COIN),
                b: Term::var("t"),
                kind: "contained_by".into(),
            }
            .into(),
            Predicate::Tag {
                e: Term::var("t"),
                comp: "container".into(),
            }
            .into(),
        ])
    }

    #[test]
    fn pursues_a_plan_to_completion() {
        // One container: the plan is take-then-put and nothing vetoes, so pursuit
        // achieves the goal and every step ran once.
        let facts = Facts::default();
        facts.add_tag(CHEST, "container");
        let table = [take(), put()];
        let planner = Planner::new(&table, &facts, &UnitCost);
        let driver = Driver::new(&planner);

        let ran: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let progress = driver.pursue(
            ACTOR,
            &coin_in_a_container(),
            &[CHEST],
            &mut World::new(),
            |_w, step| {
                ran.borrow_mut().push(step.affordance.name.clone());
                facts.commit(step);
                Beat::Committed
            },
        );

        assert_eq!(progress, Progress::Achieved);
        assert_eq!(*ran.borrow(), vec!["take", "put"]);
    }

    #[test]
    fn committed_beats_do_not_claim_a_false_goal() {
        let facts = Facts::default();
        facts.add_tag(CHEST, "container");
        let table = [take(), put()];
        let planner = Planner::new(&table, &facts, &UnitCost);
        let driver = Driver::new(&planner);

        let progress = driver.pursue(
            ACTOR,
            &coin_in_a_container(),
            &[CHEST],
            &mut World::new(),
            |_world, _step| Beat::Committed,
        );

        assert_eq!(progress, Progress::Unmet);
    }

    #[test]
    fn a_permanently_vetoed_step_is_tried_once_then_abandoned() {
        // The only route needs put-into-CHEST, but that beat always vetoes (a
        // contested action). The driver excludes it, finds no other route, and
        // abandons: crucially it issues the failing put exactly once, never looping.
        let facts = Facts::default();
        facts.add_tag(CHEST, "container");
        let table = [take(), put()];
        let planner = Planner::new(&table, &facts, &UnitCost);
        let driver = Driver::new(&planner);

        let put_attempts = RefCell::new(0);
        let progress = driver.pursue(
            ACTOR,
            &coin_in_a_container(),
            &[CHEST],
            &mut World::new(),
            |_w, step| {
                if step.affordance.name == "put" {
                    *put_attempts.borrow_mut() += 1;
                    Beat::Refused
                } else {
                    facts.commit(step);
                    Beat::Committed
                }
            },
        );

        assert_eq!(progress, Progress::Abandoned);
        assert_eq!(
            *put_attempts.borrow(),
            1,
            "the excluded put must not be retried"
        );
    }

    #[test]
    fn replans_around_a_vetoed_step_onto_another_binding() {
        // Two containers. The put into whichever chest the planner picks first
        // vetoes once; the driver excludes it and replans onto the other chest,
        // where the put commits. The goal is achieved by the alternative binding.
        let facts = Facts::default();
        facts.add_tag(CHEST, "container");
        facts.add_tag(CHEST_B, "container");
        let table = [take(), put()];
        let planner = Planner::new(&table, &facts, &UnitCost);
        let driver = Driver::new(&planner);

        // Refuse the put into the first chest we are asked to put into; commit the
        // rest. The first-seen put target becomes the excluded one.
        let poisoned: RefCell<Option<EntityId>> = RefCell::new(None);
        let committed_put: RefCell<Option<EntityId>> = RefCell::new(None);
        let progress = driver.pursue(
            ACTOR,
            &coin_in_a_container(),
            &[CHEST, CHEST_B],
            &mut World::new(),
            |_w, step| {
                if step.affordance.name != "put" {
                    facts.commit(step);
                    return Beat::Committed;
                }
                let target = step.frame.target.unwrap();
                let mut poison = poisoned.borrow_mut();
                match *poison {
                    None => {
                        *poison = Some(target);
                        Beat::Refused
                    }
                    Some(first) if first == target => Beat::Refused,
                    _ => {
                        *committed_put.borrow_mut() = Some(target);
                        facts.commit(step);
                        Beat::Committed
                    }
                }
            },
        );

        assert_eq!(progress, Progress::Achieved);
        let poisoned = poisoned.borrow().unwrap();
        let committed = committed_put.borrow().unwrap();
        assert_ne!(committed, poisoned, "recovered onto the other container");
    }

    #[test]
    fn an_unreachable_goal_abandons_without_running_anything() {
        // No container exists, so no plan is ever produced: pursuit abandons and the
        // run closure is never called.
        let facts = Facts::default();
        let table = [take(), put()];
        let planner = Planner::new(&table, &facts, &UnitCost);
        let driver = Driver::new(&planner);

        let ran = RefCell::new(false);
        let progress = driver.pursue(
            ACTOR,
            &coin_in_a_container(),
            &[CHEST],
            &mut World::new(),
            |_w, _step| {
                *ran.borrow_mut() = true;
                Beat::Committed
            },
        );

        assert_eq!(progress, Progress::Abandoned);
        assert!(!*ran.borrow());
    }
}
