//! The action layer: the in-game command surface over the structural executor.
//! Pure synchronous logic (no tokio), depending only on `musce_core` (the world)
//! and `musce_proto` (the command/event vocabulary), so it stays fast to test and
//! free of the transport. See `docs/architecture/actions.md`.
//!
//! Three pieces fit together:
//! - `execute` applies a typed [`Action`] to the world, structurally only.
//! - the verb handlers (`look`, `go`, `take`, `drop`, `say`) own gameplay rules
//!   and perception, dispatched through the [`CommandTable`].
//! - the audience resolver expands room/entity-addressed output into the
//!   connections that should see it, before anything reaches net.

mod audience;
mod bindings;
mod dispatch;
mod executor;
mod names;
mod seed;
mod verbs;

use musce_core::{EntityId, World};
use musce_proto::ConnectionId;

pub use bindings::Actors;
pub use dispatch::{CommandTable, dispatch_bare};
pub use executor::{Action, ExecError, execute};
pub use seed::{Seeded, seed};

/// The stub `@play`: bind a connection to the seeded player avatar as session
/// state. Returns the actor so the floor can confirm it to the player. The next
/// increment replaces this with the persisted `Controls`/`Focus` flow without
/// touching the verb handlers, which already take the actor explicitly.
pub fn play(world: &World, actors: &mut Actors, conn: ConnectionId) -> Option<EntityId> {
    let actor = seed::find_player(world)?;
    actors.bind(conn, actor);
    Some(actor)
}

/// The actor's own description, for confirmations like "You become X." `None` if
/// the entity has no description.
pub fn actor_name(world: &World, actor: EntityId) -> Option<String> {
    world
        .entity(actor)
        .and_then(|er| er.get::<&musce_core::Description>().map(|d| d.0.clone()))
}
