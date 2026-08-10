//! The one crate an app depends on. `musce` re-exports the engine's app-facing
//! surface and nothing else: an app programs against this, never against the
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
//! downstream consumer. See `docs/architecture/engine-and-app.md`.

/// Identity, components, relations, and the world queries: the `musce_core`
/// layer an app builds its entities and rules on. `hecs` is re-exported for
/// `EntityBuilder` and the raw query API; a `hecs` major version is therefore
/// part of this crate's contract.
pub mod world {
    pub use musce_core::hecs;
    pub use musce_core::{Map, Value};

    pub use musce_core::{
        AcyclicRelation, Cascade, RelTarget, Relation, RelationError, RelationRole, Walk,
    };
    pub use musce_core::{
        ComponentBlob, Description, Id, Locus, Name, NamedComponent, RegistryError,
    };
    pub use musce_core::{Controls, Focus, FocusError};
    pub use musce_core::{DestroyCause, Fact};
    pub use musce_core::{EntityBlob, LoadError, Snapshot};
    pub use musce_core::{EntityId, MutateError, World};
}

/// Verbs, dispatch, the structural mutation path, and the perception/emit
/// channel: the `musce_action` layer an app's command handlers and systems run
/// through.
pub mod action {
    pub use musce_action::actor_name;
    pub use musce_action::{Action, ActionKind, ExecError, execute};
    pub use musce_action::{Actors, Audience, Event, Outbound, resolve};
    pub use musce_action::{
        Affordance, Clause, Frame, Guard, Literal, Predicate, Term, Var, WorldModel,
    };
    pub use musce_action::{
        Caller, CommandTable, Gate, Grounded, Handler, PerformHandler, dispatch_command,
        dispatch_perform,
    };
    pub use musce_action::{CapId, CapRegistry, CapSet, Verdict};
    pub use musce_action::{ColdOp, Ctx, System, SystemCtx, run_systems};
    pub use musce_action::{GaugeDirection, GaugeId, GaugeLevel, GaugeTarget};
}

/// Durable storage. `WorldStore` is the app-facing handle, chosen by URL scheme
/// at connect time; the concrete backends stay internal so an app names one type
/// whether it runs on SQLite or Postgres.
pub mod store {
    pub use musce_persistence::{Error, KvStore, Loaded, Persistence, SCHEMA_VERSION, WorldStore};
}

/// The wire vocabulary an app's output addresses: connection identity and the
/// event kinds a handler emits.
pub mod wire {
    pub use musce_proto::{
        ConnectionId, Entity, EventKind, Offer, OfferStatus, Role, SnapshotData,
    };
}

/// The authentication surface an app supplies to the runtime: the login veto and
/// the restricted account view it inspects. Capabilities live under `action` (an
/// authorization concern), and account identity/hashing under `musce_auth`, exposed
/// here once an app needs to build real credentials.
pub mod auth {
    pub use musce_host::{AccountId, AccountView, LoginVeto};
}

/// Generic secondary indexes over a component, behind the `musce_index` feature. An
/// app enables `features = ["musce_index"]` and reaches the index machinery here;
/// an app that does not is unaffected.
#[cfg(feature = "musce_index")]
pub mod index {
    pub use musce_index::*;
}

/// The optional planner side of the agency subsystem (the `CostModel` seam and
/// the `bind_var` binding primitive now; the planner and arbiter later), behind
/// the `musce_agency` feature. The affordance vocabulary it plans over is
/// non-optional engine surface under [`action`]; this module re-exports it too,
/// so a planner-facing consumer reaches everything from one path. An app enables
/// `features = ["musce_agency"]` and reaches it here. See
/// `docs/architecture/agency/` and `docs/architecture/affordances.md`.
#[cfg(feature = "musce_agency")]
pub mod agency {
    pub use musce_agency::*;
}

// The composition-root API: what an app's `main` wires up and hands to `run`.
pub use musce_host::{
    App, ChooseActor, Config, LISTEN_ADDR, Register, RunReport, SAVE_EVERY, Seed, TICK_INTERVAL,
    TickCtx, WS_LISTEN_ADDR, run,
};

/// The high-frequency surface, for `use musce::prelude::*;`. Curated, not a glob
/// of everything: the types an app touches on nearly every screen (the world
/// handle, the handler context, the mutation path, the common components), so the
/// canonical grouped paths stay available without forcing dozens of imports.
pub mod prelude {
    pub use crate::action::{Action, Ctx, SystemCtx, execute};
    pub use crate::world::hecs::EntityBuilder;
    pub use crate::world::{Description, EntityId, Locus, Name, NamedComponent, Value, World};
    pub use crate::{App, Config, run};
}
