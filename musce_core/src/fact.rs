//! Structural facts: observations of world mutations, emitted at the `World`
//! mutator layer and delivered to systems as a read-only per-tick batch. A fact is
//! never itself a mutation and is never persisted; it is the engine reporting what
//! just happened so game logic can react (e.g. a death cry on destruction). The
//! buffer hangs off `World` and is drained once per tick; see
//! `docs/architecture/actions.md`.

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
