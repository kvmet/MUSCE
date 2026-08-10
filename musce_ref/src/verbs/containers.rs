//! Containers: moving a held thing into a container (`put`) or into someone's
//! hands (`give`). Both are one `Move` under a game rule about what the
//! destination accepts, the same shape as `take`/`drop`: `put` accepts anything
//! marked `Container`; `give` is the reference app's first typed canonical
//! affordance. The rules are game policy, not part of `execute`. See
//! `docs/architecture/actions.md`.

use musce::action::{Action, Ctx, Frame, PerformOutcome, Refusal, execute};
use musce::wire::EventKind;
use musce::world::{EntityId, World};

use crate::agency::RefWorldModel;
use crate::names::{self, Scope};
use crate::verbs::Outcome;

/// Put `item` into `container`, subject to the put rule. The grounded action a
/// player's `put` verb and an agent's plan both resolve to, so a scripted put is
/// vetoed exactly as a typed one. `Ctx`-free and silent; the caller narrates.
pub(crate) fn do_put(
    world: &mut World,
    actor: EntityId,
    item: EntityId,
    container: EntityId,
) -> Outcome {
    // The gameplay veto (item held, destination is a container) is the `put`
    // affordance's guards, read through the same `RefWorldModel` the planner
    // reads, so verb and plan cannot disagree on what `put` permits.
    let frame = Frame {
        actor,
        object: Some(item),
        target: Some(container),
    };
    if let Some(guard) = crate::agency::put().veto(&frame, world, &RefWorldModel) {
        return Outcome::Refused(guard.reason);
    }
    // Putting a held container into itself would cycle; the executor rejects it,
    // and "you can't put that there" is the right thing for the player to hear.
    match execute(
        world,
        Action::Move {
            entity: item,
            into: container,
        },
    ) {
        Ok(_) => Outcome::Committed,
        Err(_) => Outcome::Refused("You can't put that there."),
    }
}

/// `put <item> in <container>`: move a held thing into a container in reach. The
/// item comes from the actor's hands; the container may be held (a pack) or on the
/// floor. Anything marked `Container` accepts contents; anything else is refused.
pub fn put(ctx: &mut Ctx, args: &str) {
    let Some((item_query, container_query)) = split(args, " in ") else {
        ctx.emit_self(
            EventKind::Feedback,
            "Put what where? (put <item> in <container>)",
        );
        return;
    };

    let Some(item) = names::resolve(ctx.world, ctx.actor(), Scope::Inventory, item_query) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You aren't carrying \"{item_query}\"."),
        );
        return;
    };
    let Some(container) = reachable(ctx.world, ctx.actor(), container_query) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You don't see \"{container_query}\" here."),
        );
        return;
    };

    let actor = ctx.actor();
    let frame = Frame {
        actor,
        object: Some(item),
        target: Some(container),
    };
    let (world, out) = ctx.world_and_out();
    crate::act::perform_narrated(world, actor, &crate::agency::put(), &frame, out);
}

/// `give <item> to <someone>`: hand a held thing to a being in the room.
/// Three-party like `wave at`: the actor, the recipient, and the rest of the room
/// each read their own line. The canonical guards decide whether the selected
/// recipient accepts gifts; to stash a thing in an object, use `put`.
pub fn give(ctx: &mut Ctx, args: &str) {
    let Some((item_query, who_query)) = split(args, " to ") else {
        ctx.emit_self(
            EventKind::Feedback,
            "Give what to whom? (give <item> to <someone>)",
        );
        return;
    };

    let Some(item) = names::resolve(ctx.world, ctx.actor(), Scope::Inventory, item_query) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You aren't carrying \"{item_query}\"."),
        );
        return;
    };
    let Some(recipient) = names::resolve(ctx.world, ctx.actor(), Scope::Room, who_query) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You don't see \"{who_query}\" here."),
        );
        return;
    };
    let action = crate::affordances::give_action(ctx.actor(), item, recipient);
    match ctx.perform(&action) {
        Ok(PerformOutcome::Committed(_)) => {}
        Ok(PerformOutcome::Refused(Refusal::Guard { reason, .. }))
        | Ok(PerformOutcome::Refused(Refusal::Resolution { reason })) => {
            ctx.emit_self(EventKind::Feedback, reason);
        }
        Ok(PerformOutcome::Refused(Refusal::Gate)) => {
            ctx.emit_self(EventKind::Feedback, "You aren't allowed to do that.");
        }
        Err(error) => {
            tracing::error!(error = %error, "canonical give failed");
            ctx.emit_self(EventKind::Feedback, "You can't give that away.");
        }
    }
}

