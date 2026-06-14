//! The stub connection<->actor binding. `@play` records that a connection drives
//! an actor `EntityId`; bare in-game commands and the audience resolver read it.
//!
//! This is deliberately the *session-state* stub the action slice stands on: the
//! next increment replaces it with the persisted `Controls` relation + `Focus`
//! component (world state) without touching the verb handlers, which already take
//! the actor explicitly. See `docs/architecture/networking-and-sessions.md`.

use std::collections::HashMap;

use musce_core::EntityId;
use musce_proto::ConnectionId;

/// Which actor each connection currently drives. One connection drives at most
/// one actor here; several connections may drive the same actor (no exclusion in
/// the stub). The reverse direction (`conns_for`) is what the audience resolver
/// needs to turn an in-world actor back into the connections that perceive it.
#[derive(Default)]
pub struct Actors {
    by_conn: HashMap<ConnectionId, EntityId>,
}

impl Actors {
    pub fn bind(&mut self, conn: ConnectionId, actor: EntityId) {
        self.by_conn.insert(conn, actor);
    }

    pub fn unbind(&mut self, conn: ConnectionId) {
        self.by_conn.remove(&conn);
    }

    pub fn actor_of(&self, conn: ConnectionId) -> Option<EntityId> {
        self.by_conn.get(&conn).copied()
    }

    /// Every connection driving `actor`. Linear in the binding count, which is
    /// fine at stub scale; a reverse index can come with the real `Controls`
    /// layer if it ever profiles hot.
    pub fn conns_for(&self, actor: EntityId) -> impl Iterator<Item = ConnectionId> + '_ {
        self.by_conn
            .iter()
            .filter_map(move |(&conn, &a)| (a == actor).then_some(conn))
    }
}
