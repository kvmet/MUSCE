//! The session floor: the always-present bottom of the input stack. Once a
//! connection is authenticated it has a session here, and `@`-namespaced account
//! commands route to it no matter what is overlaid on top. Auth is still stubbed
//! (every connection is an anonymous guest).
//!
//! The floor includes `@play`, which binds the connection to an actor so bare
//! commands have something to act through. Which actor is game policy, injected
//! as the game's `bind_actor`; the floor only renders the confirmation. The
//! binding is session state (held in `musce_action::Actors`), not world state;
//! the next increment replaces its body with the persisted `Controls`/`Focus`
//! flow without touching this floor. See
//! `docs/architecture/networking-and-sessions.md` and
//! `docs/architecture/engine-and-game.md`.

use std::collections::HashMap;

use musce_action::Actors;
use musce_core::{EntityId, World};
use musce_proto::{ConnectionId, Event, EventKind, Outgoing};

use crate::ChooseActor;

/// One live session. Holds the connection's attachment: the actor its bare
/// commands act through, set by `@play`. This attachment is session state
/// (connections are ephemeral, so it cannot live in the world); it will grow to
/// hold the account id and character slots.
#[derive(Default)]
struct Session {
    actor: Option<EntityId>,
}

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
        self.map.insert(id, Session::default());
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

    /// The actor a connection currently drives, if it has attached via `@play`.
    pub fn actor_of(&self, id: ConnectionId) -> Option<EntityId> {
        self.map.get(&id).and_then(|s| s.actor)
    }

    /// Attach a connection to the actor its bare commands will drive.
    fn attach(&mut self, id: ConnectionId, actor: EntityId) {
        if let Some(s) = self.map.get_mut(&id) {
            s.actor = Some(actor);
        }
    }

    /// Build the audience index the action layer's resolver consumes: the
    /// conn->actor view derived from the current attachments. Transient, rebuilt
    /// per dispatch; the attachments here are the source of truth.
    pub fn audience_index(&self) -> Actors {
        let mut actors = Actors::default();
        for (&id, session) in &self.map {
            if let Some(actor) = session.actor {
                actors.bind(id, actor);
            }
        }
        actors
    }

    /// Handle one `@`-namespaced account command (the leading `@` already
    /// stripped). These are the floor and stay reachable regardless of what sits
    /// on top of the input stack.
    pub fn account_command(
        &mut self,
        id: ConnectionId,
        rest: &str,
        world: &World,
        choose_actor: ChooseActor,
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
                feedback(
                    id,
                    &format!("{} connection(s) online.", self.online_count()),
                    emit,
                );
            }
            "help" => {
                // The floor documents only its own account commands; in-game
                // verbs are the game's surface, not the engine's to enumerate.
                feedback(id, "Commands: @play, @quit, @who, @help.", emit);
            }
            "play" => self.play(id, world, choose_actor, emit),
            other => {
                feedback(id, &format!("Unknown command: @{other}"), emit);
            }
        }
    }

    /// `@play`: attach this connection to an actor so its bare commands have
    /// something to act through. Which actor is the game's policy, injected as
    /// `choose_actor`; the floor records the attachment (session state) and
    /// renders the confirmation. Durable embodiment (`Controls`/`Focus`) will
    /// back the choice later without touching this floor.
    fn play(
        &mut self,
        id: ConnectionId,
        world: &World,
        choose_actor: ChooseActor,
        emit: &mut impl FnMut(Outgoing),
    ) {
        match choose_actor(world) {
            Some(actor) => {
                self.attach(id, actor);
                let name =
                    musce_action::actor_name(world, actor).unwrap_or_else(|| "someone".into());
                feedback(id, &format!("You are now {name}."), emit);
            }
            None => feedback(id, "There is no character to play yet.", emit),
        }
    }
}

fn feedback(id: ConnectionId, text: &str, emit: &mut impl FnMut(Outgoing)) {
    emit(Outgoing::Event(Event::to_connection(
        id,
        EventKind::Feedback,
        text,
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Description, EntityId, Id, Player};
    use musce_proto::Audience;

    /// An engine-only `@play` policy for tests: choose the first `Player` in the
    /// world. Stands in for a game's injected `choose_actor`.
    fn first_player_choose(world: &World) -> Option<EntityId> {
        world
            .ecs
            .query::<(&Id, &Player)>()
            .iter()
            .next()
            .map(|(id, _)| id.0)
    }

    /// Spawn a lone described player avatar and return it.
    fn spawn_avatar(world: &mut World) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Player);
        b.add(Description("a tester".into()));
        world.spawn(b)
    }

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
        let id = ConnectionId(7);
        s.connect(id, &mut |_| {});

        let mut out = Vec::new();
        s.account_command(id, "quit", &world, first_player_choose, &mut |o| {
            out.push(o)
        });
        assert!(matches!(
            out[0],
            Outgoing::Event(Event {
                kind: EventKind::Feedback,
                ..
            })
        ));
        assert!(matches!(out[1], Outgoing::Close(c) if c == id));
    }

    #[test]
    fn play_attaches_through_the_injected_policy() {
        let mut s = Sessions::default();
        let mut world = World::new();
        let avatar = spawn_avatar(&mut world);
        let id = ConnectionId(3);
        s.connect(id, &mut |_| {});

        let mut out = Vec::new();
        s.account_command(id, "play", &world, first_player_choose, &mut |o| {
            out.push(o)
        });
        assert!(matches!(
            out.as_slice(),
            [Outgoing::Event(Event {
                kind: EventKind::Feedback,
                ..
            })]
        ));
        // The attachment is recorded as session state, and surfaces in the
        // audience index the resolver consumes.
        assert_eq!(s.actor_of(id), Some(avatar));
        assert!(s.audience_index().conns_for(avatar).any(|c| c == id));
    }

    #[test]
    fn unknown_at_command_feeds_back() {
        let mut s = Sessions::default();
        let world = World::new();
        let id = ConnectionId(2);
        s.connect(id, &mut |_| {});

        let mut out = Vec::new();
        s.account_command(id, "bogus", &world, first_player_choose, &mut |o| {
            out.push(o)
        });
        match &out[..] {
            [
                Outgoing::Event(Event {
                    kind: EventKind::Feedback,
                    text,
                    ..
                }),
            ] => {
                assert!(text.contains("bogus"));
            }
            other => panic!("expected one feedback event, got {other:?}"),
        }
    }
}
