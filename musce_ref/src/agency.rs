//! Reference-game perception policy for planning candidates.

use musce::world::{EntityId, World};

/// Entities directly sharing the actor's enclosing locus. The app decides this
/// knowledge scope; the generic planner never broadens it.
pub fn known_here(world: &World, actor: EntityId) -> Vec<EntityId> {
    match world.enclosing_locus(actor) {
        Some(locus) => world
            .contents(locus)
            .iter()
            .copied()
            .filter(|&entity| entity != actor)
            .collect(),
        None => Vec::new(),
    }
}
