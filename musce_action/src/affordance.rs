//! The affordance vocabulary: the symbolic shape a verb's precondition and effect
//! are written in, and the [`WorldModel`] seam that reads it against the world.
//! This lives in the engine, non-optional, because a verb-gate ("may this actor
//! do this now, and if not, why?") is a dispatch concern that exists whether or
//! not anything plans. The GOAP planner in `musce_agency` is one *consumer* of
//! this vocabulary, not its owner. See `docs/architecture/affordances.md`.
//!
//! Game content, the concrete affordances and the relation/component vocabulary
//! their predicates name, lives in the consumer crate, never here. The engine
//! iterates a clause and calls back into the game-supplied [`WorldModel`]; it
//! interprets no game vocabulary itself.
//!
//! Deferred optimization: relation kinds, component tags, and `Var` names are
//! all `String` here, matching the executor's `Action`. None of it is
//! serialized, so a future global symbol table interning these to `Copy` ids
//! engine-wide is a pure internal swap, not a migration. Tracked as an upcoming
//! optimization, not built.

use musce_core::{EntityId, World};

/// A predicate argument: a bound entity, or a variable the planner binds by
/// enumeration. `Const` vs `Var` is the fungibility axis: a non-fungible want
/// ("greet *this* king") pins a `Const`; a fungible one ("any food") is a `Var`
/// plus the constraints that filter it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Term {
    Const(EntityId),
    Var(Var),
}

/// A planner variable, named within its clause.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Var(pub String);

/// The two chainable predicate kinds. Their parameters, a relation `kind` and a
/// component `comp`, are game vocabulary the engine never interprets, carried as
/// the same tag strings the executor's `Action` uses. A new game state is a new
/// *parameter*, never a new variant: `cooked` / `worn` / `armor` are
/// `Tag(_, "cooked")`, `Related(_, _, "worn")`, `Tag(_, "armor")`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Predicate {
    /// A relation of kind `kind` links `a` to `b`.
    Related { a: Term, b: Term, kind: String },
    /// `e` bears the component/marker `comp`.
    Tag { e: Term, comp: String },
}

/// A precondition or goal: a conjunction of predicates whose `Var`s are
/// existentially bound by the planner. An empty clause is trivially satisfied.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Clause(pub Vec<Predicate>);

impl Term {
    /// A planner variable term, by name. Sugar for `Term::Var(Var(name.into()))`.
    pub fn var(name: &str) -> Term {
        Term::Var(Var(name.into()))
    }

    /// Substitute a role-var with the entity a frame binds it to. A `Const`, or a
    /// `Var` that names no bound role (a free variable the planner will bind), is
    /// returned unchanged.
    pub fn bind(&self, frame: &Frame) -> Term {
        match self {
            Term::Var(v) => match frame.role(v) {
                Some(id) => Term::Const(id),
                None => self.clone(),
            },
            Term::Const(_) => self.clone(),
        }
    }

    /// Replace this term with `id` iff it is exactly `Term::Var(var)`. Unlike
    /// [`Term::bind`], which maps the frame *role* names, this substitutes one
    /// named variable: the primitive that branches a candidate entity into an
    /// existential clause (see `musce_agency::bind_var`).
    pub fn substitute(&self, var: &Var, id: EntityId) -> Term {
        match self {
            Term::Var(v) if v == var => Term::Const(id),
            _ => self.clone(),
        }
    }
}

impl Predicate {
    /// Substitute a frame's bound roles into this predicate's terms.
    pub fn bind(&self, frame: &Frame) -> Predicate {
        match self {
            Predicate::Related { a, b, kind } => Predicate::Related {
                a: a.bind(frame),
                b: b.bind(frame),
                kind: kind.clone(),
            },
            Predicate::Tag { e, comp } => Predicate::Tag {
                e: e.bind(frame),
                comp: comp.clone(),
            },
        }
    }

    /// Substitute the named variable `var` with `id` throughout this predicate's
    /// terms.
    pub fn substitute(&self, var: &Var, id: EntityId) -> Predicate {
        match self {
            Predicate::Related { a, b, kind } => Predicate::Related {
                a: a.substitute(var, id),
                b: b.substitute(var, id),
                kind: kind.clone(),
            },
            Predicate::Tag { e, comp } => Predicate::Tag {
                e: e.substitute(var, id),
                comp: comp.clone(),
            },
        }
    }
}

impl Clause {
    /// Ground this clause against a frame: each role-var becomes the entity the
    /// frame fills it with, and free variables are left for the planner. A
    /// precondition or effect written over roles becomes a concrete predicate set.
    pub fn bind(&self, frame: &Frame) -> Clause {
        Clause(self.0.iter().map(|p| p.bind(frame)).collect())
    }

    /// Substitute the named variable `var` with `id` throughout this clause.
    pub fn substitute(&self, var: &Var, id: EntityId) -> Clause {
        Clause(self.0.iter().map(|p| p.substitute(var, id)).collect())
    }
}

/// The case frame: which entities fill an affordance's roles for one grounded
/// instance. The parser fills it from a command line, the planner by
/// unification. Its arity is fixed: `actor`, two object slots (`object`,
/// `target`), and a relation `kind` (the preposition). That is the shape the
/// parser and the structural `Action` already carry; a third object slot would
/// be a migration, so it is deliberately absent (see the affordances doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub actor: EntityId,
    pub object: Option<EntityId>,
    pub target: Option<EntityId>,
    pub kind: Option<String>,
}

