//! Containers: moving a held thing into a container (`put`) or into someone's
//! hands (`give`). Both are one `Move` under a game rule about what the
//! destination accepts, the same shape as `take`/`drop`: `put` accepts anything
//! marked `Container`, `give` accepts a being (a `Player` or `Creature`). The
//! rule is game policy, kept in the handler, not in `execute`. See
//! `docs/architecture/actions.md`.

use musce_action::{Action, Ctx, execute};
use musce_core::{EntityId, World};
use musce_proto::EventKind;

use crate::commit_or_log;
use crate::kinds::{Container, Creature, Player};
use crate::names::{self, Scope, display_name};

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

    let Some(item) = names::resolve(ctx.world, ctx.actor, Scope::Inventory, item_query) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You aren't carrying \"{item_query}\"."),
        );
        return;
    };
    let Some(container) = reachable(ctx.world, ctx.actor, container_query) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You don't see \"{container_query}\" here."),
        );
        return;
    };
    if !ctx.world.has::<Container>(container) {
        ctx.emit_self(EventKind::Feedback, "You can't put things in that.");
        return;
    }

    let item_name = display_name(ctx.world, item);
    let container_name = display_name(ctx.world, container);
    let who = display_name(ctx.world, ctx.actor);
    let room = ctx.world.enclosing_room(ctx.actor);

    // Putting a held container into itself would cycle; the executor rejects it and
    // "you can't put that there" is the right thing for the player to hear.
    if execute(
        ctx.world,
        Action::Move {
            entity: item,
            into: container,
        },
    )
    .is_err()
    {
        ctx.emit_self(EventKind::Feedback, "You can't put that there.");
        return;
    }

    ctx.emit_self(
        EventKind::Feedback,
        format!("You put {item_name} in {container_name}."),
    );
    if let Some(room) = room {
        ctx.emit_room_except_self(
            room,
            EventKind::Narration,
            format!("{who} puts {item_name} in {container_name}."),
        );
    }
}

/// `give <item> to <someone>`: hand a held thing to a being in the room.
/// Three-party like `wave at`: the actor, the recipient, and the rest of the room
/// each read their own line. Only a `Player` or `Creature` accepts a gift; to
/// stash a thing in an object, use `put`.
pub fn give(ctx: &mut Ctx, args: &str) {
    let Some((item_query, who_query)) = split(args, " to ") else {
        ctx.emit_self(
            EventKind::Feedback,
            "Give what to whom? (give <item> to <someone>)",
        );
        return;
    };

    let Some(item) = names::resolve(ctx.world, ctx.actor, Scope::Inventory, item_query) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You aren't carrying \"{item_query}\"."),
        );
        return;
    };
    let Some(recipient) = names::resolve(ctx.world, ctx.actor, Scope::Room, who_query) else {
        ctx.emit_self(
            EventKind::Feedback,
            format!("You don't see \"{who_query}\" here."),
        );
        return;
    };
    if !is_being(ctx.world, recipient) {
        ctx.emit_self(EventKind::Feedback, "You can't give things to that.");
        return;
    }

    let item_name = display_name(ctx.world, item);
    let them = display_name(ctx.world, recipient);
    let who = display_name(ctx.world, ctx.actor);
    let room = ctx.world.enclosing_room(ctx.actor);

    // The recipient is a being in the room, not something the held item contains,
    // so this move cannot cycle and has no reachable structural failure; a bug here
    // is logged loud rather than silently shown to the player as a refusal.
    if !commit_or_log(
        ctx.world,
        Action::Move {
            entity: item,
            into: recipient,
        },
        "give: move held item into the recipient",
    ) {
        ctx.emit_self(EventKind::Feedback, "You can't give that away.");
        return;
    }

    ctx.emit_self(
        EventKind::Feedback,
        format!("You give {item_name} to {them}."),
    );
    ctx.emit_entity(
        recipient,
        EventKind::Narration,
        format!("{who} gives you {item_name}."),
    );
    if let Some(room) = room {
        ctx.emit_room_except(
            room,
            EventKind::Narration,
            format!("{who} gives {item_name} to {them}."),
            &[ctx.actor, recipient],
        );
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

/// A being that can be handed a thing: a player avatar or a creature. Rooms,
/// items, and containers are not recipients (`put` covers stashing into objects).
fn is_being(world: &World, entity: EntityId) -> bool {
    world.has::<Player>(entity) || world.has::<Creature>(entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::{Container, Creature, Item, Player};
    use musce_action::{Ctx, Outbound, Verdict};
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Description, EntityId, Name, Room, World};
    use musce_proto::{Audience, ConnectionId};

    struct Fixture {
        world: World,
        actor: EntityId,
        coin: EntityId,
        chest: EntityId,
        rat: EntityId,
        room: EntityId,
    }

    /// A room holding the actor (carrying a coin), a chest (a `Container`), and a
    /// giant rat (a `Creature`, so a valid gift recipient).
    fn fixture() -> Fixture {
        let mut world = World::new();
        let room = spawn(&mut world, |b| {
            b.add(Room);
            b.add(Description("a bare room".into()));
        });
        let actor = spawn(&mut world, |b| {
            b.add(Player);
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
        let mut out = Vec::new();
        let verdict = Verdict::guest();
        let mut ctx = Ctx::new(world, actor, ConnectionId(1), &verdict, &mut out);
        f(&mut ctx);
        out
    }

    fn feedback(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Connection(_)))
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
            matches!(o.event.to, Audience::Room(r) if r == f.room)
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
            matches!(o.event.to, Audience::Room(r) if r == f.room)
                && o.event
                    .text
                    .contains("a fighter gives a copper coin to a giant rat")
        }));
    }

    #[test]
    fn give_refuses_a_non_being_recipient() {
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
}
