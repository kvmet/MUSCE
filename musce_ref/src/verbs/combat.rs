//! Combat: the `attack` verb and the two stat components it is the first consumer
//! of. `Special` is the character's stat block; `Health` is what a blow spends.
//! Both are game vocabulary (the engine reasons about none of it), registered
//! through `Game.register` like `Wander` and the kind markers, and both land here
//! *with* their consumer rather than as inert schema: `attack` reads the
//! attacker's Strength and drains the target's Health, and a killing blow routes
//! through `execute(Destroy)` so the same `Fact::Destroyed` reaction that narrates
//! any demise (`death_cry`) narrates a kill. See
//! `docs/architecture/engine-and-game.md` (the component boundary),
//! `docs/architecture/actions.md` (`execute`), and `docs/architecture/facts.md`
//! (the `Fact::Destroyed` reaction channel).

use musce::action::{Action, Ctx};
use musce::wire::EventKind;
use musce::world::{NamedComponent, World};
use serde::{Deserialize, Serialize};

use crate::commit_or_log;
use crate::names::{self, Scope, display_name};

/// The canonical seven-stat block, taken as one whole even though combat reads
/// only Strength today: the tuple is the character sheet, and future consumers
/// (perception checks, agility dodges) read the same struct rather than each
/// bolting on a field. Stamina and Mana are deliberately *not* here; they are
/// their own components, added with their own consumers (sprinting, spells), so a
/// thing that has neither does not carry them.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub(crate) struct Special {
    pub strength: u8,
    pub perception: u8,
    pub endurance: u8,
    pub charisma: u8,
    pub intelligence: u8,
    pub agility: u8,
    pub luck: u8,
}

impl NamedComponent for Special {
    const TAG: &'static str = "special";
}

/// A living thing's hit points. Having `Health` is what makes a thing fightable:
/// `attack` refuses anything without it, so rooms, items, and exits are inert.
/// `max` is carried from the start (healing and the Endurance-derived cap read it)
/// though nothing spends it up yet.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub(crate) struct Health {
    pub current: u16,
    pub max: u16,
}

impl NamedComponent for Health {
    const TAG: &'static str = "health";
}

/// `attack <target>` (also `kill`): strike a fightable thing in the room. Damage
/// is the attacker's Strength (a statless attacker still lands 1); the blow drains
/// the target's `Health`, and a blow that empties it destroys the target, which
/// the `death_cry` reaction then narrates from the `Fact` channel.
pub fn attack(ctx: &mut Ctx, args: &str) {
    let query = args.trim();
    if query.is_empty() {
        ctx.emit_self(EventKind::Feedback, "Attack what?");
        return;
    }
    // `resolve` never returns the actor, so there is no self-attack case to guard.
    let Some(target) = names::resolve(ctx.world, ctx.actor, Scope::Room, query) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You don't see \"{query}\" here."),
        );
        return;
    };
    if !ctx.world.has::<Health>(target) {
        ctx.emit_self(EventKind::Feedback, "You can't attack that.");
        return;
    }

    let dmg = u16::from(attacker_strength(ctx.world, ctx.actor).max(1));

    // Spend the damage against the target's Health, in a tight scope so the mutable
    // component borrow releases before the emits below reborrow the world. The
    // target has `Health` (checked above) and is live (just resolved), so both
    // lookups hold.
    let killed = {
        let e = ctx
            .world
            .index()
            .get(target)
            .expect("resolved target is live");
        let mut hp = ctx
            .world
            .ecs()
            .get::<&mut Health>(e)
            .expect("a fightable target has Health");
        hp.current = hp.current.saturating_sub(dmg);
        hp.current == 0
    };

    let who = display_name(ctx.world, ctx.actor);
    let them = display_name(ctx.world, target);
    let room = ctx.world.enclosing_locus(ctx.actor);

    if killed {
        ctx.emit_self(EventKind::Feedback, format!("You strike {them} down!"));
        if let Some(room) = room {
            ctx.emit_locus_except_self(
                room,
                EventKind::Narration,
                format!("{who} strikes {them} down."),
            );
        }
        // Destroy is infallible, so this cannot fail; a structural error would be a
        // bug, logged loud rather than silently swallowed. The emitted
        // `Fact::Destroyed` drives `death_cry`'s room narration the same tick.
        commit_or_log(
            ctx.world,
            Action::Destroy { entity: target },
            "attack: destroy the slain target",
        );
    } else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You hit {them} for {dmg} damage."),
        );
        ctx.emit_entity(
            target,
            EventKind::Narration,
            format!("{who} hits you for {dmg} damage."),
        );
        if let Some(room) = room {
            // Three-party like `wave at`: actor and target each read their own line,
            // so cut both from the room's bystander view.
            ctx.emit_locus_except(
                room,
                EventKind::Narration,
                format!("{who} hits {them}."),
                &[ctx.actor, target],
            );
        }
    }
}

