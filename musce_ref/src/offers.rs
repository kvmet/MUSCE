//! "What can I do to this?": enumerating the affordances a client offers when a
//! player points at an entity. A text parser never needs this (it resolves a
//! named verb straight to one handler), but a click-driven client does: it holds
//! an entity and asks the game for the acts available on it, each annotated so the
//! client can render a live control, a greyed one with the reason, or a prompt for
//! the missing piece.
//!
//! This is game content, like [`crate::agency::perform`]: it names the concrete
//! affordance set and reads them through [`RefWorldModel`]. The *classification*
//! ([`classify`]) is generic and could promote into `musce_action` once a second
//! consumer wants it; it stays here until then, per the promotion discipline in
//! `docs/architecture/affordances.md`.
//!
//! Three shapes this query forces into the open, none of which the single-verb
//! [`Affordance::veto`](musce::agency::Affordance::veto) path had to face:
//!
//! 1. **Which role the pointed-at entity fills** ([`focus_role`]) is implicit in
//!    the parser's name resolution and in `perform`'s match arms. A resolver-less
//!    client has neither, so the convention must be stated.
//! 2. **`veto` conflates an unbound role with a failed guard.** An unbound `Var`
//!    reads as a false predicate, so `put` on a container with nothing chosen to
//!    put would report "You aren't carrying that" rather than "pick something".
//!    So role-completeness is tested *before* `veto`, and the status is three-way,
//!    not two ([`OfferStatus`]).
//! 3. **The parser's implicit type filter is lost.** `go north` resolves to an
//!    exit by construction; a click does not, so this query will offer `go` on a
//!    rock (it isn't locked) and `take` on a chest (it isn't a being). Recovering
//!    that filter is a deferred design decision, documented and tested as a known
//!    gap below.

use musce::agency::{Affordance, Frame, Predicate, Term, WorldModel};
use musce::world::{EntityId, World};

use crate::agency::{RefWorldModel, drop, eat, go, put, take};

/// The frame role the pointed-at entity fills for an affordance. `actor` is always
/// the player and `kind` is a preposition, so neither is a click target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Object,
    Target,
}

/// How an affordance stands for a given pointed-at entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferStatus {
    /// Every role the guards need is bound and no guard vetoes. A live control.
    Available,
    /// Every needed role is bound, but a guard fails. A greyed control carrying
    /// the reason the player would hear.
    Vetoed(&'static str),
    /// The pointed-at entity filled the focus role, but a guard still constrains an
    /// unbound role (`put` needs an object once you have picked the container). A
    /// control that opens a sub-pick for `role`.
    NeedsRole(Role),
}

/// One enumerated act: the affordance name and its status for the pointed-at entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub name: String,
    pub status: OfferStatus,
}

/// The affordances this game exposes to a client, the same set [`perform`] can
/// dispatch. A by-name/by-effect table lands with the planner's needs; a flat list
/// is all enumeration wants.
///
/// [`perform`]: crate::agency::perform
fn affordances() -> Vec<Affordance> {
    vec![take(), drop(), put(), eat(), go()]
}

/// The acts available on `clicked` for `actor`: one [`Offer`] per affordance, the
/// pointed-at entity bound into each affordance's focus role and the rest left for
/// [`classify`] to report. This resolves nothing beyond the focus role, the way a
/// handler leaves name resolution to game policy.
pub fn affordances_on(world: &World, actor: EntityId, clicked: EntityId) -> Vec<Offer> {
    affordances()
        .into_iter()
        .map(|aff| {
            let frame = frame_for(actor, clicked, focus_role(&aff.name));
            let status = classify(&aff, &frame, world, &RefWorldModel);
            Offer {
                name: aff.name,
                status,
            }
        })
        .collect()
}

/// The three-way status [`Affordance::veto`](musce::agency::Affordance::veto)
/// alone cannot give. A guard over an unbound role-`Var` reads as failed (the
/// game's model answers a free `Var` as not-held), which is "role not chosen yet",
/// not "vetoed". So completeness is checked first, and only a frame that binds
/// every guard-referenced role reaches `veto`.
fn classify(aff: &Affordance, frame: &Frame, world: &World, model: &dyn WorldModel) -> OfferStatus {
    for role in required_roles(aff) {
        if !filled(frame, role) {
            return OfferStatus::NeedsRole(role);
        }
    }
    match aff.veto(frame, world, model) {
        Some(guard) => OfferStatus::Vetoed(guard.reason),
        None => OfferStatus::Available,
    }
}

/// Which role the pointed-at entity fills. `put`/`go` act *on* a target (a
/// container, an exit); the rest act on an object. Keyed by name so both
/// enumeration and a grounded click (`crate::pointing::perform`) map the focus the
/// same way.
pub(crate) fn focus_role(name: &str) -> Role {
    match name {
        "put" | "go" => Role::Target,
        _ => Role::Object,
    }
}

/// The affordance this game exposes under `name`, or `None` if there is none. The
/// by-name lookup a grounded click resolves through, drawn from the same
/// [`affordances`] set enumeration reports.
pub(crate) fn affordance_named(name: &str) -> Option<Affordance> {
    affordances().into_iter().find(|a| a.name == name)
}

