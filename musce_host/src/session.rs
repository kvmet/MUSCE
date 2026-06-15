//! The session floor: the always-present bottom of the input stack. Once a
//! connection is authenticated it has a session here, and `@`-namespaced account
//! commands route to it no matter what is overlaid on top. Auth is still stubbed
//! (every connection is an anonymous guest).
//!
//! The floor includes `@play`, which binds the connection to a character so bare
//! commands have something to act through. Which character is game policy,
//! injected as the game's `choose_actor`; the floor only renders the
//! confirmation. The binding is session state, not world state; the driven actor
//! is resolved live from the character's `Focus` (`actor =
//! focus_of(character).unwrap_or(character)`), so durable `Controls`/`Focus`
//! embodiment backs the binding without the floor knowing. See
//! `docs/architecture/networking-and-sessions.md` and
//! `docs/architecture/engine-and-game.md`.

use std::collections::HashMap;

use musce_action::Actors;
use musce_core::{EntityId, World};
use musce_proto::{ConnectionId, Event, EventKind, Outgoing};

use crate::ChooseActor;

/// One live session. Holds the connection's attachment: the character it drives,
/// set by `@play`. The driven actor is resolved live from that character's
/// `Focus` (see [`resolve_actor`]), so piloting redirects bare commands without
/// changing this attachment. It is session state (connections are ephemeral, so
/// it cannot live in the world); it will grow to hold the account id and the
/// character slots.
#[derive(Default)]
struct Session {
    character: Option<EntityId>,
}

/// Resolve the entity a character's bare commands drive: the entity it is
/// piloting (its `Focus`) if that is live, otherwise the character itself. Read
/// live so a `pilot` that moves `Focus` redirects subsequent commands at once.
///
/// The liveness check is a **defensive backstop only**: a focused entity's
/// despawn clears the cursor through the `Focus` `Detach` cascade, so a `Focus`
/// aimed at a despawned entity means corrupt or partially loaded state, not
/// ordinary play. We log it rather than hand a verb a dead actor, and never
/// silently paper over the dangling pointer.
pub(crate) fn resolve_actor(world: &World, character: EntityId) -> EntityId {
    match world.focus_of(character) {
        Some(focus) if world.entity(focus).is_some() => focus,
        Some(dangling) => {
            tracing::warn!(
                ?character,
                ?dangling,
                "focus aimed at a despawned entity; resolving to the character"
            );
            character
        }
        None => character,
    }
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

    /// The character a connection drives, if it has attached via `@play`. The
    /// live actor is derived from this through [`resolve_actor`].
    pub fn character_of(&self, id: ConnectionId) -> Option<EntityId> {
        self.map.get(&id).and_then(|s| s.character)
    }

    /// Attach a connection to the character its bare commands will drive.
    fn attach(&mut self, id: ConnectionId, character: EntityId) {
        if let Some(s) = self.map.get_mut(&id) {
            s.character = Some(character);
        }
    }

    /// Build the audience index the action layer's resolver consumes: the
    /// conn->actor view derived from the current attachments, each resolved
    /// through its character's `Focus`. Transient, rebuilt per dispatch; the
    /// attachments plus `Focus` are the source of truth.
    pub fn audience_index(&self, world: &World) -> Actors {
        let mut actors = Actors::default();
        for (&id, session) in &self.map {
            if let Some(character) = session.character {
                actors.bind(id, resolve_actor(world, character));
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
        // audience index the resolver consumes. With no Focus, the actor is the
        // character itself.
        assert_eq!(s.character_of(id), Some(avatar));
        assert!(s.audience_index(&world).conns_for(avatar).any(|c| c == id));
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

    #[test]
    fn resolve_actor_without_focus_is_the_character() {
        let mut world = World::new();
        let character = spawn_avatar(&mut world);
        assert_eq!(resolve_actor(&world, character), character);
    }

    #[test]
    fn resolve_actor_follows_live_focus() {
        let mut world = World::new();
        let character = spawn_avatar(&mut world);
        let robot = spawn_avatar(&mut world);
        world.set_focus(character, robot).unwrap();
        assert_eq!(resolve_actor(&world, character), robot);
    }

    /// The defensive backstop: a `Focus` pointing at an entity that is not in the
    /// world (a corrupt or partially loaded snapshot, since a normal despawn would
    /// have cleared it via the cascade) resolves to the character, never the dead
    /// actor. Built by loading a blob whose `focus` link references an absent id.
    #[test]
    fn resolve_actor_backstop_falls_back_on_dangling_focus() {
        use musce_core::{EntityBlob, Map, Value};

        let character = EntityId(1);
        let ghost = EntityId(9999);

        let mut data = Map::new();
        data.insert("id".into(), Value::from(1u64));
        data.insert("player".into(), Value::Null);
        data.insert("description".into(), Value::from("a pilot"));
        data.insert("focus".into(), Value::from(9999u64));

        let mut world = World::new();
        world
            .load(
                &[EntityBlob {
                    id: character,
                    zone: None,
                    data: Value::Object(data),
                }],
                10_000,
            )
            .unwrap();

        assert_eq!(world.focus_of(character), Some(ghost));
        assert!(world.entity(ghost).is_none());
        assert_eq!(resolve_actor(&world, character), character);
    }
}
