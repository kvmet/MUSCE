//! The pointing web client's App seams: two reads and one act. [`snapshot`]
//! projects the world into a wire containment tree and [`offers`] a clicked entity
//! into its affordance list, both as `musce_proto` DTOs the read replies carry;
//! [`perform`] is the act, running a clicked affordance on already-bound entities.
//! These are the game side of the `App.snapshot`/`App.offers`/`App.perform`
//! seams: the engine routes to them and (for the reads) serializes the result,
//! holding no game vocabulary itself. Names, kinds, the affordance set, and which
//! role a clicked entity fills are all game knowledge, which is why they live here.
//!
//! Perception is the MVP rule the rest of the reference game already uses: an actor
//! sees its enclosing room and everything nested within (co-located implies known,
//! the same seed as `crate::agency::known_here`). A closed-container or
//! line-of-sight refinement would narrow `collect` here without touching the wire.
//!
//! See `docs/architecture/networking-and-sessions.md` and
//! `docs/architecture/offers.md`.

use musce::action::{Ctx, Verdict};
use musce::agency::Frame;
use musce::wire::{Entity, EventKind, Offer, OfferStatus, Role, SnapshotData};
use musce::world::{Description, EntityId, Locus, World};

use crate::exits::ExitQueries;
use crate::kinds::{Container, Creature, Edible, Exit, Item, Player};
use crate::offers::{self, affordances_on};
use crate::verbs::Locked;

/// Project the perceivable containment tree for `actor`: rooted at its enclosing
/// room, every entity nested within (including, as the actor's own contents, its
/// inventory). The wire snapshot the `Query::Snapshot` reply carries.
pub fn snapshot(world: &World, actor: EntityId) -> SnapshotData {
    let root = world.enclosing_locus(actor).unwrap_or(actor);
    let mut entities = Vec::new();
    collect(world, root, &mut entities);
    SnapshotData {
        root: root.0.to_string(),
        actor: actor.0.to_string(),
        entities,
    }
}

/// Walk containment depth-first from `id`, pushing one [`Entity`] per node.
/// Containment is acyclic, so this terminates without a visited set.
fn collect(world: &World, id: EntityId, out: &mut Vec<Entity>) {
    let mut contents = world.contents(id);
    // A locus's exits are relation-backed (`LeadsFrom`), not containment children,
    // so raw containment misses them. The pointing client has no `go` box to type
    // into: it reaches an exit by clicking it, so project the room's exits as nodes
    // under it. This is the one place the client's tree diverges from containment.
    if world.has::<Locus>(id) {
        contents.extend(world.exits_of(id));
    }
    out.push(Entity {
        id: id.0.to_string(),
        name: world.name_of(id).unwrap_or_else(|| "something".into()),
        kinds: kinds_of(world, id),
        contents: contents.iter().map(|c| c.0.to_string()).collect(),
        details: details_of(world, id),
    });
    for child in contents {
        collect(world, child, out);
    }
}

/// The passive detail an actor perceives about an entity by presence, as ordered
/// `(label, value)` pairs. App vocabulary, like `kinds_of`: the reference game
/// exposes an entity's `Description`, the same prose a narrated `examine` reveals,
/// delivered silently so a click renders without a second round-trip.
fn details_of(world: &World, id: EntityId) -> Vec<(String, String)> {
    let mut details = Vec::new();
    if let Some(desc) = world.get::<Description>(id) {
        details.push(("description".to_string(), desc.0.clone()));
    }
    details
}

/// The game kind tags on an entity, the same vocabulary a text client's prose
/// implies. Probed by the game's own kind markers; the engine has no notion of what
/// a "container" is.
fn kinds_of(world: &World, id: EntityId) -> Vec<String> {
    let mut kinds = Vec::new();
    for (present, tag) in [
        (world.has::<Locus>(id), "locus"),
        (world.has::<Player>(id), "player"),
        (world.has::<Creature>(id), "creature"),
        (world.has::<Container>(id), "container"),
        (world.has::<Item>(id), "item"),
        (world.has::<Exit>(id), "exit"),
        (world.has::<Edible>(id), "edible"),
        (world.has::<Locked>(id), "locked"),
    ] {
        if present {
            kinds.push(tag.to_string());
        }
    }
    kinds
}