/// The roles a client must supply, recovered from the roles the affordance's
/// *guards* reference. This needs no new field on [`Affordance`]: arity is already
/// latent in the clauses. Guards, not the effect, because a guard names an entity
/// whose state must be validated (so it must be bound), while an effect may name a
/// *derived* destination the game fills itself (`drop`'s target is the actor's
/// room, never a pick). For the current verb set every user-supplied role is
/// guard-constrained; an affordance that reads a role `perform` needs but no guard
/// mentions would want this revisited.
pub(crate) fn required_roles(aff: &Affordance) -> Vec<Role> {
    [("object", Role::Object), ("target", Role::Target)]
        .into_iter()
        .filter(|(name, _)| guards_mention(aff, name))
        .map(|(_, role)| role)
        .collect()
}

/// Whether any guard clause references a role-`Var` of this name.
fn guards_mention(aff: &Affordance, name: &str) -> bool {
    aff.guards
        .iter()
        .flat_map(|g| &g.clause.0)
        .any(|lit| match &lit.predicate {
            Predicate::Related { a, b, .. } => is_var(a, name) || is_var(b, name),
            Predicate::Tag { e, .. } => is_var(e, name),
        })
}

/// Whether `term` is exactly the role variable named `name`.
fn is_var(term: &Term, name: &str) -> bool {
    matches!(term, Term::Var(v) if v.0 == name)
}

fn frame_for(actor: EntityId, clicked: EntityId, focus: Role) -> Frame {
    let mut frame = Frame {
        actor,
        object: None,
        target: None,
        kind: None,
    };
    match focus {
        Role::Object => frame.object = Some(clicked),
        Role::Target => frame.target = Some(clicked),
    }
    frame
}

pub(crate) fn filled(frame: &Frame, role: Role) -> bool {
    match role {
        Role::Object => frame.object.is_some(),
        Role::Target => frame.target.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce::world::hecs::EntityBuilder;
    use musce::world::{Description, Locus, Name};

    use crate::kinds::{Container, Exit, Item};
    use crate::verbs::Locked;

    struct Fixture {
        world: World,
        actor: EntityId,
        chest: EntityId,
        coin: EntityId,
        loose: EntityId,
        rock: EntityId,
        gate: EntityId,
    }

    /// A room with: the actor holding a `coin`; a `chest` (`Container`, not
    /// takeable); a `loose` coin on the floor; a takeable `rock`; and a locked
    /// `gate` exit. Registered as at boot so guards read tags by name.
    fn fixture() -> Fixture {
        let mut world = World::new();
        crate::systems::register(&mut world);

        let room = spawn(&mut world, |b| {
            b.add(Locus);
            b.add(Description("a bare room".into()));
        });
        let actor = spawn(&mut world, |b| {
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

        let loose = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Name("a stray button".into()));
        });
        world.move_entity(loose, room).unwrap();

        let rock = spawn(&mut world, |b| {
            b.add(Item);
            b.add(Name("a smooth rock".into()));
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
            actor,
            chest,
            coin,
            loose,
            rock,
            gate,
        }
    }

    fn spawn(w: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
        let mut b = EntityBuilder::new();
        f(&mut b);
        w.spawn(b)
    }

    /// The status of one named affordance when `actor` points at `clicked`.
    fn offer(f: &Fixture, clicked: EntityId, name: &str) -> OfferStatus {
        affordances_on(&f.world, f.actor, clicked)
            .into_iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("no offer named {name}"))
            .status
    }

    #[test]
    fn pointing_at_a_container_offers_put_but_needs_an_object() {
        // The case bare `veto` gets wrong: the object is unbound, which is "pick
        // something to put", not the held-guard's "You aren't carrying that".
        let f = fixture();
        assert_eq!(
            offer(&f, f.chest, "put"),
            OfferStatus::NeedsRole(Role::Object)
        );
    }

    #[test]
    fn choosing_the_object_resolves_put_to_available_or_vetoed() {
        // The sub-pick: bind the second role and re-classify. A held coin passes
        // both guards; an unheld one fails the first with its real reason.
        let f = fixture();
        let full = |object| Frame {
            actor: f.actor,
            object: Some(object),
            target: Some(f.chest),
            kind: None,
        };
        assert_eq!(
            classify(&put(), &full(f.coin), &f.world, &RefWorldModel),
            OfferStatus::Available
        );
        assert_eq!(
            classify(&put(), &full(f.loose), &f.world, &RefWorldModel),
            OfferStatus::Vetoed("You aren't carrying that.")
        );
    }

    #[test]
    fn a_guard_veto_surfaces_as_the_greyed_reason() {
        let f = fixture();
        assert_eq!(offer(&f, f.gate, "go"), OfferStatus::Vetoed("It's locked."));
    }

    #[test]
    fn a_clean_act_is_available() {
        let f = fixture();
        assert_eq!(offer(&f, f.rock, "take"), OfferStatus::Available);
    }

    #[test]
    fn known_gap_enumeration_over_offers_without_a_type_filter() {
        // Finding 3: with no resolver, the focus role binds any entity, so an
        // affordance whose guards happen to pass on the wrong *kind* is offered.
        // This pins the current behavior so the eventual type filter is a
        // deliberate change, not an accidental one. See the module docs.
        let f = fixture();
        // `take` on a chest: a chest is not a locus/player/creature, so the
        // takeable guard passes even though a chest is a fixture you can't carry.
        assert_eq!(offer(&f, f.chest, "take"), OfferStatus::Available);
        // `go` on a rock: a rock is not locked, so the traversal guard passes even
        // though a rock is not an exit.
        assert_eq!(offer(&f, f.rock, "go"), OfferStatus::Available);
    }
}
