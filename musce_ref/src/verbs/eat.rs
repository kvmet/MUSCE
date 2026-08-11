//! Eating command parsing. The canonical app affordance owns applicability,
//! mutation, advertised effects, and narration.

use musce::action::Ctx;
use musce::wire::EventKind;

use crate::names::{self, Scope};

/// `eat <food>`: eat a held edible thing. Resolves in inventory scope (you eat what
/// you carry), mirroring `drop`, which keeps the verb consistent with `eat`'s
/// held-precondition: a resolved item is already in hand, so the guard's held
/// literal is satisfied and only edibility can refuse.
pub fn eat(ctx: &mut Ctx, args: &str) {
    if args.trim().is_empty() {
        ctx.emit_self(EventKind::Feedback, "Eat what?");
        return;
    }
    let Some(target) = names::resolve(ctx.world, ctx.actor(), Scope::Inventory, args) else {
        ctx.emit_self(EventKind::Feedback, "You aren't carrying that.");
        return;
    };

    let action = crate::affordances::eat_action(ctx.actor(), target);
    crate::affordances::perform_command(ctx, &action, "You can't eat that.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consume::Fed;
    use crate::kinds::{Creature, Edible, Item};
    use musce::action::{PerformOutcome, Refusal, SystemCtx, Verdict};
    use musce::world::hecs::EntityBuilder;
    use musce::world::{Description, EntityId, Locus, Name, World};
    use std::time::SystemTime;

    struct Fixture {
        world: World,
        mouse: EntityId,
        crumb: EntityId,
    }

    /// A room, a creature, and a loose edible crumb in it.
    fn fixture() -> Fixture {
        let mut world = World::new();
        crate::systems::register(&mut world);

        let room = spawn(&mut world, |b| {
            b.add(Locus);
            b.add(Description("a pantry".into()));
        });
        let mouse = spawn(&mut world, |b| {
            b.add(Creature);
            b.add(Name("a field mouse".into()));
            b.add(Description("a small brown field mouse".into()));
        });
        world.move_entity(mouse, room).unwrap();
        let crumb = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Edible);
            b.add(Name("a crust of bread".into()));
            b.add(Description("a dry crust of bread".into()));
        });
        world.move_entity(crumb, room).unwrap();

        Fixture {
            world,
            mouse,
            crumb,
        }
    }

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    fn perform(world: &mut World, actor: EntityId, food: EntityId) -> PerformOutcome {
        let affordances =
            crate::affordances::build(world, &musce::action::CapRegistry::new()).unwrap();
        let mut out = Vec::new();
        let mut ctx = SystemCtx::new(
            world,
            &affordances,
            1,
            SystemTime::UNIX_EPOCH,
            &[],
            &mut out,
        );
        ctx.perform(
            &Verdict::guest(),
            &crate::affordances::eat_action(actor, food),
        )
        .unwrap()
    }

    #[test]
    fn eating_a_held_edible_sates_the_eater_and_spends_it() {
        let mut f = fixture();
        // Put the crumb in the mouse's hands, then eat it.
        f.world.move_entity(f.crumb, f.mouse).unwrap();

        let out = perform(&mut f.world, f.mouse, f.crumb);
        assert!(matches!(out, PerformOutcome::Committed(_)));
        assert!(f.world.has::<Fed>(f.mouse), "the eater is now fed");
        assert!(
            !f.world.has::<Edible>(f.crumb),
            "the crust is eaten down, no longer edible"
        );
        // Not destroyed: the spent crust remains in hand.
        assert_eq!(f.world.container_of(f.crumb), Some(f.mouse));
    }

    #[test]
    fn eating_an_unheld_thing_is_refused() {
        // The crust is on the floor, not held: the guard's held literal fails.
        let mut f = fixture();
        let out = perform(&mut f.world, f.mouse, f.crumb);
        assert!(matches!(
            out,
            PerformOutcome::Refused(Refusal::Guard { .. })
        ));
        assert!(!f.world.has::<Fed>(f.mouse));
        assert!(f.world.has::<Edible>(f.crumb)); // untouched
    }

    #[test]
    fn eating_a_held_inedible_thing_is_refused() {
        // A held pebble is not edible: the guard's edible literal fails.
        let mut f = fixture();
        let pebble = spawn(&mut f.world, |b| {
            b.add(Item);
            b.add(Name("a pebble".into()));
            b.add(Description("a smooth grey pebble".into()));
        });
        f.world.move_entity(pebble, f.mouse).unwrap();

        let out = perform(&mut f.world, f.mouse, pebble);
        assert!(matches!(
            out,
            PerformOutcome::Refused(Refusal::Guard { .. })
        ));
        assert!(!f.world.has::<Fed>(f.mouse));
    }

    /// The room hears a third-person line when a being eats; the eater reads its own.
    #[test]
    fn the_room_hears_the_eating() {
        use musce::action::{Audience, Caller, Outbound, Verdict};
        use musce::wire::ConnectionId;

        let mut f = fixture();
        f.world.move_entity(f.crumb, f.mouse).unwrap();

        let mut out: Vec<Outbound> = Vec::new();
        let verdict = Verdict::guest();
        let affordances =
            crate::affordances::build(&f.world, &musce::action::CapRegistry::new()).unwrap();
        let mut ctx = Ctx::new(
            &mut f.world,
            &affordances,
            Caller::new(f.mouse, ConnectionId(1), &verdict),
            &mut out,
        );
        eat(&mut ctx, "bread");

        let room_lines: Vec<String> = out
            .iter()
            .filter(|o| matches!(o.event.to, Audience::Locus(_)))
            .map(|o| o.event.text.clone())
            .collect();
        assert!(
            room_lines
                .iter()
                .any(|t| t.contains("a field mouse eats a crust of bread")),
            "eating narration, got: {room_lines:?}"
        );
    }
}
