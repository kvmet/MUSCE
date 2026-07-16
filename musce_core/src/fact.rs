//! Structural facts: observations of world mutations, emitted at the `World`
//! mutator layer and delivered to systems as a read-only per-tick batch. A fact is
//! never itself a mutation and is never persisted; it is the engine reporting what
//! just happened so game logic can react (e.g. a death cry on destruction). The
//! buffer hangs off `World` and is drained once per tick; see
//! `docs/architecture/facts.md`.
//!
//! The set is deliberately small. A mutation earns a fact only for one of two
//! reasons:
//!
//! 1. It carries state a reaction needs that the post-mutation world can no longer
//!    answer: either the mutation destroyed that state (destruction annihilates the
//!    dying entity's locus and name, hence the pre-removal snapshot in `Destroyed`)
//!    or the change happened somewhere unobservable (a cascade removal below
//!    `execute`). This is the payload-carrier role; `Destroyed`/`Moved`/
//!    `LocusChanged` fill it.
//! 2. It is a bounded trigger a per-tick maintainer cannot cheaply poll for. Some
//!    consumers keep a derived read-model current and would otherwise rescan the
//!    world every tick to find what changed; a trigger lets them react to the few
//!    entities that moved instead of the whole table. `ComponentChanged` fills this
//!    role. It is kept bounded by an explicit opt-in (`World::track_component`): a
//!    component emits nothing until something asks to track it, so the stream never
//!    grows to narrate-everything.
//!
//! The two roles are not disjoint: `Destroyed` already serves both, carrying an
//! unrecoverable payload for a death reaction while an index consumes it purely as a
//! lifecycle trigger (evict the gone entity, reread nothing). `ComponentChanged`
//! extends that existing trigger-consumption rather than inverting the charter.
//!
//! A mutation whose result is fully queryable afterward and that no maintainer needs
//! a trigger for still gets no fact. Facts recover the unrecoverable or feed a
//! bounded maintainer; they do not narrate.

use crate::id::EntityId;

/// Why an entity was destroyed. The discriminator that lets one reaction catch
/// every removal in a recursive `@purge` while skipping the collateral of a single
/// `@destroy` (whose cascade removals are `Cascade`, not `Direct`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestroyCause {
    /// The entity a destroy directly targeted.
    Direct,
    /// An entity removed by a relation cascade below a targeted destroy.
    Cascade,
}

/// An observation of a structural world mutation. Captured at the mutator layer
/// (so cascades, which happen below `execute`, are observed too) and read by a
/// reaction after the mutation has committed.
#[derive(Clone, Debug)]
pub enum Fact {
    /// An entity was despawned. `last_locus` and `name` are a pre-removal snapshot:
    /// the entity is gone by the time a reaction reads them, so the data it needs
    /// is captured while the entity is still live. `name` is the entity's `Name`,
    /// falling back to its `Description` (`None` if it carried neither);
    /// `last_locus` is its `enclosing_locus` (`None` for a location-less entity or
    /// a top-level locus).
    Destroyed {
        entity: EntityId,
        last_locus: Option<EntityId>,
        name: Option<String>,
        cause: DestroyCause,
    },
    /// An entity's containment link changed: it was reparented from `from` to `to`.
    /// `from` is the vanished prior container (`None` if it was a root), `to` the
    /// new one (`None` if it became a root, e.g. reparented out of a destroyed
    /// top-level container).
    ///
    /// Fires for the entity whose **own** containment link changed, and never for
    /// the subtree it carries. A character walking rooms reparents only itself; the
    /// sword in its hand keeps its link (`contained_by` the character), so the
    /// engine emits nothing for the sword. That is deliberate: the sword's locus
    /// change is *derivable* from this fact plus the containment tree (the sword is
    /// under the character, so it went where the character went), and a fact exists
    /// only for what a reaction cannot reconstruct. A consumer that needs the moved
    /// subtree walks `descendants(entity)` from this fact, once, only when it cares.
    /// See `docs/architecture/facts.md`.
    Moved {
        entity: EntityId,
        from: Option<EntityId>,
        to: Option<EntityId>,
    },
    /// A move carried an entity across a perception boundary: its `enclosing_locus`
    /// changed from `from` to `to` (`None` for a location-less entity or a top-level
    /// locus). Emitted **in addition to** `Moved`, and only when the locus actually
    /// differs, so a perception reaction subscribes to this alone and never
    /// recomputes the boundary. Like `Moved`, it fires for the entity whose own link
    /// changed, not its carried subtree (same derivability argument).
    LocusChanged {
        entity: EntityId,
        from: Option<EntityId>,
        to: Option<EntityId>,
    },
    /// A tracked component was set, overwritten, or removed on an entity. A
    /// payload-free *trigger*, not a payload-carrier: it names only the entity and
    /// the component's `tag`, and a maintainer reconciles by rereading the current
    /// value (present after a set, absent after a remove, recovering the old key from
    /// its own reverse map). Nothing here is unrecoverable, so nothing is carried;
    /// see the two-reason charter above.
    ///
    /// Emitted only for components a consumer opted into via
    /// `World::track_component`, and only from the `World` mutator layer (set/remove
    /// by tag, typed insert/remove, `create`, and `modify`), never from a raw
    /// in-crate `&mut` component write, which bypasses the mutator entirely (see
    /// `World::modify` and the tracking guard). Duplicate triggers in a tick are
    /// safe: reread is idempotent, and order against a same-entity `Destroyed` is
    /// irrelevant because reread converges either way.
    ComponentChanged { entity: EntityId, tag: &'static str },
}
