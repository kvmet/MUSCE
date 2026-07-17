//! This game's agency content: the concrete affordances its verbs expose to the
//! planner, and [`RefWorldModel`], the reading of the affordance vocabulary's
//! predicates against `musce_ref`'s own relation and component names. The
//! generic mechanism lives in `musce_agency`; only this crate knows what
//! `"contained_by"` means. See `docs/architecture/agency/`.

use musce::agency::{Affordance, Clause, Frame, Guard, Predicate, Term, WorldModel};
use musce::world::{EntityId, World};

use crate::verbs::{TakeOutcome, do_take};

/// `take <item>`: the item ends up held by the actor. Its declared effect is the
/// stored containment edge (`object` `contained_by` `actor`), the same edge the
/// [`take`](crate::verbs::take) handler commits by moving the item into the
/// actor. It carries no guards on purpose: the takeable rule is a *negation*
/// (not a fixture, not a being) the current vocabulary cannot express, so the
/// veto stays whole in `do_take` until negation lands (phase C).
pub fn take() -> Affordance {
    Affordance {
        name: "take".into(),
        guards: Vec::new(),
        effect: Clause(vec![Predicate::Related {
            a: Term::var("object"),
            b: Term::var("actor"),
            kind: "contained_by".into(),
        }]),
    }
}

/// `drop <item>`: the held item ends up in the actor's room. Unlike `take`, the
/// gameplay veto (the item must be held) *is* expressible in the current
/// vocabulary as a single relation predicate, so it is a guard rather than a
/// handler check. The effect's destination is the room the actor stands in, which
/// is not a parsed role; the caller (or planner) binds `target` to the enclosing
/// locus, the same derived-location shape `go` has.
pub fn drop() -> Affordance {
    Affordance {
        name: "drop".into(),
        guards: vec![Guard {
            clause: Clause(vec![Predicate::Related {
                a: Term::var("object"),
                b: Term::var("actor"),
                kind: "contained_by".into(),
            }]),
            reason: "You aren't carrying that.",
        }],
        effect: Clause(vec![Predicate::Related {
            a: Term::var("object"),
            b: Term::var("target"),
            kind: "contained_by".into(),
        }]),
    }
}

/// `put <item> in <container>`: the held item ends up inside a container. Its
/// gameplay veto is two guards the current vocabulary expresses in full: the item
/// is held (`related(object, actor, contained_by)`) and the destination is a
/// container (`tag(target, "container")`), each with the reason the `put` handler
/// shows. The one refusal the guards do not capture is the containment *cycle*
/// (putting a held bag into itself); that is a structural invariant the executor
/// owns and re-checks at commit, not a gameplay rule, so it correctly stays out
/// of the guards.
///
/// The held guard is redundant with the handler's inventory-scoped name
/// resolution (a resolved item is already held), but the planner binds entities
/// directly with no resolution, so it needs the full precondition; the handler
/// evaluating a guard resolution already guaranteed is harmless.
pub fn put() -> Affordance {
    Affordance {
        name: "put".into(),
        guards: vec![
            Guard {
                clause: Clause(vec![Predicate::Related {
                    a: Term::var("object"),
                    b: Term::var("actor"),
                    kind: "contained_by".into(),
                }]),
                reason: "You aren't carrying that.",
            },
            Guard {
                clause: Clause(vec![Predicate::Tag {
                    e: Term::var("target"),
                    comp: "container".into(),
                }]),
                reason: "You can't put things in that.",
            },
        ],
        effect: Clause(vec![Predicate::Related {
            a: Term::var("object"),
            b: Term::var("target"),
            kind: "contained_by".into(),
        }]),
    }
}

/// Reads the affordance vocabulary's predicates against this game's world.
///
/// Each predicate kind maps to a concrete world query: a `"contained_by"`
/// relation is the containment edge [`World::container_of`] reports, and a `Tag`
/// is the presence of a component under that name. A relation kind this game
/// does not define is not a silent `false` (which would mask a typo'd
/// affordance) but a `debug_assert` in development.
pub struct RefWorldModel;

impl WorldModel for RefWorldModel {
    fn holds(&self, predicate: &Predicate, world: &World) -> bool {
        match predicate {
            Predicate::Related { a, b, kind } => {
                let (Some(a), Some(b)) = (as_const(a), as_const(b)) else {
                    return false; // a free Var: the planner's job, not held here
                };
                match kind.as_str() {
                    "contained_by" => world.container_of(a) == Some(b),
                    other => {
                        debug_assert!(false, "RefWorldModel: unknown relation kind {other:?}");
                        false
                    }
                }
            }
            Predicate::Tag { e, comp } => match as_const(e) {
                Some(e) => world.component_value(e, comp).is_some(),
                None => false,
            },
        }
    }
}

/// The entity a ground term names, or `None` if it is still a free variable.
fn as_const(term: &Term) -> Option<EntityId> {
    match term {
        Term::Const(id) => Some(*id),
        Term::Var(_) => None,
    }
}

/// The trivial knowledge seed: an actor knows the entities sharing its locus.
/// This is the MVP stand-in for perception; sense-propagation and a persisted
/// `Known` relation are a deferred layer (see `docs/architecture/agency/`). The
/// actor itself is excluded, so a plan never enumerates the actor's own body as
/// a candidate. The set a planner's [`bind_var`](musce::agency::bind_var) draws
/// candidates from.
pub fn known_here(world: &World, actor: EntityId) -> Vec<EntityId> {
    match world.enclosing_locus(actor) {
        Some(locus) => world
            .contents(locus)
            .into_iter()
            .filter(|&e| e != actor)
            .collect(),
        None => Vec::new(),
    }
}

/// Execute one bound affordance through this game's grounded action for it, so a
/// planned action is vetoed exactly as the matching player verb is. Returns
/// whether the action committed (`true`) or was refused by its rule (`false`):
/// that committed/refused bit is the per-beat outcome a planner and the step-6
/// learner read. Dispatch is by affordance name, the by-name key a player's
/// parser also uses; the frame's roles must already be ground (enumeration fills
/// them first).
pub fn perform(world: &mut World, affordance: &Affordance, frame: &Frame) -> bool {
    match affordance.name.as_str() {
        "take" => match frame.object {
            Some(item) => matches!(do_take(world, frame.actor, item), TakeOutcome::Took),
            None => {
                debug_assert!(false, "take affordance performed with no object bound");
                false
            }
        },
        other => {
            debug_assert!(
                false,
                "perform: no grounded action for affordance {other:?}"
            );
            false
        }
    }
}
