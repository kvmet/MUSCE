use super::*;
use crate::kinds::{Container, Creature};
use crate::names::{self, Scope};
use crate::verbs::{Locked, Outcome, do_take, go, take};

/// Build step 2: a verb's affordance is real game content, and its declared
/// effect must match what the verb actually does. Running the real `take`
/// handler and then reading the affordance's ground effect through
/// `RefWorldModel` ties the declared `contained_by` predicate to the containment
/// edge the verb commits: a wrong relation kind, a flipped direction, an empty
/// effect, or the wrong role would leave the check below false and fail. The
/// reversed-edge assertion proves the oracle discriminates rather than returning
/// true for everything.
#[test]
fn take_affordance_effect_matches_the_verb() {
    use crate::agency::{RefWorldModel, take as take_affordance};
    use musce::agency::{Frame, Predicate, Term, WorldModel};

    let mut f = fixture();
    f.world.move_entity(f.actor, f.garden).unwrap(); // be where the key is

    run(&mut f.world, f.actor, |c| take(c, "key"));

    // The frame the parser would have bound for `take key`.
    let frame = Frame {
        actor: f.actor,
        object: Some(f.key),
        target: None,
        kind: None,
    };
    let effect = take_affordance().effect.bind(&frame);
    let model = RefWorldModel;

    assert!(!effect.0.is_empty(), "take must declare an effect");
    assert!(
        effect.0.iter().all(|l| l.holds(&f.world, &model)),
        "take's declared effect does not hold after the verb ran"
    );

    // The reversed edge (actor contained_by object) is not true, so the oracle
    // is not vacuously satisfied: a flipped-direction affordance would fail above.
    let reversed = Predicate::Related {
        a: Term::Const(f.actor),
        b: Term::Const(f.key),
        kind: "contained_by".into(),
    };
    assert!(!model.holds(&reversed, &f.world));
}

/// `take`'s veto is a conjunction of negated tags (not a locus, not a player,
/// not a creature), all sharing one message. The `veto`, read through
/// `RefWorldModel`, agrees with the handler on every gameplay case, and the
/// structural cycle it deliberately does not capture stays a commit-time refusal.
#[test]
fn take_guards_predict_the_gameplay_veto() {
    use crate::agency::{RefWorldModel, take as take_affordance};
    use musce::agency::Frame;

    // A movable item: no veto, and the take commits.
    {
        let mut f = fixture();
        f.world.move_entity(f.actor, f.garden).unwrap(); // be where the key is
        let frame = Frame {
            actor: f.actor,
            object: Some(f.key),
            target: None,
            kind: None,
        };
        let veto = take_affordance()
            .veto(&frame, &f.world, &RefWorldModel)
            .map(|g| g.reason);
        run(&mut f.world, f.actor, |c| take(c, "key"));
        let committed = f.world.container_of(f.key) == Some(f.actor);
        assert!(
            veto.is_none() && committed,
            "veto {veto:?}, committed {committed}"
        );
    }
    // A being (a creature): the guard vetoes with the message the handler shows.
    {
        let mut f = fixture();
        f.world.move_entity(f.actor, f.garden).unwrap();
        let rat = spawn(&mut f.world, |b| {
            b.add(Creature);
            b.add(Description("a sewer rat".into()));
        });
        f.world.move_entity(rat, f.garden).unwrap();
        let frame = Frame {
            actor: f.actor,
            object: Some(rat),
            target: None,
            kind: None,
        };
        let veto = take_affordance()
            .veto(&frame, &f.world, &RefWorldModel)
            .map(|g| g.reason);
        assert_eq!(veto, Some("You can't take that."));
        assert_eq!(f.world.container_of(rat), Some(f.garden)); // unmoved
    }
    // A fixture (a room, tagged Locus): the guard vetoes it too.
    {
        let f = fixture();
        let frame = Frame {
            actor: f.actor,
            object: Some(f.garden),
            target: None,
            kind: None,
        };
        let veto = take_affordance()
            .veto(&frame, &f.world, &RefWorldModel)
            .map(|g| g.reason);
        assert_eq!(veto, Some("You can't take that."));
    }
    // A held container the actor stands inside: the guard permits (a container is
    // neither locus, player, nor creature), but taking it would close a cycle, so
    // the executor refuses at commit. That divergence is the guard/executor boundary.
    {
        let mut f = fixture();
        let bag = spawn(&mut f.world, |b| {
            b.add(Container);
            b.add(Description("a leather bag".into()));
        });
        f.world.move_entity(bag, f.garden).unwrap();
        f.world.move_entity(f.actor, bag).unwrap(); // the actor is inside the bag
        let frame = Frame {
            actor: f.actor,
            object: Some(bag),
            target: None,
            kind: None,
        };
        let veto = take_affordance()
            .veto(&frame, &f.world, &RefWorldModel)
            .map(|g| g.reason);
        assert!(veto.is_none(), "a container is takeable by the guard");
        // The grounded action still refuses: the move would cycle.
        assert!(matches!(
            do_take(&mut f.world, f.actor, bag),
            Outcome::Refused("You can't take that.")
        ));
    }
}

