//! Structural facts: observations of world mutations, emitted at the `World`
//! mutator layer and delivered to systems as a read-only per-tick batch. A fact is
//! never itself a mutation and is never persisted; it is the engine reporting what
//! just happened so game logic can react (e.g. a death cry on destruction). The
//! buffer hangs off `World` and is drained once per tick; see
//! `docs/architecture/facts.md`.
//!
//! The set is deliberately small. A mutation earns a fact only where a reaction
//! needs something it cannot reconstruct by querying the post-mutation world:
//! either the mutation destroyed that state (destruction annihilates the dying
//! entity's locus and name, hence the pre-removal snapshot in `Destroyed`) or the
//! change is otherwise unobservable (a cascade removal happens below `execute`). A
//! mutation whose result is fully queryable afterward gets no fact. Facts recover
//! the unrecoverable; they do not narrate. `Moved` and `LocusChanged` are the
//! proposed near-term additions (movement's vanished prior container and prior
//! locus); see the doc for the full reasoning.

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
}
