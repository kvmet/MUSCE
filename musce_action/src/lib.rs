//! The action layer: the engine's in-app command surface over the structural
//! executor. Pure synchronous logic (no tokio), depending only on `musce_core`
//! (the world) and `musce_proto` (the command/event vocabulary), so it stays fast
//! to test and free of the transport. It holds engine mechanism only; the verbs,
//! their parsing, name resolution, and the seed world are app content and live
//! in the reference app `musce_ref`. See `docs/architecture/actions.md` and
//! `docs/architecture/engine-and-app.md`.
//!
//! Three pieces fit together:
//! - `execute` applies a typed [`Action`] to the world, structurally only.
//! - the [`CommandTable`] looks an app's registered verbs up and
//!   `dispatch_command` runs them through a [`Ctx`] whose emit API is the surface
//!   handlers program against; the host points it at the embodiment or admin
//!   table per the input-stack frame.
//! - the audience resolver expands room/entity-addressed output into the
//!   connections that should see it, before anything reaches net.

mod affordance;
mod audience;
mod bindings;
mod caps;
mod ctx;
mod dispatch;
mod event;
mod executor;
mod gauge;
mod registry;

use musce_core::{EntityId, World};

pub use affordance::{Affordance, Clause, Frame, Guard, Literal, Predicate, Term, Var, WorldModel};
pub use audience::{Outbound, resolve};
pub use bindings::Actors;
pub use caps::{CapId, CapSet, Verdict};
pub use ctx::{Caller, ColdOp, Ctx, System, SystemCtx, run_systems};
pub use dispatch::{
    CommandTable, Gate, Grounded, Handler, PerformHandler, dispatch_command, dispatch_perform,
};
pub use event::{Audience, Event};
pub use executor::{Action, ExecError, execute};
pub use gauge::{GaugeDirection, GaugeId, GaugeLevel, GaugeTarget};
pub use registry::CapRegistry;

/// The actor's own description, for floor confirmations like "You are now X."
/// `None` if the entity has no description. This is a plain component read, not
/// name *resolution* (matching a typed noun against descriptions), which is app
/// policy and lives in `musce_ref`.
pub fn actor_name(world: &World, actor: EntityId) -> Option<String> {
    world
        .get::<musce_core::Description>(actor)
        .map(|d| d.0.clone())
}