/// Build step 3: a hand-authored plan, run end to end without the parser. The
/// fungible object is bound by enumeration over what the actor knows (co-located
/// here), then the bound affordance executes through the game's grounded action.
/// Exercises the two step-3 primitives together: `bind_var` (the candidate
/// enumeration the planner reuses) and execution through the same veto a player
/// hits, proving a planned pickup is filtered and vetoed exactly as a typed one.
#[test]
fn a_planned_take_binds_a_candidate_and_runs_through_the_veto() {
    use crate::agency::{RefWorldModel, known_here, perform, take as take_affordance};
    use musce::agency::{Clause, Frame, Predicate, Term, Var, bind_var};

    let mut f = fixture();
    f.world.move_entity(f.actor, f.garden).unwrap(); // co-located with the key
    // A creature shares the room: a known entity that is *not* an item, so the
    // constraint must reject it, and the veto must refuse a take of it.
    let rat = spawn(&mut f.world, |b| {
        b.add(Creature);
        b.add(Description("a sewer rat".into()));
    });
    f.world.move_entity(rat, f.garden).unwrap();

    // The plan step: "take some x that is an item." The object role is left free
    // and constrained; enumeration fills it from what the actor knows.
    let x = Var("x".into());
    let constraint = Clause(vec![
        Predicate::Tag {
            e: Term::Var(x.clone()),
            comp: "item".into(),
        }
        .into(),
    ]);
    let candidates = known_here(&f.world, f.actor);
    let bound = bind_var(&x, &constraint, &candidates, &f.world, &RefWorldModel);

    // Only the item satisfies the constraint; the rat is filtered out.
    assert_eq!(bound, vec![f.key]);

    // Execute the bound affordance through the grounded action: the key is taken.
    let frame = Frame {
        actor: f.actor,
        object: Some(bound[0]),
        target: None,
        kind: None,
    };
    assert!(matches!(
        perform(&mut f.world, &take_affordance(), &frame, &Verdict::guest()),
        Outcome::Committed
    ));
    assert_eq!(f.world.container_of(f.key), Some(f.actor));

    // The same veto a player hits refuses a planned take of the creature: the
    // plan cannot do what a typed `take rat` cannot.
    let rat_frame = Frame {
        actor: f.actor,
        object: Some(rat),
        target: None,
        kind: None,
    };
    assert!(matches!(
        perform(
            &mut f.world,
            &take_affordance(),
            &rat_frame,
            &Verdict::guest()
        ),
        Outcome::Refused(_)
    ));
    assert_eq!(f.world.container_of(rat), Some(f.garden)); // unmoved
}

