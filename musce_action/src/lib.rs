//! The action layer: the engine's in-game command surface over the structural
//! executor. Pure synchronous logic (no tokio), depending only on `musce_core`
//! (the world) and `musce_proto` (the command/event vocabulary), so it stays fast
//! to test and free of the transport. It holds engine mechanism only; the verbs,
//! their parsing, name resolution, and the seed world are game content and live
//! in the reference game `musce_ref`. See `docs/architecture/actions.md` and
//! `docs/architecture/engine-and-game.md`.
//!
//! Three pieces fit together:
//! - `execute` applies a typed [`Action`] to the world, structurally only.
//! - the [`CommandTable`] looks a game's registered verbs up and
//!   `dispatch_command` runs them through a [`Ctx`] whose emit API is the surface
//!   handlers program against; the host points it at the embodiment or admin
//!   table per the input-stack frame.
//! - the audience resolver expands room/entity-addressed output into the
//!   connections that should see it, before anything reaches net.

mod audience;
mod bindings;
mod ctx;
mod dispatch;
mod executor;

use musce_core::{EntityId, World};

pub use audience::{Outbound, resolve};
pub use bindings::Actors;
pub use ctx::{Ctx, System, SystemCtx};
pub use dispatch::{CommandTable, Gate, Handler, dispatch_command};
pub use executor::{Action, ExecError, execute};

/// The actor's own description, for floor confirmations like "You are now X."
/// `None` if the entity has no description. This is a plain component read, not
/// name *resolution* (matching a typed noun against descriptions), which is game
/// policy and lives in `musce_ref`.
pub fn actor_name(world: &World, actor: EntityId) -> Option<String> {
    world
        .entity(actor)
        .and_then(|er| er.get::<&musce_core::Description>().map(|d| d.0.clone()))
}
