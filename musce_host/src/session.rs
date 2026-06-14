//! The session floor: the always-present bottom of the input stack. Once a
//! connection is authenticated it has a session here, and `@`-namespaced account
//! commands route to it no matter what is overlaid on top. Auth is still stubbed
//! (every connection is an anonymous guest).
//!
//! This slice grows the floor with the stub `@play`, which binds the connection
//! to the seeded player avatar so bare commands have an actor to act through.
//! That binding is session state (held in `musce_action::Actors`), not world
//! state; the next increment replaces it with the persisted `Controls`/`Focus`
//! flow. See `docs/architecture/networking-and-sessions.md`.

use std::collections::HashMap;

use musce_action::Actors;
use musce_core::World;
use musce_proto::{ConnectionId, Event, EventKind, Outgoing};

/// One live session. A marker for now (its presence means the connection is
/// authenticated); it will grow to hold the account id and character slots.
struct Session;

/// The floor for every connection. Owns the session table and handles the
/// `@`-namespace. The connection<->actor binding lives in `Actors` (owned by the
/// dispatcher), so several frames can read it; the floor only writes it via
/// `@play`.
#[derive(Default)]
pub struct Sessions {
    map: HashMap<ConnectionId, Session>,
}

impl Sessions {
    /// Allocate a session for a freshly connected client and greet it.
    pub fn connect(&mut self, id: ConnectionId, emit: &mut impl FnMut(Outgoing)) {
        self.map.insert(id, Session);
        emit(Outgoing::Event(Event::to_connection(
            id,
            EventKind::System,
            "Welcome to MUSCE. @play to enter the world, @help for commands.",
        )));
    }

    /// Tear a session down. The caller also clears the actor binding.
    pub fn disconnect(&mut self, id: ConnectionId) {
        self.map.remove(&id);
    }

    /// Whether this connection has an authenticated session. Input from a
    /// connection without one (net got ahead of us, or a late command after
    /// teardown) has nothing to act on.
    pub fn is_live(&self, id: ConnectionId) -> bool {
        self.map.contains_key(&id)
    }

    pub fn online_count(&self) -> usize {
        self.map.len()
    }

    /// Handle one `@`-namespaced account command (the leading `@` already
    /// stripped). These are the floor and stay reachable regardless of what sits
    /// on top of the input stack.
    pub fn account_command(
        &mut self,
        id: ConnectionId,
        rest: &str,
        world: &World,
        actors: &mut Actors,
        emit: &mut impl FnMut(Outgoing),
    ) {
        let mut parts = rest.split_whitespace();
        let verb = parts.next().unwrap_or("");
        match verb {
            "quit" => {
                feedback(id, "Goodbye.", emit);
                emit(Outgoing::Close(id));
            }
            "who" => {
                feedback(id, &format!("{} connection(s) online.", self.online_count()), emit);
            }
            "help" => {
                feedback(
                    id,
                    "Commands: @play, @quit, @who, @help. In-world: look, go <dir>, take, drop, say.",
                    emit,
                );
            }
            "play" => self.play(id, world, actors, emit),
            other => {
                feedback(id, &format!("Unknown command: @{other}"), emit);
            }
        }
    }

    /// The stub `@play`: bind this connection to the seeded player avatar so its
    /// bare commands have an actor.
    fn play(
        &mut self,
        id: ConnectionId,
        world: &World,
        actors: &mut Actors,
        emit: &mut impl FnMut(Outgoing),
    ) {
        match musce_action::play(world, actors, id) {
            Some(actor) => {
                let name = musce_action::actor_name(world, actor).unwrap_or_else(|| "someone".into());
                feedback(id, &format!("You are now {name}. Type 'look' to see where you are."), emit);
            }
            None => feedback(id, "There is no character to play yet.", emit),
        }
    }
}

fn feedback(id: ConnectionId, text: &str, emit: &mut impl FnMut(Outgoing)) {
    emit(Outgoing::Event(Event::to_connection(id, EventKind::Feedback, text)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_proto::Audience;

    #[test]
    fn connect_greets() {
        let mut s = Sessions::default();
        let id = ConnectionId(1);
        let mut out = Vec::new();
        s.connect(id, &mut |o| out.push(o));
        assert!(matches!(
            out.as_slice(),
            [Outgoing::Event(Event { kind: EventKind::System, to: Audience::Connection(c), .. })] if *c == id
        ));
        assert!(s.is_live(id));
    }

    #[test]
    fn quit_emits_close() {
        let mut s = Sessions::default();
        let world = World::new();
        let mut actors = Actors::default();
        let id = ConnectionId(7);
        s.connect(id, &mut |_| {});

        let mut out = Vec::new();
        s.account_command(id, "quit", &world, &mut actors, &mut |o| out.push(o));
        assert!(matches!(out[0], Outgoing::Event(Event { kind: EventKind::Feedback, .. })));
        assert!(matches!(out[1], Outgoing::Close(c) if c == id));
    }

    #[test]
    fn play_binds_to_the_seeded_avatar() {
        let mut s = Sessions::default();
        let mut world = World::new();
        let seeded = musce_action::seed(&mut world);
        let mut actors = Actors::default();
        let id = ConnectionId(3);
        s.connect(id, &mut |_| {});

        let mut out = Vec::new();
        s.account_command(id, "play", &world, &mut actors, &mut |o| out.push(o));
        assert!(matches!(out.as_slice(), [Outgoing::Event(Event { kind: EventKind::Feedback, .. })]));
        assert_eq!(actors.actor_of(id), Some(seeded.avatar));
    }

    #[test]
    fn unknown_at_command_feeds_back() {
        let mut s = Sessions::default();
        let world = World::new();
        let mut actors = Actors::default();
        let id = ConnectionId(2);
        s.connect(id, &mut |_| {});

        let mut out = Vec::new();
        s.account_command(id, "bogus", &world, &mut actors, &mut |o| out.push(o));
        match &out[..] {
            [Outgoing::Event(Event { kind: EventKind::Feedback, text, .. })] => {
                assert!(text.contains("bogus"));
            }
            other => panic!("expected one feedback event, got {other:?}"),
        }
    }
}
