//! The GOAP planner: backward goal-regression over the affordance table.
//!
//! A search node is a subgoal [`Clause`] (the literals that must become true).
//! The search starts from the goal and works *backward*: it picks an unsatisfied
//! literal, finds an affordance whose `effect` produces it, and replaces that
//! literal with the affordance's guard preconditions, prepending the step. It
//! succeeds when every literal of a node already holds in the *actual current
//! world*, so the planner never simulates a hypothetical world; it only ever asks
//! the game's [`WorldModel`] whether a ground literal holds right now. That is why
//! regression fits this vocabulary where a forward simulation would need a
//! hypothetical-state model the engine does not have.
//!
//! The output is a transient [`Plan`]: a sequence of bound steps the game lowers
//! through its grounded action (`perform`), where the veto lives. Nothing here is
//! persisted. Cost is minimized through the game's [`CostModel`] (uniform-cost
//! search); the trivial [`UnitCost`](crate::UnitCost) makes this min-length.
//!
//! Scope of the current planner (see `docs/architecture/agency/planner.md`):
//! effects are add-only (no delete lists), so only a positive subgoal literal is
//! regressed; a negated literal is checked by `holds` and, if unsatisfied, dead-ends
//! the branch (no effect makes a fact false). Soundness under step interference is
//! backstopped by execution's replan-on-veto, not by the planner. Movement (`go`)
//! stays out until multi-room knowledge makes a cross-room goal formable.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};

use musce_core::{EntityId, World};

use musce_action::{Affordance, Clause, Frame, Literal, Predicate, Term, Var, WorldModel};

use crate::{Cost, CostModel, bind_var};

/// One bound step of a plan: an affordance and the frame grounding its roles. The
/// game lowers it through its grounded action for that affordance name.
#[derive(Debug, Clone)]
pub struct Step {
    pub affordance: Affordance,
    pub frame: Frame,
}

/// A transient, ordered action sequence the planner emits. Never persisted: a plan
/// lowers to the executor's structural `Action` set through the game's grounded
/// action, so no agency type embeds in a serialized script.
pub type Plan = Vec<Step>;

/// A backward-regression planner over a fixed affordance table. The static
/// planning context (the table and the game's read/cost policies) lives here; the
/// per-query inputs (actor, goal, known set, world) are arguments to [`plan`]. An
/// exclusion set for the replan loop slots in as a later `plan` argument with no
/// churn (see the planner doc).
///
/// [`plan`]: Planner::plan
pub struct Planner<'a> {
    affordances: &'a [Affordance],
    model: &'a dyn WorldModel,
    cost: &'a dyn CostModel,
}

/// The deepest plan the search will build, and the most nodes it will settle,
/// before giving up. Backstops against a pathological table; the reference verb
/// set plans in two steps over a handful of affordances, so neither binds in
/// practice. They exist so `plan` is total (a plan or `None`, never a spin).
const MAX_DEPTH: usize = 8;
const MAX_SETTLED: usize = 10_000;

impl<'a> Planner<'a> {
    pub fn new(
        affordances: &'a [Affordance],
        model: &'a dyn WorldModel,
        cost: &'a dyn CostModel,
    ) -> Self {
        Planner {
            affordances,
            model,
            cost,
        }
    }

    /// A minimum-cost plan whose execution makes `goal` hold for `actor`, or `None`
    /// if no chain of known affordances reaches it. `known` is the candidate set the
    /// actor may bind a fungible goal slot against (the game's knowledge seam): a
    /// goal like "hold some food" (`∃x. related(x, actor, contained_by) ∧ tag(x,
    /// food)`) enumerates `x` over `known`, grounds the goal per candidate, and keeps
    /// the cheapest plan. `world` is borrowed only for the duration of the call, so
    /// the caller can take `&mut World` to execute the returned plan immediately
    /// after.
    pub fn plan(
        &self,
        actor: EntityId,
        goal: &Clause,
        known: &[EntityId],
        world: &World,
    ) -> Option<Plan> {
        // Ground the actor role: a goal is written in the same role vocabulary as
        // affordance clauses, so `actor` names the planning agent.
        let goal = goal.substitute(&Var("actor".to_string()), actor);

        let free = free_vars(&goal);
        let result = match free.as_slice() {
            [] => self.regress(actor, &goal, world),
            [var] => {
                // Bind the fungible slot: candidates are the known entities the
                // *static* part of the goal admits (the properties no affordance can
                // grant, so they must already hold, e.g. `tag(x, food)`). The
                // achievable part (`related(x, actor, contained_by)`) is left for
                // regression to plan. Each grounding is regressed; the cheapest wins.
                let filter = self.static_filter(var, &goal);
                bind_var(var, &filter, known, world, self.model)
                    .into_iter()
                    .filter_map(|c| self.regress(actor, &goal.substitute(var, c), world))
                    .min_by_key(|(cost, _)| *cost)
            }
            // Multiple free vars is a combinatorial product no current goal needs;
            // deferred (see the planner doc). A two-var goal simply finds no plan.
            _ => None,
        };
        result.map(|(_, plan)| plan)
    }