impl Frame {
    /// The entity a role-var binds to, or `None` if the var names no role this
    /// frame fills. The role names (`actor` / `object` / `target`) are the
    /// binding convention between an affordance's clauses and its frame.
    fn role(&self, v: &Var) -> Option<EntityId> {
        match v.0.as_str() {
            "actor" => Some(self.actor),
            "object" => self.object,
            "target" => self.target,
            _ => None,
        }
    }
}

/// A grounded action: the reusable unit a player verb and the planner both
/// resolve to. This is the generic shape; concrete instances (which relation
/// kinds and components a given verb names, its rule, its prose) are game
/// content in the consumer crate.
///
/// Cost is deliberately not a field: it is not a property of the action but a
/// game policy over `(actor, affordance, world)`, supplied through
/// `musce_agency::CostModel`, so an actor's costs can vary and later be learned
/// (agency build step 6).
#[derive(Debug, Clone)]
pub struct Affordance {
    /// By-name key for the parser's lookup.
    pub name: String,
    /// Predicates that must hold for the action to be *plannable*: a symbolic
    /// approximation of the handler's real pre-commit rule, which stays truth
    /// and re-checks at execution.
    pub precondition: Clause,
    /// The predicates the action makes true, so the planner can chain backward
    /// toward a goal. Declared explicitly rather than projected off the
    /// committed `Action`, keeping the executor's internals out of the
    /// mechanism. Auto-projection is a possible later refinement (see the
    /// affordances doc); it may never be worth it.
    pub effect: Clause,
}

/// The game-supplied reading of a predicate against the world. A predicate names
/// game vocabulary (`"contained_by"`, `"armor"`) the engine never interprets, so
/// only the game can say whether it holds. Dispatch uses this to test a verb's
/// guards; the planner uses it to test whether a goal or precondition is already
/// satisfied.
///
/// There is deliberately no generic default: a crate that never names game
/// vocabulary has no reading to offer, and that absence is the boundary working
/// as intended. Every game supplies its own model in the consumer crate.
///
/// Only *ground* predicates (every [`Term`] a [`Term::Const`]) are meaningful.
/// Binding a free [`Var`] by enumerating candidate entities is a separate
/// planner primitive (`musce_agency::bind_var`), not this trait; an
/// implementation may treat a predicate that still carries a `Var` as not held.
pub trait WorldModel {
    /// Whether `predicate` holds in `world` right now.
    fn holds(&self, predicate: &Predicate, world: &World) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str) -> Term {
        Term::var(name)
    }

    // The worked clauses below come from the affordances design doc; the point
    // is to prove the vocabulary can express them. The relation kinds and
    // components ("contains", "food", "worn", "armor") are illustrative game
    // vocabulary, not a claim about musce_ref's actual tags.

    #[test]
    fn expresses_have_food_goal() {
        // ∃x. holds(self, x) ∧ tag(x, Food)
        let goal = Clause(vec![
            Predicate::Related {
                a: var("self"),
                b: var("x"),
                kind: "contains".into(),
            },
            Predicate::Tag {
                e: var("x"),
                comp: "food".into(),
            },
        ]);
        assert_eq!(goal.0.len(), 2);
    }

    #[test]
    fn expresses_wearing_armor_goal() {
        // ∃x. related(self, x, Worn) ∧ tag(x, Armor)
        let goal = Clause(vec![
            Predicate::Related {
                a: var("self"),
                b: var("x"),
                kind: "worn".into(),
            },
            Predicate::Tag {
                e: var("x"),
                comp: "armor".into(),
            },
        ]);
        assert!(goal.0.contains(&Predicate::Tag {
            e: var("x"),
            comp: "armor".into(),
        }));
    }

    #[test]
    fn const_and_var_terms_are_distinct() {
        // The fungibility axis: pinning an entity is a Const, "any" is a Var.
        let king = Term::Const(EntityId(7));
        assert_ne!(king, var("x"));
    }

    #[test]
    fn take_affordance_declares_its_effect() {
        // take = Move(item, into=actor); effect holds(actor, object).
        let take = Affordance {
            name: "take".into(),
            precondition: Clause::default(),
            effect: Clause(vec![Predicate::Related {
                a: var("actor"),
                b: var("object"),
                kind: "contains".into(),
            }]),
        };
        assert_eq!(take.name, "take");
        assert_eq!(take.effect.0.len(), 1);
    }

    #[test]
    fn frame_binds_role_vars_to_consts() {
        // take's effect over roles: object becomes contained_by actor.
        let effect = Clause(vec![Predicate::Related {
            a: var("object"),
            b: var("actor"),
            kind: "contained_by".into(),
        }]);
        let frame = Frame {
            actor: EntityId(1),
            object: Some(EntityId(2)),
            target: None,
            kind: None,
        };
        assert_eq!(
            effect.bind(&frame),
            Clause(vec![Predicate::Related {
                a: Term::Const(EntityId(2)),
                b: Term::Const(EntityId(1)),
                kind: "contained_by".into(),
            }])
        );
    }

    #[test]
    fn binding_leaves_free_vars_for_the_planner() {
        // `x` names no frame role, so it survives as a Var (the "any food" slot).
        let c = Clause(vec![Predicate::Tag {
            e: var("x"),
            comp: "food".into(),
        }]);
        let frame = Frame {
            actor: EntityId(1),
            object: None,
            target: None,
            kind: None,
        };
        assert_eq!(c.bind(&frame), c);
    }
}