/// Build step 3, completed for the rest of the verb set: `perform` dispatches
/// `put`, `drop`, and `go` through the same grounded action (and thus the same
/// veto) the typed verb runs, not just `take`. Each verb's permitted case lands
/// the mutation and its refused case leaves the world untouched, proving the
/// old debug-assert stubs are gone and planned ≡ typed for every verb.
#[test]
fn planned_put_drop_go_run_through_the_typed_veto() {
    use crate::agency::{drop as drop_aff, go as go_aff, perform, put as put_aff};
    use crate::kinds::{Container, Item};
    use musce::agency::Frame;

    // put: a held coin into a chest commits; the same coin into a non-container
    // (a creature) is refused by the container guard, and nothing moves.
    {
        let mut f = fixture();
        let chest = spawn(&mut f.world, |b| {
            b.add(Container);
            b.add(Description("a wooden chest".into()));
        });
        f.world.move_entity(chest, f.hall).unwrap();
        let rat = spawn(&mut f.world, |b| {
            b.add(Creature);
            b.add(Description("a sewer rat".into()));
        });
        f.world.move_entity(rat, f.hall).unwrap();
        let coin = spawn(&mut f.world, |b| {
            b.add(Item);
            b.add(Description("a gold coin".into()));
        });
        f.world.move_entity(coin, f.actor).unwrap(); // held

        let framed = |obj, tgt| Frame {
            actor: f.actor,
            object: Some(obj),
            target: Some(tgt),
            kind: None,
        };
        assert!(matches!(
            perform(
                &mut f.world,
                &put_aff(),
                &framed(coin, chest),
                &Verdict::guest()
            ),
            Outcome::Committed
        ));
        assert_eq!(f.world.container_of(coin), Some(chest));

        f.world.move_entity(coin, f.actor).unwrap(); // back to hand for a clean try
        assert!(matches!(
            perform(
                &mut f.world,
                &put_aff(),
                &framed(coin, rat),
                &Verdict::guest()
            ),
            Outcome::Refused(_)
        ));
        assert_eq!(f.world.container_of(coin), Some(f.actor)); // unmoved
    }

    // drop: a held coin lands in the room; an unheld thing (the garden key) is
    // refused by the held guard.
    {
        let mut f = fixture();
        let coin = spawn(&mut f.world, |b| {
            b.add(Item);
            b.add(Description("a gold coin".into()));
        });
        f.world.move_entity(coin, f.actor).unwrap();
        let held = Frame {
            actor: f.actor,
            object: Some(coin),
            target: None,
            kind: None,
        };
        assert!(matches!(
            perform(&mut f.world, &drop_aff(), &held, &Verdict::guest()),
            Outcome::Committed
        ));
        assert_eq!(f.world.container_of(coin), Some(f.hall));

        let unheld = Frame {
            actor: f.actor,
            object: Some(f.key), // in the garden, not carried
            target: None,
            kind: None,
        };
        assert!(matches!(
            perform(&mut f.world, &drop_aff(), &unheld, &Verdict::guest()),
            Outcome::Refused(_)
        ));
        assert_eq!(f.world.container_of(f.key), Some(f.garden)); // unmoved
    }

    // go: an open exit moves the actor; a locked one is refused, actor unmoved.
    {
        let mut f = fixture();
        let north = names::resolve(&f.world, f.actor, Scope::Exits, "north").unwrap();
        let framed = Frame {
            actor: f.actor,
            object: None,
            target: Some(north),
            kind: None,
        };
        assert!(matches!(
            perform(&mut f.world, &go_aff(), &framed, &Verdict::guest()),
            Outcome::Committed
        ));
        assert_eq!(f.world.enclosing_locus(f.actor), Some(f.garden));
    }
    {
        let mut f = fixture();
        let north = names::resolve(&f.world, f.actor, Scope::Exits, "north").unwrap();
        f.world.insert(north, Locked);
        let framed = Frame {
            actor: f.actor,
            object: None,
            target: Some(north),
            kind: None,
        };
        assert!(matches!(
            perform(&mut f.world, &go_aff(), &framed, &Verdict::guest()),
            Outcome::Refused(_)
        ));
        assert_eq!(f.world.enclosing_locus(f.actor), Some(f.hall)); // unmoved
    }
}