    /// The literals of `goal` that mention `var` and that no affordance can make
    /// true (no positive effect shares their shape), so they must already hold: the
    /// static constraint that filters candidate bindings for `var`. A negated literal
    /// is always static (no add-only effect makes a fact false). The rest of the
    /// goal, the achievable part, is planned by regression, not used to filter.
    fn static_filter(&self, var: &Var, goal: &Clause) -> Clause {
        Clause(
            goal.0
                .iter()
                .filter(|l| mentions(l, var) && !self.is_achievable(l))
                .cloned()
                .collect(),
        )
    }

    /// Whether some affordance's positive effect could produce this literal: a
    /// positive literal whose predicate shares a kind/comp with an effect. Used only
    /// to split a goal into its achievable and static parts.
    fn is_achievable(&self, literal: &Literal) -> bool {
        !literal.negated
            && self
                .affordances
                .iter()
                .flat_map(|a| a.effect.0.iter())
                .filter(|e| !e.negated)
                .any(|e| same_shape(&e.predicate, &literal.predicate))
    }

    /// Uniform-cost backward search from `goal`. Nodes are settled cheapest-first,
    /// so the first node whose every literal holds yields an optimal plan, returned
    /// with its total cost.
    fn regress(&self, actor: EntityId, goal: &Clause, world: &World) -> Option<(Cost, Plan)> {
        let mut heap = BinaryHeap::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut seq: u64 = 0;
        heap.push(Frontier {
            cost: 0,
            seq,
            subgoal: goal.clone(),
            steps: Vec::new(),
        });

        let mut settled = 0;
        while let Some(node) = heap.pop() {
            // Settle-on-pop: the cheapest path to this subgoal is now final.
            if !visited.insert(canonical(&node.subgoal)) {
                continue;
            }
            settled += 1;
            if settled > MAX_SETTLED {
                return None;
            }

            let unsatisfied: Vec<&Literal> = node
                .subgoal
                .0
                .iter()
                .filter(|l| !l.holds(world, self.model))
                .collect();
            if unsatisfied.is_empty() {
                let mut plan = node.steps;
                plan.reverse(); // pick order is last-executed-first
                return Some((node.cost, plan));
            }
            // A negated literal cannot be achieved: no add-only effect makes a fact
            // false, so a branch that still needs one is dead. (Its message-side
            // twin, a guard that already holds, passed the filter above.)
            if unsatisfied.iter().any(|l| l.negated) {
                continue;
            }
            if node.steps.len() >= MAX_DEPTH {
                continue;
            }

            // Regress one unsatisfied (positive) literal by every affordance whose
            // effect can produce it.
            let target = unsatisfied[0].predicate.clone();
            for affordance in self.affordances {
                for effect in affordance.effect.0.iter().filter(|l| !l.negated) {
                    let Some(frame) = unify(&effect.predicate, &target, actor) else {
                        continue;
                    };
                    let subgoal = successor(&node.subgoal, affordance, &frame);
                    if visited.contains(&canonical(&subgoal)) {
                        continue;
                    }
                    seq += 1;
                    let mut steps = node.steps.clone();
                    steps.push(Step {
                        affordance: affordance.clone(),
                        frame,
                    });
                    heap.push(Frontier {
                        cost: node.cost + self.cost.cost(actor, affordance, world),
                        seq,
                        subgoal,
                        steps,
                    });
                }
            }
        }
        None
    }
}

