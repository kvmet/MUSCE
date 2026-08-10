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
    };
    assert!(matches!(
        perform(&mut f.world, &take_affordance(), &frame),
        Outcome::Committed
    ));
    assert_eq!(f.world.container_of(f.key), Some(f.actor));

    // The same veto a player hits refuses a planned take of the creature: the
    // plan cannot do what a typed `take rat` cannot.
    let rat_frame = Frame {
        actor: f.actor,
        object: Some(rat),
        target: None,
    };
    assert!(matches!(
        perform(&mut f.world, &take_affordance(), &rat_frame),
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
        };
        assert!(matches!(
            perform(&mut f.world, &put_aff(), &framed(coin, chest)),
            Outcome::Committed
        ));
        assert_eq!(f.world.container_of(coin), Some(chest));

        f.world.move_entity(coin, f.actor).unwrap(); // back to hand for a clean try
        assert!(matches!(
            perform(&mut f.world, &put_aff(), &framed(coin, rat)),
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
        };
        assert!(matches!(
            perform(&mut f.world, &drop_aff(), &held),
            Outcome::Committed
        ));
        assert_eq!(f.world.container_of(coin), Some(f.hall));

        let unheld = Frame {
            actor: f.actor,
            object: Some(f.key), // in the garden, not carried
            target: None,
        };
        assert!(matches!(
            perform(&mut f.world, &drop_aff(), &unheld),
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
        };
        assert!(matches!(
            perform(&mut f.world, &go_aff(), &framed),
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
        };
        assert!(matches!(
            perform(&mut f.world, &go_aff(), &framed),
            Outcome::Refused(_)
        ));
        assert_eq!(f.world.enclosing_locus(f.actor), Some(f.hall)); // unmoved
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

/// Build step 4: the planner's *own* output, run through the same grounded action
/// a player hits, satisfies the goal. Reaching "coin in chest" requires the
/// backward chain the planner exists to find: put's held-precondition is achieved
/// by take, so the plan is take-then-put, discovered by regression, not authored.
/// The oracle is an independent check against world state (the goal predicate
/// became true), not a comparison to a hand-written plan.
#[test]
fn a_regressed_plan_run_through_perform_satisfies_the_goal() {
    use crate::agency::{RefWorldModel, known_here, perform, put as put_aff, take as take_aff};
    use crate::kinds::{Container, Item};
    use musce::agency::{Clause, Planner, Predicate, Term, UnitCost};

    let mut f = fixture();
    // A coin on the floor and a chest, both co-located with the actor in the hall.
    let chest = spawn(&mut f.world, |b| {
        b.add(Container);
        b.add(Description("a wooden chest".into()));
    });
    f.world.move_entity(chest, f.hall).unwrap();
    let coin = spawn(&mut f.world, |b| {
        b.add(Item);
        b.add(Description("a gold coin".into()));
    });
    f.world.move_entity(coin, f.hall).unwrap();

    // The goal: the coin ends up inside the chest. The coin is not held, so the
    // planner must insert the take that puts it in hand before the put.
    let goal = Clause(vec![
        Predicate::Related {
            a: Term::Const(coin),
            b: Term::Const(chest),
            kind: "contained_by".into(),
        }
        .into(),
    ]);

    let table = [take_aff(), put_aff()];
    let known = known_here(&f.world, f.actor);
    let plan = Planner::new(&table, &RefWorldModel, &UnitCost)
        .plan(f.actor, &goal, &known, &f.world)
        .expect("the planner finds a take-then-put plan");

    let names: Vec<&str> = plan.iter().map(|s| s.affordance.name.as_str()).collect();
    assert_eq!(names, ["take", "put"]);

    // Run the plan through the same veto-checked grounded action a typed verb
    // runs, then assert the world satisfies the goal.
    for step in &plan {
        assert!(
            matches!(
                perform(&mut f.world, &step.affordance, &step.frame),
                Outcome::Committed
            ),
            "planned {} was refused",
            step.affordance.name
        );
    }
    assert_eq!(f.world.container_of(coin), Some(chest));
}

/// Build step 4b: an existential goal ("hold some item") binds its fungible slot
/// from what the actor knows, then plans for the chosen entity. The item tag
/// filters the candidates (a co-located creature is rejected), and regression
/// plans the take. Run through `perform`, the actor ends up holding the item.
#[test]
fn a_fungible_goal_binds_a_known_item_and_plans_the_take() {
    use crate::agency::{RefWorldModel, known_here, perform, put as put_aff, take as take_aff};
    use crate::kinds::Item;
    use musce::agency::{Clause, Planner, Predicate, Term, UnitCost};

    let mut f = fixture();
    // A coin (an item) and a rat (a creature, not an item), both co-located.
    let coin = spawn(&mut f.world, |b| {
        b.add(Item);
        b.add(Description("a gold coin".into()));
    });
    f.world.move_entity(coin, f.hall).unwrap();
    let rat = spawn(&mut f.world, |b| {
        b.add(Creature);
        b.add(Description("a sewer rat".into()));
    });
    f.world.move_entity(rat, f.hall).unwrap();

    // ∃x. related(x, actor, contained_by) ∧ tag(x, item): hold some item.
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

    let table = [take_aff(), put_aff()];
    let known = known_here(&f.world, f.actor);
    let plan = Planner::new(&table, &RefWorldModel, &UnitCost)
        .plan(f.actor, &goal, &known, &f.world)
        .expect("the planner binds the item and plans a take");

    let names: Vec<&str> = plan.iter().map(|s| s.affordance.name.as_str()).collect();
    assert_eq!(names, ["take"]);
    assert_eq!(plan[0].frame.object, Some(coin)); // the item, not the rat

    for step in &plan {
        assert!(matches!(
            perform(&mut f.world, &step.affordance, &step.frame),
            Outcome::Committed
        ));
    }
    assert_eq!(f.world.container_of(coin), Some(f.actor));
    assert_eq!(f.world.container_of(rat), Some(f.hall)); // the rat was never chosen
}

/// Build step 5, the execution driver: replan-on-veto against live world state. A
/// plan's put beat is made to veto for real (a chest is smashed mid-plan, standing
/// in for another actor), and the driver must exclude that step and replan from the
/// now-current world onto the surviving container. The take already committed, so
/// the coin is genuinely held; the replan does *not* redo it, proving the driver
/// reads live state, not the state it planned against. The goal ends true.
#[test]
fn the_driver_replans_around_a_vetoed_beat_and_finishes_the_goal() {
    use crate::agency::{RefWorldModel, known_here, perform, put as put_aff, take as take_aff};
    use crate::kinds::{Container, Item};
    use musce::agency::{Beat, Clause, Driver, Planner, Predicate, Progress, Term, UnitCost};
    use std::cell::RefCell;

    let mut f = fixture();
    // Two chests and a loose coin, all co-located with the actor in the hall.
    let chest_a = spawn(&mut f.world, |b| {
        b.add(Container);
        b.add(Description("an oak chest".into()));
    });
    let chest_b = spawn(&mut f.world, |b| {
        b.add(Container);
        b.add(Description("an iron chest".into()));
    });
    f.world.move_entity(chest_a, f.hall).unwrap();
    f.world.move_entity(chest_b, f.hall).unwrap();
    let coin = spawn(&mut f.world, |b| {
        b.add(Item);
        b.add(Description("a gold coin".into()));
    });
    f.world.move_entity(coin, f.hall).unwrap();

    // ∃t. related(coin, t, contained_by) ∧ tag(t, container): put the coin in *some*
    // container. Which chest is the driver's to pick, and to re-pick when one fails.
    let goal = Clause(vec![
        Predicate::Related {
            a: Term::Const(coin),
            b: Term::var("t"),
            kind: "contained_by".into(),
        }
        .into(),
        Predicate::Tag {
            e: Term::var("t"),
            comp: "container".into(),
        }
        .into(),
    ]);

    let table = [take_aff(), put_aff()];
    let known = known_here(&f.world, f.actor);
    let planner = Planner::new(&table, &RefWorldModel, &UnitCost);
    let driver = Driver::new(&planner);

    // Smash the first chest the driver tries to put into (another force breaks it),
    // then perform the beat: the container guard now vetoes it for real.
    let smashed: RefCell<Option<musce::world::EntityId>> = RefCell::new(None);
    let progress = driver.pursue(f.actor, &goal, &known, &mut f.world, |world, step| {
        if step.affordance.name == "put" && smashed.borrow().is_none() {
            let target = step.frame.target.unwrap();
            world.remove::<Container>(target);
            *smashed.borrow_mut() = Some(target);
        }
        match perform(world, &step.affordance, &step.frame) {
            Outcome::Committed => Beat::Committed,
            Outcome::Refused(_) => Beat::Refused,
        }
    });

    let smashed = smashed
        .borrow()
        .expect("a put was attempted and smashed a chest");
    let survivor = if smashed == chest_a { chest_b } else { chest_a };
    assert_eq!(progress, Progress::Achieved);
    assert_eq!(f.world.container_of(coin), Some(survivor)); // recovered onto the other
}

/// Build step 5, the arbiter closing the loop: two injected goals, the higher-
/// urgency one is committed to and pursued to completion through the same
/// `perform` a player hits, and then released. This is the whole hand-injected
/// autonomy loop (select, plan, execute, release) end to end, off any tick. The
/// world reflects the *selected* goal specifically: pursuing the loser would have
/// left the coin merely held, not stowed.
#[test]
fn the_arbiter_selects_a_goal_the_driver_carries_out() {
    use crate::agency::{RefWorldModel, known_here, perform, put as put_aff, take as take_aff};
    use crate::kinds::{Container, Item};
    use musce::agency::{
        Arbiter, Beat, Clause, Driver, Goal, Planner, Predicate, Progress, Term, UnitCost,
    };

    let mut f = fixture();
    let chest = spawn(&mut f.world, |b| {
        b.add(Container);
        b.add(Description("a wooden chest".into()));
    });
    f.world.move_entity(chest, f.hall).unwrap();
    let coin = spawn(&mut f.world, |b| {
        b.add(Item);
        b.add(Description("a gold coin".into()));
    });
    f.world.move_entity(coin, f.hall).unwrap();

    // The urgent goal: the coin ends up in the chest. The idle one: merely hold some
    // item. Pursuing the idle goal would stop at "held"; only the urgent one stows.
    let stow = Goal {
        predicate: Clause(vec![
            Predicate::Related {
                a: Term::Const(coin),
                b: Term::Const(chest),
                kind: "contained_by".into(),
            }
            .into(),
        ]),
        urgency: 8,
    };
    let hold = Goal {
        predicate: Clause(vec![
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
        ]),
        urgency: 3,
    };

    let mut arbiter = Arbiter::new(0);
    let chosen = arbiter
        .select(&[hold, stow.clone()])
        .expect("a goal is chosen");
    assert_eq!(chosen.predicate, stow.predicate); // the urgent one wins

    let table = [take_aff(), put_aff()];
    let known = known_here(&f.world, f.actor);
    let planner = Planner::new(&table, &RefWorldModel, &UnitCost);
    let driver = Driver::new(&planner);

    let progress = driver.pursue(
        f.actor,
        &chosen.predicate,
        &known,
        &mut f.world,
        |world, step| match perform(world, &step.affordance, &step.frame) {
            Outcome::Committed => Beat::Committed,
            Outcome::Refused(_) => Beat::Refused,
        },
    );

    assert_eq!(progress, Progress::Achieved);
    assert_eq!(f.world.container_of(coin), Some(chest)); // the selected goal, achieved
    arbiter.release();
    assert!(arbiter.committed().is_none());
}