/// The affordance gate is enforced on the automation entry: `perform` refuses a
/// cap-gated act under a verdict lacking the capability, runs it under one that
/// holds it, and lets su bypass, exactly as a `Gate::Cap` command does at
/// dispatch. A synthetic cap-gated `take` isolates the gate: the takeable guard
/// permits the key, so the gate alone decides. This is the automation-authority
/// half of the (ii) model; a player verb keeps its `CommandTable` gate instead.
#[test]
fn perform_enforces_the_affordance_gate() {
    use crate::agency::perform;
    use musce::action::{CapId, CapSet, Gate};
    use musce::agency::{Affordance, Clause, Frame};

    let cap = CapId(0);
    let gated_take = || Affordance {
        name: "take".into(),
        gate: Gate::Cap(cap),
        guards: Vec::new(),
        effect: Clause::default(),
    };
    let frame = |actor, item| Frame {
        actor,
        object: Some(item),
        target: None,
        kind: None,
    };

    // Guest verdict lacks the cap: the gate refuses before do_take runs.
    {
        let mut f = fixture();
        f.world.move_entity(f.actor, f.garden).unwrap();
        let out = perform(
            &mut f.world,
            &gated_take(),
            &frame(f.actor, f.key),
            &Verdict::guest(),
        );
        assert!(matches!(out, Outcome::Refused(_)));
        assert_eq!(f.world.container_of(f.key), Some(f.garden)); // unmoved
    }
    // A verdict holding the cap: the gate admits, the takeable key is taken.
    {
        let mut f = fixture();
        f.world.move_entity(f.actor, f.garden).unwrap();
        let granted = Verdict::new([cap].into_iter().collect(), false);
        let out = perform(
            &mut f.world,
            &gated_take(),
            &frame(f.actor, f.key),
            &granted,
        );
        assert!(matches!(out, Outcome::Committed));
        assert_eq!(f.world.container_of(f.key), Some(f.actor));
    }
    // su bypasses the gate with no grant at all.
    {
        let mut f = fixture();
        f.world.move_entity(f.actor, f.garden).unwrap();
        let su = Verdict::new(CapSet::new(), true);
        let out = perform(&mut f.world, &gated_take(), &frame(f.actor, f.key), &su);
        assert!(matches!(out, Outcome::Committed));
        assert_eq!(f.world.container_of(f.key), Some(f.actor));
    }
}

/// Guard/handler agreement for the negated guard: `go`'s `¬ tag(exit, "locked")`
/// veto, read through `RefWorldModel`, predicts the same permit/refuse (and the
/// same message) `can_traverse` produces. Proves negation is evaluated correctly
/// and that the movement veto and the affordance the planner reads cannot drift.
#[test]
fn go_guard_predicts_the_locked_veto() {
    use crate::agency::{RefWorldModel, go as go_aff};
    use musce::agency::Frame;

    let mut f = fixture();
    let north = names::resolve(&f.world, f.actor, Scope::Exits, "north").unwrap();
    let frame = Frame {
        actor: f.actor,
        object: None,
        target: Some(north),
        kind: None,
    };

    // Unlocked: no veto, and the actor traverses.
    let veto_open = go_aff()
        .veto(&frame, &f.world, &RefWorldModel)
        .map(|g| g.reason);
    assert!(veto_open.is_none());

    // Locked: the negated guard fires with exactly the message the handler shows.
    f.world.insert(north, Locked);
    let veto_locked = go_aff()
        .veto(&frame, &f.world, &RefWorldModel)
        .map(|g| g.reason);
    assert_eq!(veto_locked, Some("It's locked."));

    let out = run(&mut f.world, f.actor, |c| go(c, "north"));
    assert_eq!(f.world.enclosing_locus(f.actor), Some(f.hall)); // vetoed, didn't move
    assert!(self_feedback(&out).iter().any(|t| t.contains("locked")));
}