/// The affordances available on `clicked` for `actor`, in wire form. Delegates the
/// classification to [`affordances_on`] and maps its statuses to the serde DTOs.
pub fn offers(world: &World, actor: EntityId, clicked: EntityId) -> Vec<Offer> {
    affordances_on(world, actor, clicked)
        .into_iter()
        .map(|o| Offer {
            name: o.name,
            status: to_wire(o.status),
        })
        .collect()
}

fn to_wire(status: offers::OfferStatus) -> OfferStatus {
    match status {
        offers::OfferStatus::Available => OfferStatus::Available,
        offers::OfferStatus::Vetoed(reason) => OfferStatus::Vetoed {
            reason: reason.to_string(),
        },
        offers::OfferStatus::NeedsRole(role) => OfferStatus::NeedsRole {
            role: to_wire_role(role),
        },
    }
}

fn to_wire_role(role: offers::Role) -> Role {
    match role {
        offers::Role::Object => Role::Object,
        offers::Role::Target => Role::Target,
    }
}

/// Perform a clicked affordance for `actor`, entities already bound: `focus` is the
/// clicked entity and `with` an optional sub-pick, mapped onto the affordance's
/// roles by the same [`focus_role`](offers::focus_role) convention enumeration
/// uses. The click supplies the ground the name resolver would otherwise recover;
/// beyond that it is an ordinary act, so it routes through the shared
/// [`crate::act::perform_narrated`], narrating to the actor and the room exactly as
/// the typed verb does. A co-located text player reads the third-person line at
/// once, not on their next snapshot.
///
/// The third pointing seam, but an act, not a read: it mutates and narrates. See
/// `docs/architecture/networking-and-sessions.md`.
pub fn perform(
    ctx: &mut Ctx,
    verdict: &Verdict,
    name: &str,
    focus: EntityId,
    with: Option<EntityId>,
) {
    // The click carries raw ids, so gate every supplied entity through the actor's
    // perceivable set before grounding. Without this the click path would be
    // strictly more powerful than the typed verbs, whose name resolution is
    // locus-scoped: a client could act on any entity by guessing its id.
    if !perceivable(ctx.world, ctx.actor, focus)
        || with.is_some_and(|w| !perceivable(ctx.world, ctx.actor, w))
    {
        ctx.emit_self(EventKind::Feedback, "You don't see that here.");
        return;
    }
    let (object, target) = match offers::focus_role(name) {
        offers::Role::Object => (Some(focus), with),
        offers::Role::Target => (with, Some(focus)),
    };
    // Perception spans the whole locus subtree, but manipulation does not: an object
    // must be held or lie loose in the room. Without this a click could take an item
    // out of another creature's inventory, which the text path's room-scoped name
    // resolution never allows. Targets (a container, an exit) are constrained by
    // their own guards and the perception gate, not this reachability rule.
    if object.is_some_and(|o| !offers::reachable(ctx.world, ctx.actor, o)) {
        ctx.emit_self(EventKind::Feedback, "You can't reach that.");
        return;
    }
    let Some(affordance) = offers::affordance_named(name) else {
        ctx.emit_self(EventKind::Feedback, "You can't do that.");
        return;
    };
    let frame = Frame {
        actor: ctx.actor,
        object,
        target,
        kind: None,
    };
    // The client is the enumerator now and can omit a role the affordance needs (a
    // `put` with no object sub-picked). Refuse cleanly here rather than routing an
    // under-bound frame into `agency::perform`, whose `bad_frame` invariant assumes a
    // complete frame and would `debug_assert!` (a debug panic, a wrong release message).
    if offers::required_roles(&affordance)
        .into_iter()
        .any(|role| !offers::filled(&frame, role))
    {
        ctx.emit_self(EventKind::Feedback, "You need to choose something first.");
        return;
    }
    let actor = ctx.actor;
    let (world, out) = ctx.world_and_out();
    crate::act::perform_narrated(world, actor, &affordance, &frame, verdict, out);
}