/// The attacker's Strength, or 0 for a thing with no stat block (the caller floors
/// damage at 1, so a statless attacker still lands a blow).
fn attacker_strength(world: &World, actor: musce::world::EntityId) -> u8 {
    world
        .entity(actor)
        .and_then(|er| er.get::<&Special>().map(|s| s.strength))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::{Creature, Item, Player};
    use musce::action::{Audience, Ctx, Outbound, Verdict};
    use musce::wire::ConnectionId;
    use musce::world::hecs::EntityBuilder;
    use musce::world::{Description, EntityId, Fact, Locus, Name, World};

    struct Fixture {
        world: World,
        actor: EntityId,
        rat: EntityId,
        room: EntityId,
    }

    /// One room, a Strength-5 attacker, and a giant rat with 8 HP standing in it.
    fn fixture() -> Fixture {
        let mut world = World::new();

        let room = spawn(&mut world, |b| {
            b.add(Locus);
            b.add(Description("a bare room".into()));
        });
        let actor = spawn(&mut world, |b| {
            b.add(Player);
            b.add(Name("a fighter".into()));
            b.add(Special {
                strength: 5,
                ..Default::default()
            });
        });
        world.move_entity(actor, room).unwrap();
        let rat = spawn(&mut world, |b| {
            b.add(Creature);
            b.add(Name("a giant rat".into()));
            b.add(Health { current: 8, max: 8 });
        });
        world.move_entity(rat, room).unwrap();

        Fixture {
            world,
            actor,
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
        let mut out = Vec::new();
        let verdict = Verdict::guest();
        let mut ctx = Ctx::new(world, actor, ConnectionId(1), &verdict, &mut out);
        f(&mut ctx);
        out
    }

    fn hp(world: &World, e: EntityId) -> u16 {
        world
            .entity(e)
            .and_then(|er| er.get::<&Health>().map(|h| h.current))
            .expect("has Health")
    }

    fn feedback(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Connection(_)))
            .map(|o| o.event.text.clone())
            .collect()
    }

    #[test]
    fn a_hit_drains_health_and_narrates_three_ways() {
        let mut f = fixture();
        let out = run(&mut f.world, f.actor, |c| attack(c, "rat"));

        // Strength 5 off 8 HP leaves the rat alive at 3.
        assert_eq!(hp(&f.world, f.rat), 3);
        assert!(f.world.entity(f.rat).is_some());

        // Actor feedback, a line directed at the target, and a room line.
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t.contains("You hit a giant rat for 5"))
        );
        assert!(
            out.iter()
                .any(|o| matches!(o.event.to, Audience::Entity(e) if e == f.rat)
                    && o.event.text.contains("hits you for 5"))
        );
        assert!(
            out.iter()
                .any(|o| matches!(o.event.to, Audience::Locus(r) if r == f.room)
                    && o.event.text.contains("a fighter hits a giant rat"))
        );
    }

    #[test]
    fn a_lethal_blow_destroys_the_target_and_emits_the_death_fact() {
        let mut f = fixture();
        run(&mut f.world, f.actor, |c| attack(c, "rat")); // 8 -> 3
        run(&mut f.world, f.actor, |c| attack(c, "rat")); // 3 -> 0, dies

        // The rat is gone, and a Destroyed fact for it is on the channel for the
        // death_cry reaction to narrate.
        assert!(f.world.entity(f.rat).is_none());
        let facts = f.world.take_facts();
        assert!(
            facts.iter().any(|fact| matches!(
                fact,
                Fact::Destroyed { name: Some(n), .. } if n.contains("giant rat")
            )),
            "a lethal blow emits the death fact"
        );
    }

    #[test]
    fn a_thing_without_health_is_not_attackable() {
        let mut f = fixture();
        let key = spawn(&mut f.world, |b| {
            b.add(Item);
            b.add(Name("a brass key".into()));
        });
        f.world.move_entity(key, f.room).unwrap();
        let _ = f.world.take_facts(); // discard the setup move; the attack's facts are what matter

        let out = run(&mut f.world, f.actor, |c| attack(c, "key"));
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t.contains("can't attack that"))
        );
        // Nothing was destroyed.
        assert!(f.world.entity(key).is_some());
        assert!(f.world.take_facts().is_empty());
    }

    #[test]
    fn a_statless_attacker_still_lands_one_damage() {
        let mut f = fixture();
        // A brawler with no `Special`: the Strength floor of 1 still lands a blow,
        // so the damage formula never zeroes out.
        let brawler = spawn(&mut f.world, |b| {
            b.add(Player);
            b.add(Name("a brawler".into()));
        });
        f.world.move_entity(brawler, f.room).unwrap();

        let out = run(&mut f.world, brawler, |c| attack(c, "rat"));
        assert_eq!(hp(&f.world, f.rat), 7); // 8 - 1
        assert!(feedback(&out).iter().any(|t| t.contains("for 1 damage")));
    }
}