/// Split `args` on the first occurrence of `sep` into two trimmed, non-empty
/// halves. `None` if the separator is absent or either half is blank, which the
/// callers turn into a usage prompt.
fn split<'a>(args: &'a str, sep: &str) -> Option<(&'a str, &'a str)> {
    let (left, right) = args.split_once(sep)?;
    let (left, right) = (left.trim(), right.trim());
    (!left.is_empty() && !right.is_empty()).then_some((left, right))
}

/// Resolve a thing the actor can reach to put something into: a container held in
/// hand wins over one on the floor, matching the inventory-first order the rest of
/// the resolver uses.
fn reachable(world: &World, actor: EntityId, query: &str) -> Option<EntityId> {
    names::resolve(world, actor, Scope::Inventory, query)
        .or_else(|| names::resolve(world, actor, Scope::Room, query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::{Container, Creature, GiftRecipient, Item, Player};
    use musce::action::schema::{AffordanceId, Effect, Resolution, Term};
    use musce::action::{Audience, Ctx, Outbound, PerformError, PerformOutcome, Refusal};
    use musce::wire::ConnectionId;
    use musce::world::hecs::EntityBuilder;
    use musce::world::{Description, EntityId, Locus, Name, World};

    struct Fixture {
        world: World,
        actor: EntityId,
        coin: EntityId,
        chest: EntityId,
        rat: EntityId,
        room: EntityId,
    }

    /// A room holding the actor (carrying a coin), a chest (a `Container`), and a
    /// giant rat (a `Creature`, so a valid gift recipient). Components are
    /// registered as at boot, so `put`'s guard can read the container tag by name
    /// through `RefWorldModel`.
    fn fixture() -> Fixture {
        let mut world = World::new();
        crate::systems::register(&mut world);
        let room = spawn(&mut world, |b| {
            b.add(Locus);
            b.add(Description("a bare room".into()));
        });
        let actor = spawn(&mut world, |b| {
            b.add(Player);
            b.add(GiftRecipient);
            b.add(Name("a fighter".into()));
        });
        world.move_entity(actor, room).unwrap();
        let coin = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Name("a copper coin".into()));
        });
        world.move_entity(coin, actor).unwrap();
        let chest = spawn(&mut world, |b| {
            b.add(Container);
            b.add(Name("a wooden chest".into()));
        });
        world.move_entity(chest, room).unwrap();
        let rat = spawn(&mut world, |b| {
            b.add(Creature);
            b.add(GiftRecipient);
            b.add(Name("a giant rat".into()));
        });
        world.move_entity(rat, room).unwrap();

        Fixture {
            world,
            actor,
            coin,
            chest,
            rat,
            room,
        }
    }

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    fn run(world: &mut World, actor: EntityId, f: impl FnOnce(&mut Ctx)) -> Vec<Outbound> {
        use musce::action::{Caller, Verdict};

        let mut out = Vec::new();
        let verdict = Verdict::guest();
        let caps = musce::action::CapRegistry::new();
        let affordances = crate::affordances::build(world, &caps).unwrap();
        let mut ctx = Ctx::new(
            world,
            &affordances,
            Caller::new(actor, ConnectionId(1), &verdict),
            &mut out,
        );
        f(&mut ctx);
        out
    }

    fn perform_give(
        world: &mut World,
        actor: EntityId,
        item: EntityId,
        recipient: EntityId,
    ) -> (Result<PerformOutcome, PerformError>, Vec<Outbound>) {
        use musce::action::{Caller, Verdict};

        let caps = musce::action::CapRegistry::new();
        let affordances = crate::affordances::build(world, &caps).unwrap();
        let verdict = Verdict::guest();
        let mut out = Vec::new();
        let result = {
            let mut ctx = Ctx::new(
                world,
                &affordances,
                Caller::new(actor, ConnectionId(1), &verdict),
                &mut out,
            );
            ctx.perform(&crate::affordances::give_action(actor, item, recipient))
        };
        (result, out)
    }

    // Directed lines, whether connection- or entity-addressed: `put` narrates its
    // first person to the actor entity through the shared narrator, while `give`
    // still emits its own connection-addressed feedback.
    fn feedback(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Connection(_) | Audience::Entity(_)))
            .map(|o| o.event.text.clone())
            .collect()
    }

    #[test]
    fn put_moves_a_held_item_into_the_container() {
        let mut f = fixture();
        let out = run(&mut f.world, f.actor, |c| put(c, "coin in chest"));

        assert_eq!(f.world.container_of(f.coin), Some(f.chest));
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t == "You put a copper coin in a wooden chest.")
        );
        assert!(out.iter().any(|o| {
            matches!(o.event.to, Audience::Locus(r) if r == f.room)
                && o.event
                    .text
                    .contains("a fighter puts a copper coin in a wooden chest")
        }));
    }

    #[test]
    fn put_refuses_dropping_a_held_container_into_itself() {
        let mut f = fixture();
        // A container the actor holds: putting it into itself would close a
        // containment cycle, the one reachable structural refusal `put` guards (and
        // the reason it commits through `execute` rather than `commit_or_log`).
        let bag = spawn(&mut f.world, |b| {
            b.add(Container);
            b.add(Name("a leather bag".into()));
        });
        f.world.move_entity(bag, f.actor).unwrap();

        let out = run(&mut f.world, f.actor, |c| put(c, "bag in bag"));
        // The bag is unmoved and the player hears the structural refusal.
        assert_eq!(f.world.container_of(bag), Some(f.actor));
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t.contains("can't put that there"))
        );
    }

    #[test]
    fn put_refuses_a_non_container_destination() {
        let mut f = fixture();
        // The rat is a being, not a container.
        let out = run(&mut f.world, f.actor, |c| put(c, "coin in rat"));
        assert_eq!(f.world.container_of(f.coin), Some(f.actor));
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t.contains("can't put things in that"))
        );
    }

    #[test]
    fn put_refuses_an_item_the_actor_is_not_holding() {
        let mut f = fixture();
        let out = run(&mut f.world, f.actor, |c| put(c, "chest in chest"));
        // The chest is on the floor, not held, so there is nothing to put.
        assert!(feedback(&out).iter().any(|t| t.contains("aren't carrying")));
    }

    #[test]
    fn give_hands_an_item_to_a_being_and_narrates_three_ways() {
        let mut f = fixture();
        let out = run(&mut f.world, f.actor, |c| give(c, "coin to rat"));

        assert_eq!(f.world.container_of(f.coin), Some(f.rat));
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t == "You give a copper coin to a giant rat.")
        );
        assert!(
            out.iter()
                .any(|o| matches!(o.event.to, Audience::Entity(e) if e == f.rat)
                    && o.event.text.contains("a fighter gives you a copper coin"))
        );
        assert!(out.iter().any(|o| {
            matches!(o.event.to, Audience::Locus(r) if r == f.room)
                && o.event
                    .text
                    .contains("a fighter gives a copper coin to a giant rat")
        }));
    }

    #[test]
    fn give_refuses_an_entity_without_the_recipient_capability() {
        let mut f = fixture();
        // The chest is a container, not someone who can be handed a thing.
        let out = run(&mut f.world, f.actor, |c| give(c, "coin to chest"));
        assert_eq!(f.world.container_of(f.coin), Some(f.actor));
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t.contains("can't give things to that"))
        );
    }

    #[test]
    fn give_schema_advertises_the_relation_it_commits() {
        let f = fixture();
        let caps = musce::action::CapRegistry::new();
        let registry = crate::affordances::build(&f.world, &caps).unwrap();
        let schema = registry
            .schema(&AffordanceId::new(crate::affordances::GIVE).unwrap())
            .unwrap();

        assert_eq!(schema.resolution(), Resolution::Contested);
        assert!(matches!(
            schema.effects(),
            [Effect::SetRelation {
                source: Term::Input(item),
                relation,
                target: Term::Input(recipient),
            }] if item.as_str() == "item"
                && relation.as_str() == "contained_by"
                && recipient.as_str() == "recipient"
        ));
    }

    #[test]
    fn canonical_give_guards_held_recipient_kind_and_shared_locus() {
        let mut f = fixture();
        f.world.move_entity(f.coin, f.room).unwrap();
        let (not_held, _) = perform_give(&mut f.world, f.actor, f.coin, f.rat);
        assert!(matches!(
            not_held,
            Ok(PerformOutcome::Refused(Refusal::Guard { index: 0, .. }))
        ));

        f.world.move_entity(f.coin, f.actor).unwrap();
        let (not_recipient, _) = perform_give(&mut f.world, f.actor, f.coin, f.chest);
        assert!(matches!(
            not_recipient,
            Ok(PerformOutcome::Refused(Refusal::Guard { index: 1, .. }))
        ));

        let elsewhere = spawn(&mut f.world, |b| {
            b.add(Locus);
            b.add(Description("another room".into()));
        });
        f.world.move_entity(f.rat, elsewhere).unwrap();
        let (not_here, _) = perform_give(&mut f.world, f.actor, f.coin, f.rat);
        assert!(matches!(
            not_here,
            Ok(PerformOutcome::Refused(Refusal::Guard { index: 3, .. }))
        ));
    }

    #[test]
    fn canonical_give_declares_the_reachable_cycle_as_contested() {
        let mut f = fixture();
        f.world.move_entity(f.rat, f.coin).unwrap();

        let (result, out) = perform_give(&mut f.world, f.actor, f.coin, f.rat);

        assert!(matches!(
            result,
            Ok(PerformOutcome::Refused(Refusal::Resolution { .. }))
        ));
        assert_eq!(f.world.container_of(f.coin), Some(f.actor));
        assert!(out.is_empty(), "a refused mutation must not narrate");
    }

    #[test]
    fn a_missing_separator_prompts_for_usage() {
        let mut f = fixture();
        let put_out = run(&mut f.world, f.actor, |c| put(c, "coin"));
        assert!(
            feedback(&put_out)
                .iter()
                .any(|t| t.contains("Put what where?"))
        );
        let give_out = run(&mut f.world, f.actor, |c| give(c, "coin"));
        assert!(
            feedback(&give_out)
                .iter()
                .any(|t| t.contains("Give what to whom?"))
        );
    }

    // Guard/handler agreement: does an affordance's `veto`, read through
    // `RefWorldModel`, produce the same permit/refuse (and the same message) the
    // imperative handler produces? These pin what the current predicate vocabulary
    // can and cannot express as a gameplay veto, and that `put`'s handler now
    // shares its container check with the affordance the planner reads.

    /// `put`'s veto is two guards the vocabulary expresses in full (item held AND
    /// destination is a container), each carrying the message the handler shows.
    /// The `veto` agrees with the handler on every gameplay case.
    #[test]
    fn put_guards_predict_the_gameplay_veto() {
        use crate::agency::{RefWorldModel, put as put_aff};
        use musce::agency::Frame;

        // Held item + container destination: no veto, and it commits.
        {
            let mut f = fixture();
            let frame = Frame {
                actor: f.actor,
                object: Some(f.coin),
                target: Some(f.chest),
            };
            let veto = put_aff()
                .veto(&frame, &f.world, &RefWorldModel)
                .map(|g| g.reason);
            run(&mut f.world, f.actor, |c| put(c, "coin in chest"));
            let committed = f.world.container_of(f.coin) == Some(f.chest);
            assert!(
                veto.is_none() && committed,
                "veto {veto:?}, committed {committed}"
            );
        }
        // Held item + non-container destination (a creature): the container guard
        // vetoes with exactly the message the handler shows, and nothing commits.
        {
            let mut f = fixture();
            let frame = Frame {
                actor: f.actor,
                object: Some(f.coin),
                target: Some(f.rat),
            };
            let veto = put_aff()
                .veto(&frame, &f.world, &RefWorldModel)
                .map(|g| g.reason);
            let out = run(&mut f.world, f.actor, |c| put(c, "coin in rat"));
            let committed = f.world.container_of(f.coin) == Some(f.rat);
            assert_eq!(veto, Some("You can't put things in that."));
            assert!(!committed);
            assert!(
                feedback(&out)
                    .iter()
                    .any(|t| t == "You can't put things in that.")
            );
        }
        // Item not held (a container on the floor): the held guard vetoes first.
        {
            let f = fixture();
            let frame = Frame {
                actor: f.actor,
                object: Some(f.chest),
                target: Some(f.chest),
            };
            let veto = put_aff()
                .veto(&frame, &f.world, &RefWorldModel)
                .map(|g| g.reason);
            assert_eq!(veto, Some("You aren't carrying that."));
        }
    }

    /// The one refusal a guard should NOT capture: the containment cycle is a
    /// structural invariant the executor re-checks at commit, not a gameplay rule.
    /// The guards permit, the handler refuses, and that divergence is the boundary
    /// between what belongs in a guard and what does not.
    #[test]
    fn put_guards_do_not_capture_the_structural_cycle() {
        use crate::agency::{RefWorldModel, put as put_aff};
        use musce::agency::Frame;

        let mut f = fixture();
        let bag = spawn(&mut f.world, |b| {
            b.add(Container);
            b.add(Name("a leather bag".into()));
        });
        f.world.move_entity(bag, f.actor).unwrap();

        let frame = Frame {
            actor: f.actor,
            object: Some(bag),
            target: Some(bag),
        };
        let veto = put_aff()
            .veto(&frame, &f.world, &RefWorldModel)
            .map(|g| g.reason);
        run(&mut f.world, f.actor, |c| put(c, "bag in bag"));
        let committed = f.world.container_of(bag) == Some(bag);

        assert!(
            veto.is_none(),
            "bag is held and is a container: guards permit"
        );
        assert!(!committed, "the cycle is a structural refusal at commit");
    }

    /// `drop`'s veto (the item is held) is a single relation guard, fully
    /// expressible; the `veto` agrees with the handler.
    #[test]
    fn drop_guard_predicts_the_veto() {
        use crate::agency::{RefWorldModel, drop as drop_aff};
        use crate::verbs::drop;
        use musce::agency::Frame;

        // Held item: no veto, and it lands in the room.
        {
            let mut f = fixture();
            let frame = Frame {
                actor: f.actor,
                object: Some(f.coin),
                target: Some(f.room),
            };
            let veto = drop_aff()
                .veto(&frame, &f.world, &RefWorldModel)
                .map(|g| g.reason);
            run(&mut f.world, f.actor, |c| drop(c, "coin"));
            let committed = f.world.container_of(f.coin) == Some(f.room);
            assert!(
                veto.is_none() && committed,
                "veto {veto:?}, committed {committed}"
            );
        }
        // Item on the floor, not held: the held guard vetoes.
        {
            let f = fixture();
            let frame = Frame {
                actor: f.actor,
                object: Some(f.chest),
                target: Some(f.room),
            };
            let veto = drop_aff()
                .veto(&frame, &f.world, &RefWorldModel)
                .map(|g| g.reason);
            assert_eq!(veto, Some("You aren't carrying that."));
        }
    }
}