/// The distinct free variables in a clause (every `Term::Var`), in first-seen
/// order. After actor-substitution these are the goal's existential slots.
fn free_vars(clause: &Clause) -> Vec<Var> {
    let mut vars: Vec<Var> = Vec::new();
    let mut note = |term: &Term| {
        if let Term::Var(v) = term
            && !vars.contains(v)
        {
            vars.push(v.clone());
        }
    };
    for literal in &clause.0 {
        match &literal.predicate {
            Predicate::Related { a, b, .. } => {
                note(a);
                note(b);
            }
            Predicate::Tag { e, .. } => note(e),
        }
    }
    vars
}

/// Whether `var` appears in a literal's predicate.
fn mentions(literal: &Literal, var: &Var) -> bool {
    let is = |term: &Term| matches!(term, Term::Var(v) if v == var);
    match &literal.predicate {
        Predicate::Related { a, b, .. } => is(a) || is(b),
        Predicate::Tag { e, .. } => is(e),
    }
}

/// Whether two predicates could unify on their game vocabulary alone (same variant
/// and same relation kind / component), ignoring terms. The cheap achievability
/// test the static/achievable goal split uses.
fn same_shape(a: &Predicate, b: &Predicate) -> bool {
    match (a, b) {
        (Predicate::Related { kind: ka, .. }, Predicate::Related { kind: kb, .. }) => ka == kb,
        (Predicate::Tag { comp: ca, .. }, Predicate::Tag { comp: cb, .. }) => ca == cb,
        _ => false,
    }
}

/// The subgoal after regressing through `affordance` under `frame`: the literals
/// the affordance's bound effect produces are discharged from `subgoal`, and its
/// bound guard preconditions become new obligations. Literals are deduplicated so
/// two affordances requiring the same precondition do not double it.
fn successor(subgoal: &Clause, affordance: &Affordance, frame: &Frame) -> Clause {
    let produced = affordance.effect.bind(frame);
    let mut next: Vec<Literal> = subgoal
        .0
        .iter()
        .filter(|l| !produced.0.contains(l))
        .cloned()
        .collect();
    for guard in &affordance.guards {
        for literal in guard.clause.bind(frame).0 {
            if !next.contains(&literal) {
                next.push(literal);
            }
        }
    }
    Clause(next)
}

/// Match an affordance's (positive) effect predicate against a ground subgoal
/// predicate, producing the [`Frame`] that grounds the affordance's roles, or
/// `None` if they cannot match. Not general unification: the frame has three fixed
/// roles (`actor`, `object`, `target`), so this is a small positional match of the
/// effect's role-vars against the subgoal's entities, with `actor` fixed to the
/// planning agent.
fn unify(effect: &Predicate, goal: &Predicate, actor: EntityId) -> Option<Frame> {
    let mut frame = Frame {
        actor,
        object: None,
        target: None,
        kind: None,
    };
    match (effect, goal) {
        (
            Predicate::Related {
                a: ea,
                b: eb,
                kind: ek,
            },
            Predicate::Related {
                a: ga,
                b: gb,
                kind: gk,
            },
        ) if ek == gk => {
            unify_term(ea, ga, &mut frame)?;
            unify_term(eb, gb, &mut frame)?;
            Some(frame)
        }
        (Predicate::Tag { e: ee, comp: ec }, Predicate::Tag { e: ge, comp: gc }) if ec == gc => {
            unify_term(ee, ge, &mut frame)?;
            Some(frame)
        }
        _ => None,
    }
}

/// Match one effect term against the (ground) subgoal term, recording any role
/// binding into `frame`. The subgoal term must be ground; an effect `Const` must
/// equal it, an effect role-var binds (or must agree with) its frame slot, and an
/// effect var that is not a frame role cannot be grounded here.
fn unify_term(effect: &Term, goal: &Term, frame: &mut Frame) -> Option<()> {
    let Term::Const(id) = goal else {
        return None; // the subgoal is expected ground; a free var cannot match
    };
    let id = *id;
    match effect {
        Term::Const(c) => (*c == id).then_some(()),
        Term::Var(Var(role)) => match role.as_str() {
            "actor" => (frame.actor == id).then_some(()),
            "object" => set_slot(&mut frame.object, id),
            "target" => set_slot(&mut frame.target, id),
            _ => None,
        },
    }
}

