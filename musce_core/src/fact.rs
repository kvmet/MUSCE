//! Structural facts: observations of world mutations, emitted at the `World`
//! mutator layer and delivered to systems as a read-only per-tick batch. A fact is
//! never itself a mutation and is never persisted; it is the engine reporting what
//! just happened so game logic can react (e.g. a death cry on destruction). The
//! buffer hangs off `World` and is drained once per tick; see
//! `docs/architecture/actions.md`.
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
}