/// Whether `actor` can perceive `id`: it shares the actor's enclosing locus, the
/// exact subtree [`snapshot`] roots at and exposes to the client (room contents,
/// nested container contents, and the actor's own inventory all resolve to that
/// locus), or it is an exit out of that locus (relation-backed, so outside the
/// containment subtree but still rendered and clickable). The MVP perception rule,
/// matching `crate::agency::known_here` plus the exit projection; a location-less
/// actor perceives nothing. Assumes loci do not nest, as the reference world model
/// holds.
fn perceivable(world: &World, actor: EntityId, id: EntityId) -> bool {
    match world.enclosing_locus(actor) {
        Some(locus) => {
            world.enclosing_locus(id) == Some(locus) || world.exits_of(locus).contains(&id)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce::action::{Audience, Outbound};
    use musce::wire::ConnectionId;
    use musce::world::hecs::EntityBuilder;
    use musce::world::{Description, Name};

    use crate::kinds::{Container, Creature, Item};

    /// Run a Ctx closure and return its emitted (pre-resolution) outbound buffer, so
    /// a perform test reads the actor feedback the seam emits. A guest verdict, which
    /// passes the `Gate::Open` affordances a client can click.
    fn run(world: &mut World, actor: EntityId, f: impl FnOnce(&mut Ctx)) -> Vec<Outbound> {
        let mut out = Vec::new();
        let verdict = Verdict::guest();
        let mut ctx = Ctx::new(world, actor, ConnectionId(1), &verdict, &mut out);
        f(&mut ctx);
        out
    }

    /// The first-person feedback lines an outbound buffer carries. Directed at the
    /// actor: the narrating perform emits its first person `to_entity(actor)`, while
    /// the pre-perform gates (perception, arity) emit connection-addressed refusals,
    /// so both count as feedback here.
    fn feedback(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Connection(_) | Audience::Entity(_)))
            .map(|o| o.event.text.clone())
            .collect()
    }

    /// The third-person lines broadcast to a locus: the room narration a co-located
    /// player would read.
    fn room_narration(out: &[Outbound]) -> Vec<String> {
        out.iter()
            .filter(|o| matches!(o.event.to, Audience::Locus(_)))
            .map(|o| o.event.text.clone())
            .collect()
    }

    struct Fixture {
        world: World,
        room: EntityId,
        actor: EntityId,
        coin: EntityId,
        chest: EntityId,
        rock: EntityId,
        gate: EntityId,
    }

    /// A room with the actor holding a coin, plus a chest, a takeable rock, and a
    /// locked gate. Registered as at boot so kind tags read by name.
    fn fixture() -> Fixture {
        let mut world = World::new();
        crate::systems::register(&mut world);

        let room = spawn(&mut world, |b| {
            b.add(Locus);
            b.add(Description("a bare room".into()));
        });
        let actor = spawn(&mut world, |b| {
            b.add(Player);
            b.add(Name("an adventurer".into()));
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
        let rock = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Name("a smooth rock".into()));
            b.add(Description("a smooth grey rock, worn round".into()));
        });
        world.move_entity(rock, room).unwrap();
        let gate = spawn(&mut world, |b| {
            b.add(Exit);
            b.add(Locked);
            b.add(Name("north".into()));
        });
        world.move_entity(gate, room).unwrap();

        Fixture {
            world,
            room,
            actor,
            coin,
            chest,
            rock,
            gate,
        }
    }

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    fn node(snap: &SnapshotData, id: EntityId) -> &Entity {
        snap.entities
            .iter()
            .find(|e| e.id == id.0.to_string())
            .expect("entity in snapshot")
    }

    #[test]
    fn snapshot_roots_at_the_room_and_carries_the_actor() {
        let f = fixture();
        let snap = snapshot(&f.world, f.actor);
        assert_eq!(snap.root, f.room.0.to_string());
        assert_eq!(snap.actor, f.actor.0.to_string());
        // Every entity in the room is present, including the actor and its held coin.
        for id in [f.room, f.actor, f.coin, f.chest, f.rock, f.gate] {
            assert!(
                snap.entities.iter().any(|e| e.id == id.0.to_string()),
                "missing entity {}",
                id.0
            );
        }
    }

    #[test]
    fn inventory_is_the_actors_contents_in_the_snapshot() {
        // The reason there is no separate inventory query: the held coin is a child
        // of the actor node.
        let f = fixture();
        let snap = snapshot(&f.world, f.actor);
        assert_eq!(node(&snap, f.actor).contents, vec![f.coin.0.to_string()]);
    }

    #[test]
    fn kinds_project_the_game_vocabulary() {
        let f = fixture();
        let snap = snapshot(&f.world, f.actor);
        assert!(node(&snap, f.room).kinds.contains(&"locus".to_string()));
        assert!(
            node(&snap, f.chest)
                .kinds
                .contains(&"container".to_string())
        );
        let gate = &node(&snap, f.gate).kinds;
        assert!(gate.contains(&"exit".to_string()));
        assert!(gate.contains(&"locked".to_string()));
    }

    #[test]
    fn details_carry_the_passive_description() {
        // Each node carries the game-projected passive detail: its Description, the
        // prose a narrated examine would reveal, delivered silently in the read. A
        // node with no Description carries an empty bag, not a missing entry.
        let f = fixture();
        let snap = snapshot(&f.world, f.actor);
        assert_eq!(
            node(&snap, f.room).details,
            vec![("description".to_string(), "a bare room".to_string())]
        );
        assert_eq!(
            node(&snap, f.rock).details,
            vec![(
                "description".to_string(),
                "a smooth grey rock, worn round".to_string()
            )]
        );
        assert!(node(&snap, f.actor).details.is_empty());
    }

    #[test]
    fn offers_convert_to_the_wire_statuses() {
        let f = fixture();
        let put = offers(&f.world, f.actor, f.chest)
            .into_iter()
            .find(|o| o.name == "put")
            .unwrap();
        assert!(matches!(
            put.status,
            OfferStatus::NeedsRole { role: Role::Object }
        ));

        let take = offers(&f.world, f.actor, f.rock)
            .into_iter()
            .find(|o| o.name == "take")
            .unwrap();
        assert!(matches!(take.status, OfferStatus::Available));

        let go = offers(&f.world, f.actor, f.gate)
            .into_iter()
            .find(|o| o.name == "go")
            .unwrap();
        assert!(matches!(go.status, OfferStatus::Vetoed { reason } if reason == "It's locked."));
    }

    #[test]
    fn a_clicked_take_commits_and_acknowledges() {
        // The click grounds the take on the rock's id (no name resolved): it ends up
        // held, and the actor is told so.
        let mut f = fixture();
        let rock = f.rock;
        let out = run(&mut f.world, f.actor, |ctx| {
            perform(ctx, &Verdict::guest(), "take", rock, None);
        });
        assert_eq!(f.world.container_of(rock), Some(f.actor));
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t == "You take a smooth rock."),
            "got: {:?}",
            feedback(&out)
        );
    }

    #[test]
    fn a_clicked_take_narrates_to_the_room() {
        // The B fix: a click is an act, so it narrates the third-person line to the
        // room exactly as a typed verb does. Before the shared narrator a click
        // emitted only its first person and a co-located player saw nothing until
        // their next snapshot. The line is locus-addressed and excludes the actor.
        let mut f = fixture();
        let rock = f.rock;
        let out = run(&mut f.world, f.actor, |ctx| {
            perform(ctx, &Verdict::guest(), "take", rock, None);
        });
        assert!(
            room_narration(&out)
                .iter()
                .any(|t| t == "an adventurer takes a smooth rock."),
            "got: {:?}",
            room_narration(&out)
        );
        assert!(
            out.iter()
                .any(|o| matches!(o.event.to, Audience::Locus(_)) && o.exclude.contains(&f.actor)),
            "the room line excludes the actor, who reads their own first person"
        );
    }

    #[test]
    fn a_clicked_act_surfaces_its_refusal_and_changes_nothing() {
        // Dropping the rock the actor is not carrying: the drop guard vetoes exactly
        // as it does for the typed verb, the reason reaches the actor, and the rock
        // stays where it was.
        let mut f = fixture();
        let rock = f.rock;
        let before = f.world.container_of(rock);
        let out = run(&mut f.world, f.actor, |ctx| {
            perform(ctx, &Verdict::guest(), "drop", rock, None);
        });
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t == "You aren't carrying that."),
            "got: {:?}",
            feedback(&out)
        );
        assert_eq!(f.world.container_of(rock), before);
    }

    #[test]
    fn a_clicked_act_on_an_unperceivable_entity_is_refused() {
        // A takeable item in another room the actor cannot see. Acting on it by id
        // must be refused, not grounded: the click is no more powerful than the
        // locus-scoped typed verbs. Without the perceivability gate this pulled the
        // far item into the actor's hands.
        let mut f = fixture();
        let elsewhere = spawn(&mut f.world, |b| {
            b.add(Locus);
        });
        let far_item = spawn(&mut f.world, |b| {
            b.add(Item);
            b.add(Name("a distant gem".into()));
        });
        f.world.move_entity(far_item, elsewhere).unwrap();

        let out = run(&mut f.world, f.actor, |ctx| {
            perform(ctx, &Verdict::guest(), "take", far_item, None);
        });
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t == "You don't see that here."),
            "got: {:?}",
            feedback(&out)
        );
        assert_eq!(f.world.container_of(far_item), Some(elsewhere));
    }

    #[test]
    fn snapshot_projects_relation_backed_exits_under_the_room() {
        // A real exit is relation-backed (`LeadsFrom`), not contained in the room,
        // so raw containment misses it. The client reaches exits only by clicking,
        // so `collect` projects them as room-node children, and they are perceivable
        // (so a clicked `go` is not refused as unseen). The fixture's `gate` is a
        // contained stand-in; this exercises the relation path the seed actually uses.
        use crate::exits::LeadsFrom;
        let mut f = fixture();
        let room = f.world.enclosing_locus(f.actor).unwrap();
        let exit = spawn(&mut f.world, |b| {
            b.add(Exit);
            b.add(Name("east".into()));
        });
        f.world.relate::<LeadsFrom>(exit, room).unwrap();

        let snap = snapshot(&f.world, f.actor);
        assert!(
            node(&snap, room).contents.contains(&exit.0.to_string()),
            "the exit is a child of the room node"
        );
        assert!(
            snap.entities.iter().any(|e| e.id == exit.0.to_string()),
            "the exit has its own node"
        );
        assert!(
            perceivable(&f.world, f.actor, exit),
            "a clicked exit must not be refused as unseen"
        );
    }

    #[test]
    fn a_clicked_take_of_an_item_inside_a_creature_is_refused() {
        // Finding 4: an item in another creature's inventory is perceivable (shares
        // the locus) but not reachable. Acting on it must be refused, and nothing
        // moves. Without the reachability gate this pulled the crumb into the hand.
        let mut f = fixture();
        let room = f.world.enclosing_locus(f.actor).unwrap();
        let mouse = spawn(&mut f.world, |b| {
            b.add(Creature);
            b.add(Name("a field mouse".into()));
        });
        f.world.move_entity(mouse, room).unwrap();
        let crumb = spawn(&mut f.world, |b| {
            b.add(Item);
            b.add(Name("a crumb".into()));
        });
        f.world.move_entity(crumb, mouse).unwrap();

        let out = run(&mut f.world, f.actor, |ctx| {
            perform(ctx, &Verdict::guest(), "take", crumb, None);
        });
        assert!(
            feedback(&out).iter().any(|t| t == "You can't reach that."),
            "got: {:?}",
            feedback(&out)
        );
        assert_eq!(f.world.container_of(crumb), Some(mouse));
    }

    #[test]
    fn a_clicked_put_without_a_sub_pick_is_refused_not_panicked() {
        // `put` needs an object the client sub-picks; omitting it leaves the object
        // role unbound. That must be a clean refusal, not the `bad_frame` invariant
        // inside agency::perform (a debug panic). Nothing moves.
        let mut f = fixture();
        let chest = f.chest;
        let coin = f.coin;
        let held = f.world.container_of(coin);
        let out = run(&mut f.world, f.actor, |ctx| {
            perform(ctx, &Verdict::guest(), "put", chest, None);
        });
        assert!(
            feedback(&out)
                .iter()
                .any(|t| t == "You need to choose something first."),
            "got: {:?}",
            feedback(&out)
        );
        assert_eq!(f.world.container_of(coin), held);
    }
}
