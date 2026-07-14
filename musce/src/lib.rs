//! The one crate a game depends on. `musce` re-exports the engine's game-facing
//! surface and nothing else: a game programs against this, never against the
//! internal crates (`musce_core`, `musce_action`, `musce_host`, ...) directly.
//!
//! The re-exports are grouped by concept (`world`, `action`, `store`, `wire`),
//! not by originating crate, so a public path here is decoupled from which
//! internal crate currently holds the type. Moving `Ctx` between crates, or
//! merging two crates, does not move `musce::action::Ctx`. This facade is the
//! only stability contract; the internal split stays free to churn behind it.
//!
//! `musce_ref` depends on this crate alone for its binary, which makes the
//! surface self-testing: a gap is a compile error there, not a discovery by a
//! downstream consumer. See `docs/architecture/engine-and-game.md`.

/// Identity, components, relations, and the world queries: the `musce_core`
/// layer a game builds its entities and rules on. `hecs` is re-exported for
/// `EntityBuilder` and the raw query API; a `hecs` major version is therefore
/// part of this crate's contract.
pub mod world {
    pub use musce_core::hecs;
    pub use musce_core::{Map, Value};

    pub use musce_core::{Cascade, RelSources, RelTarget, Relation, RelationError};
    pub use musce_core::{
        ComponentBlob, Description, Id, Locus, Name, NamedComponent, RegistryError,
    };
    pub use musce_core::{Controls, Focus, FocusError};
    pub use musce_core::{DestroyCause, Fact};
    pub use musce_core::{EntityBlob, Snapshot};
    pub use musce_core::{EntityId, MutateError, World};
}

/// Verbs, dispatch, the structural mutation path, and the perception/emit
/// channel: the `musce_action` layer a game's command handlers and systems run
/// through.
pub mod action {
    pub use musce_action::actor_name;
    pub use musce_action::{Action, ExecError, execute};
    pub use musce_action::{Actors, Audience, Event, Outbound, resolve};
    pub use musce_action::{Caller, CommandTable, Gate, Handler, dispatch_command};
    pub use musce_action::{CapId, CapSet, Verdict};
    pub use musce_action::{ColdOp, Ctx, System, SystemCtx, run_systems};
}

/// Durable storage. `WorldStore` is the game-facing handle, chosen by URL scheme
/// at connect time; the concrete backends stay internal so a game names one type
/// whether it runs on SQLite or Postgres.
pub mod store {
    pub use musce_persistence::{Error, KvStore, Loaded, Persistence, SCHEMA_VERSION, WorldStore};
}

/// The wire vocabulary a game's output addresses: connection identity and the
/// event kinds a handler emits.
pub mod wire {
    pub use musce_proto::{ConnectionId, EventKind};
}

/// Accounts and capabilities, re-exported wholesale from the auth layer.
pub use musce_host::auth;

/// Generic secondary indexes over a component, behind the `musce_index` feature. A
/// game enables `features = ["musce_index"]` and reaches the index machinery here;
/// a game that does not is unaffected.
#[cfg(feature = "musce_index")]
pub mod index {
    pub use musce_index::*;
}

// The composition-root API: what a game's `main` wires up and hands to `run`.
pub use musce_host::{
    ChooseActor, Config, Game, LISTEN_ADDR, Register, RunReport, SAVE_EVERY, Seed, TICK_INTERVAL,
    TickCtx, run,
};

/// The high-frequency surface, for `use musce::prelude::*;`. Curated, not a glob
/// of everything: the types a game touches on nearly every screen (the world
/// handle, the handler context, the mutation path, the common components), so the
/// canonical grouped paths stay available without forcing dozens of imports.
pub mod prelude {
    pub use crate::action::{Action, Ctx, SystemCtx, execute};
    pub use crate::world::hecs::EntityBuilder;
    pub use crate::world::{Description, EntityId, Locus, Name, NamedComponent, Value, World};
    pub use crate::{Config, Game, run};
}
