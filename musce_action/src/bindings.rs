//! The audience index: which actor each live connection drives. This is a
//! derived, transient view, not session truth. The host owns the conn->actor
//! attachment as session state (see `musce_host`'s session floor) and builds one
//! of these per dispatch from it; the audience resolver consumes the reverse
//! direction to turn in-world actors back into the connections that perceive
//! them. The world never holds it. See
//! `docs/architecture/networking-and-sessions.md`.

use std::collections::HashMap;

use musce_core::EntityId;
use musce_proto::ConnectionId;

/// Which actor each connection currently drives. One connection drives at most
/// one actor; several connections may drive the same actor. The only directions
/// needed are building it (`bind`) and iterating or searching the bindings for
/// audience resolution.
#[derive(Default)]
pub struct Actors {
    by_conn: HashMap<ConnectionId, EntityId>,
}

impl Actors {
    pub fn bind(&mut self, conn: ConnectionId, actor: EntityId) {
        self.by_conn.insert(conn, actor);
    }

    /// Every live `(connection, actor)` binding. Locus audience resolution uses
    /// this direction because perception is defined by each actor's enclosing
    /// locus, not by a locus's direct contents.
    pub fn bindings(&self) -> impl Iterator<Item = (ConnectionId, EntityId)> + '_ {
        self.by_conn.iter().map(|(&conn, &actor)| (conn, actor))
    }

    /// Every connection driving `actor`. Linear in the binding count, which is
    /// fine at this scale; a reverse index can come later if it ever profiles
    /// hot.
    pub fn conns_for(&self, actor: EntityId) -> impl Iterator<Item = ConnectionId> + '_ {
        self.by_conn
            .iter()
            .filter_map(move |(&conn, &a)| (a == actor).then_some(conn))
    }
}