/// Bind a frame slot to `id`, or require agreement if it is already bound.
fn set_slot(slot: &mut Option<EntityId>, id: EntityId) -> Option<()> {
    match slot {
        Some(existing) => (*existing == id).then_some(()),
        None => {
            *slot = Some(id);
            Some(())
        }
    }
}

/// An order-independent key for a subgoal, for the visited set. Two subgoals with
/// the same literal set are the same node regardless of literal order. Built from
/// the derived `Debug` of each literal, sorted; the search is small enough that the
/// string cost is irrelevant, and this avoids imposing a total order on the
/// vocabulary types for a planner-internal need.
fn canonical(clause: &Clause) -> String {
    let mut literals: Vec<String> = clause.0.iter().map(|l| format!("{l:?}")).collect();
    literals.sort();
    literals.join("|")
}

/// A frontier node, ordered for a min-heap by cost. `seq` (unique per push) breaks
/// ties into a total, deterministic order so equal-cost plans are found in a stable
/// sequence.
struct Frontier {
    cost: Cost,
    seq: u64,
    subgoal: Clause,
    steps: Vec<Step>,
}

impl PartialEq for Frontier {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq
    }
}
impl Eq for Frontier {}
impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap; reverse so least cost (then earliest seq) is
        // popped first.
        other
            .cost
            .cmp(&self.cost)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Gate, Guard, UnitCost};

    // A ground-fact stub standing in for a game's reading: `holds` answers from a
    // fixed set of true relations and tags, ignoring the (empty) `World`. It plays
    // the role `RefWorldModel` plays for `musce_ref`, letting the generic planner
    // be tested without a game. The relation kinds and tags are illustrative.
    #[derive(Default)]
    struct Facts {
        relations: HashSet<(EntityId, EntityId, String)>,
        tags: HashSet<(EntityId, String)>,
    }
    impl WorldModel for Facts {
        fn holds(&self, predicate: &Predicate, _world: &World) -> bool {
            match predicate {
                Predicate::Related { a, b, kind } => match (as_const(a), as_const(b)) {
                    (Some(a), Some(b)) => self.relations.contains(&(a, b, kind.clone())),
                    _ => false,
                },
                Predicate::Tag { e, comp } => match as_const(e) {
                    Some(e) => self.tags.contains(&(e, comp.clone())),
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

    // The two chaining verbs under test, mirroring `musce_ref`'s affordances: take
    // makes the object held (`object contained_by actor`); put makes it contained
    // by the target, guarded on the object being held and the target being a
    // container.
    fn take() -> Affordance {
        Affordance {
            name: "take".into(),
            gate: Gate::Open,
            guards: vec![Guard {
                clause: Clause(vec![
                    Predicate::Tag {
                        e: Term::var("object"),
                        comp: "fixture".into(),
                    }
                    .not(),
                ]),
                reason: "You can't take that.",
            }],
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
            gate: Gate::Open,
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

    fn coin_in_chest_goal() -> Clause {
        Clause(vec![
            Predicate::Related {
                a: Term::Const(COIN),
                b: Term::Const(CHEST),
                kind: "contained_by".into(),
            }
            .into(),
        ])
    }

    #[test]
    fn regresses_a_two_step_chain() {
        // Coin on the floor, chest is a container: reaching "coin in chest" needs
        // take (to hold the coin) then put. This is the backward chain: put's
        // held-precondition is achieved by take's effect.
        let mut facts = Facts::default();
        facts.tags.insert((CHEST, "container".into()));
        let table = [take(), put()];
        let planner = Planner::new(&table, &facts, &UnitCost);

        let plan = planner
            .plan(ACTOR, &coin_in_chest_goal(), &[], &World::new())
            .expect("a plan exists");

        let names: Vec<&str> = plan.iter().map(|s| s.affordance.name.as_str()).collect();
        assert_eq!(names, ["take", "put"]);
        assert_eq!(plan[0].frame.object, Some(COIN));
        assert_eq!(plan[1].frame.object, Some(COIN));
        assert_eq!(plan[1].frame.target, Some(CHEST));
        assert!(plan.iter().all(|s| s.frame.actor == ACTOR));
    }

    #[test]
    fn already_satisfied_goal_plans_nothing() {
        // The coin is already in the chest: the goal holds, so the empty plan is
        // optimal.
        let mut facts = Facts::default();
        facts.relations.insert((COIN, CHEST, "contained_by".into()));
        let table = [take(), put()];
        let plan = Planner::new(&table, &facts, &UnitCost)
            .plan(ACTOR, &coin_in_chest_goal(), &[], &World::new())
            .expect("an empty plan");
        assert!(plan.is_empty());
    }

    #[test]
    fn no_plan_when_a_precondition_is_unreachable() {
        // The chest is not a container and no affordance makes it one, so put's
        // container guard can never hold: there is no plan.
        let facts = Facts::default(); // CHEST lacks the container tag
        let table = [take(), put()];
        let plan = Planner::new(&table, &facts, &UnitCost).plan(
            ACTOR,
            &coin_in_chest_goal(),
            &[],
            &World::new(),
        );
        assert!(plan.is_none());
    }

    #[test]
    fn a_negated_guard_that_holds_does_not_block() {
        // The coin is not a fixture, so take's `¬fixture` guard holds and is not
        // something the planner tries (and fails) to achieve.
        let mut facts = Facts::default();
        facts.tags.insert((CHEST, "container".into()));
        let table = [take(), put()];
        let plan = Planner::new(&table, &facts, &UnitCost)
            .plan(ACTOR, &coin_in_chest_goal(), &[], &World::new())
            .expect("a plan exists");
        assert_eq!(plan.len(), 2);
    }

    #[test]
    fn binds_a_fungible_goal_slot_from_known_and_plans_for_it() {
        // Goal: "hold some item" — ∃x. related(x, actor, contained_by) ∧ tag(x,
        // item). The item tag is static (no affordance grants it), so it filters the
        // candidates; the held relation is achievable, so take plans it. A known
        // non-item (a rat) must be filtered out, not planned toward.
        const RAT: EntityId = EntityId(4);
        let mut facts = Facts::default();
        facts.tags.insert((COIN, "item".into()));
        // RAT is known but not an item; nothing tags it, so the filter rejects it.

        let goal = Clause(vec![
            Predicate::Related {
                a: Term::var("x"),
                b: Term::var("actor"),
                kind: "contained_by".into(),
            }
            .into(),
            Predicate::Tag {
                e: Term::var("x"),
                comp: "item".into(),
            }
            .into(),
        ]);

        let table = [take(), put()];
        let known = [COIN, RAT];
        let plan = Planner::new(&table, &facts, &UnitCost)
            .plan(ACTOR, &goal, &known, &World::new())
            .expect("a plan for the one known item");

        let names: Vec<&str> = plan.iter().map(|s| s.affordance.name.as_str()).collect();
        assert_eq!(names, ["take"]);
        assert_eq!(plan[0].frame.object, Some(COIN)); // the item, not the rat
    }

    #[test]
    fn no_known_candidate_fits_the_goal_slot() {
        // The one known entity is not an item, so the fungible slot binds nothing
        // and there is no plan (the "nothing known fits" answer, not an error).
        let facts = Facts::default(); // COIN lacks the item tag
        let goal = Clause(vec![
            Predicate::Related {
                a: Term::var("x"),
                b: Term::var("actor"),
                kind: "contained_by".into(),
            }
            .into(),
            Predicate::Tag {
                e: Term::var("x"),
                comp: "item".into(),
            }
            .into(),
        ]);
        let table = [take(), put()];
        let plan =
            Planner::new(&table, &facts, &UnitCost).plan(ACTOR, &goal, &[COIN], &World::new());
        assert!(plan.is_none());
    }

    #[test]
    fn a_negated_precondition_that_fails_dead_ends() {
        // Make the coin a fixture: take's `¬fixture` guard fails and nothing can
        // un-fixture it, so the only chain to the goal is dead and no plan exists.
        let mut facts = Facts::default();
        facts.tags.insert((CHEST, "container".into()));
        facts.tags.insert((COIN, "fixture".into()));
        let table = [take(), put()];
        let plan = Planner::new(&table, &facts, &UnitCost).plan(
            ACTOR,
            &coin_in_chest_goal(),
            &[],
            &World::new(),
        );
        assert!(plan.is_none());
    }
}
