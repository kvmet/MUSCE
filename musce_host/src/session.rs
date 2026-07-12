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
use std::net::SocketAddr;

use musce_action::Actors;
use musce_core::{EntityId, World};
use musce_proto::{ConnectionId, Event, EventKind, Outgoing};

use crate::ChooseActor;
use crate::auth::{AccountId, Accounts, CapRegistry};

/// One live session: the per-connection state the floor owns. The character it
/// drives (set by `@play`; the driven actor is resolved live from its `Focus`, see
/// [`resolve_actor`]), the account it is authorized as (thin: an id, the authority
/// holds the grants), whether superuser is suppressed for this connection
/// (`@quell`), and whether its peer is loopback (gates the slice-1 operator stub).
/// All of it is session state, since connections are ephemeral and cannot live in
/// the world.
#[derive(Default)]
struct Session {
    character: Option<EntityId>,
    account: Option<AccountId>,
    quelled: bool,
    loopback: bool,
}

/// Resolve the entity a character's bare commands drive: the entity it is
/// piloting (its `Focus`) if that is live, otherwise the character itself. Read
/// live so a `pilot` that moves `Focus` redirects subsequent commands at once.
/// `World::control_root` is the inverse, walking from a driven puppet back to its
/// character.
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
    /// Allocate a session for a freshly connected client and greet it. `peer` is the
    /// connection's remote address (`None` for an in-process connection); a loopback
    /// peer is what gates the slice-1 operator stub, so it is recorded now.
    pub fn connect(
        &mut self,
        id: ConnectionId,
        peer: Option<SocketAddr>,
        emit: &mut impl FnMut(Outgoing),
    ) {
        let loopback = peer.is_some_and(|p| p.ip().is_loopback());
        self.map.insert(
            id,
            Session {
                loopback,
                ..Default::default()
            },
        );
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

    /// The account a connection is authorized as, if any. `None` is a guest:
    /// `Open`-only, no caps, no su. The authority resolves this to a verdict.
    pub fn account_of(&self, id: ConnectionId) -> Option<AccountId> {
        self.map.get(&id).and_then(|s| s.account)
    }

    /// Whether superuser is suppressed for this connection (`@quell`). A quelled
    /// connection is evaluated on its account's actual caps, su set aside.
    pub fn is_quelled(&self, id: ConnectionId) -> bool {
        self.map.get(&id).is_some_and(|s| s.quelled)
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
    /// on top of the input stack. Returns whether the floor recognized the verb;
    /// the floor is the single authority on its own verbs, so an unrecognized one
    /// returns `false` and the caller routes it onward (to the admin table).
    // The floor's single entry point coordinates several host-owned pieces (the
    // world, the account authority and its cap registry, the game's actor policy);
    // like `dispatch_command`, it carries them as parameters rather than bundling.
    #[allow(clippy::too_many_arguments)]
    pub fn account_command(
        &mut self,
        id: ConnectionId,
        rest: &str,
        world: &World,
        accounts: &mut Accounts,
        registry: &CapRegistry,
        choose_actor: ChooseActor,
        emit: &mut impl FnMut(Outgoing),
    ) -> bool {
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
                // The floor documents only its own account commands; in-game and
                // admin verbs are the game's surface, not the engine's to enumerate.
                feedback(
                    id,
                    "Commands: @play, @operator, @login, @account, @grant, @revoke, \
                     @quell, @quit, @who, @help.",
                    emit,
                );
            }
            "play" => self.play(id, world, choose_actor, emit),
            "operator" => self.operator(id, accounts, emit),
            "login" => self.login(id, parts.next().unwrap_or(""), accounts, emit),
            "account" => {
                let sub = parts.next().unwrap_or("");
                let handle = parts.next().unwrap_or("");
                self.account_admin(id, sub, handle, accounts, emit);
            }
            "grant" => {
                let handle = parts.next().unwrap_or("");
                let cap = parts.next().unwrap_or("");
                self.grant_cap(id, handle, cap, accounts, registry, emit);
            }
            "revoke" => {
                let handle = parts.next().unwrap_or("");
                let cap = parts.next().unwrap_or("");
                self.revoke_cap(id, handle, cap, accounts, registry, emit);
            }
            "quell" => self.quell(id, emit),
            _ => return false,
        }
        true
    }

    /// `@operator`: the slice-1 authentication stub. Attaches the connection to the
    /// seeded superuser operator account, but **loopback-only**: a remote or
    /// in-process connection is refused, so an unauthenticated god-mode is never
    /// reachable over a bound port. Slice 2 replaces this with a real credential
    /// check resolving to the same account.
    fn operator(&mut self, id: ConnectionId, accounts: &Accounts, emit: &mut impl FnMut(Outgoing)) {
        if !self.map.get(&id).is_some_and(|s| s.loopback) {
            feedback(id, "Operator elevation is only available locally.", emit);
            return;
        }
        match accounts.stub_operator() {
            Some(op) => {
                if let Some(s) = self.map.get_mut(&id) {
                    s.account = Some(op);
                }
                feedback(id, "You are now the operator.", emit);
            }
            None => feedback(id, "There is no operator account.", emit),
        }
    }

    /// `@login <handle>`: the slice-2a authentication stub. Attaches the connection to
    /// the account with that handle, **loopback-only** like `@operator` since it takes
    /// no credential yet. The real credential check that lets this run remotely is the
    /// next (small) slice; it resolves the same handle to the same account, no shape
    /// change here.
    fn login(
        &mut self,
        id: ConnectionId,
        handle: &str,
        accounts: &Accounts,
        emit: &mut impl FnMut(Outgoing),
    ) {
        if !self.map.get(&id).is_some_and(|s| s.loopback) {
            feedback(id, "Login is only available locally.", emit);
            return;
        }
        if handle.is_empty() {
            feedback(id, "Log in as whom? (@login <handle>)", emit);
            return;
        }
        match accounts.account_by_handle(handle) {
            Some(acc) => {
                if let Some(s) = self.map.get_mut(&id) {
                    s.account = Some(acc);
                }
                feedback(id, &format!("You are now logged in as {handle}."), emit);
            }
            None => feedback(id, &format!("No account named \"{handle}\"."), emit),
        }
    }

    /// `@account new <handle>`: create a plain (non-su) account. Operator-only: an
    /// account without su in force is refused. This is the runtime account-creation
    /// surface the composable model needs to hold a second account.
    fn account_admin(
        &mut self,
        id: ConnectionId,
        sub: &str,
        handle: &str,
        accounts: &mut Accounts,
        emit: &mut impl FnMut(Outgoing),
    ) {
        if !self.is_operator(id, accounts) {
            feedback(id, "Only the operator may administer accounts.", emit);
            return;
        }
        if sub != "new" {
            feedback(id, "Usage: @account new <handle>.", emit);
            return;
        }
        if handle.is_empty() {
            feedback(id, "Name the new account: @account new <handle>.", emit);
            return;
        }
        if accounts.account_by_handle(handle).is_some() {
            feedback(
                id,
                &format!("An account named \"{handle}\" already exists."),
                emit,
            );
            return;
        }
        accounts.create_account(handle);
        feedback(id, &format!("Created account \"{handle}\"."), emit);
    }

    /// `@grant <handle> <capability>`: add a capability to an account. Operator-only.
    fn grant_cap(
        &mut self,
        id: ConnectionId,
        handle: &str,
        cap: &str,
        accounts: &mut Accounts,
        registry: &CapRegistry,
        emit: &mut impl FnMut(Outgoing),
    ) {
        if !self.is_operator(id, accounts) {
            feedback(id, "Only the operator may grant capabilities.", emit);
            return;
        }
        if handle.is_empty() || cap.is_empty() {
            feedback(id, "Usage: @grant <handle> <capability>.", emit);
            return;
        }
        let Some(acc) = accounts.account_by_handle(handle) else {
            feedback(id, &format!("No account named \"{handle}\"."), emit);
            return;
        };
        match accounts.grant(acc, cap, registry) {
            Ok(()) => feedback(id, &format!("Granted \"{cap}\" to {handle}."), emit),
            Err(e) => feedback(id, &format!("Can't grant that: {e}."), emit),
        }
    }

    /// `@revoke <handle> <capability>`: remove a capability from an account.
    /// Operator-only.
    fn revoke_cap(
        &mut self,
        id: ConnectionId,
        handle: &str,
        cap: &str,
        accounts: &mut Accounts,
        registry: &CapRegistry,
        emit: &mut impl FnMut(Outgoing),
    ) {
        if !self.is_operator(id, accounts) {
            feedback(id, "Only the operator may revoke capabilities.", emit);
            return;
        }
        if handle.is_empty() || cap.is_empty() {
            feedback(id, "Usage: @revoke <handle> <capability>.", emit);
            return;
        }
        let Some(acc) = accounts.account_by_handle(handle) else {
            feedback(id, &format!("No account named \"{handle}\"."), emit);
            return;
        };
        match accounts.revoke(acc, cap, registry) {
            Ok(()) => feedback(id, &format!("Revoked \"{cap}\" from {handle}."), emit),
            Err(e) => feedback(id, &format!("Can't revoke that: {e}."), emit),
        }
    }

    /// Whether this connection acts with superuser in force: an operator managing
    /// accounts. Reads the same verdict a gate would, so a quelled operator is refused
    /// (quell means "act as a normal player"), consistent with the gate check.
    fn is_operator(&self, id: ConnectionId, accounts: &Accounts) -> bool {
        accounts
            .verdict_for(self.account_of(id), self.is_quelled(id))
            .is_su()
    }

    /// `@quell`: toggle superuser suppression for this connection. Quelled, an su
    /// account is evaluated on its actual caps; toggling again restores su. Per
    /// connection and ephemeral, so a fresh connection always starts with su live and
    /// there is no lockout.
    fn quell(&mut self, id: ConnectionId, emit: &mut impl FnMut(Outgoing)) {
        if let Some(s) = self.map.get_mut(&id) {
            s.quelled = !s.quelled;
            let msg = if s.quelled {
                "Superuser suppressed for this connection. @quell again to restore."
            } else {
                "Superuser restored for this connection."
            };
            feedback(id, msg, emit);
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
    use crate::auth::{CapRegistry, MemoryAccountStore};
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Controls, Description, EntityId, Id};
    use musce_proto::Audience;

    /// A local stand-in for a game's player kind: the engine has no `Player`
    /// concept, so these tests define their own marker to pick an actor by.
    struct Avatar;

    /// An account authority with one bootstrapped su operator, for the `@operator`
    /// and `@quell` floor commands.
    fn accounts() -> Accounts {
        Accounts::boot(&MemoryAccountStore::new(), &CapRegistry::new()).unwrap()
    }

    /// A loopback peer, so `@operator` elevation is available.
    fn loopback() -> Option<std::net::SocketAddr> {
        Some("127.0.0.1:9000".parse().unwrap())
    }

    /// An engine-only `@play` policy for tests: choose the first `Avatar` in the
    /// world. Stands in for a game's injected `choose_actor`.
    fn first_player_choose(world: &World) -> Option<EntityId> {
        world
            .ecs
            .query::<(&Id, &Avatar)>()
            .iter()
            .next()
            .map(|(id, _)| id.0)
    }

    /// Spawn a lone described player avatar and return it.
    fn spawn_avatar(world: &mut World) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Avatar);
        b.add(Description("a tester".into()));
        world.spawn(b)
    }

    #[test]
    fn connect_greets() {
        let mut s = Sessions::default();
        let id = ConnectionId(1);
        let mut out = Vec::new();
        s.connect(id, None, &mut |o| out.push(o));
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
        let mut accounts = accounts();
        let reg = CapRegistry::new();
        let id = ConnectionId(7);
        s.connect(id, None, &mut |_| {});

        let mut out = Vec::new();
        s.account_command(
            id,
            "quit",
            &world,
            &mut accounts,
            &reg,
            first_player_choose,
            &mut |o| out.push(o),
        );
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
        let mut accounts = accounts();
        let reg = CapRegistry::new();
        let avatar = spawn_avatar(&mut world);
        let id = ConnectionId(3);
        s.connect(id, None, &mut |_| {});

        let mut out = Vec::new();
        s.account_command(
            id,
            "play",
            &world,
            &mut accounts,
            &reg,
            first_player_choose,
            &mut |o| out.push(o),
        );
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
    fn unknown_at_command_is_unhandled_and_silent() {
        let mut s = Sessions::default();
        let world = World::new();
        let mut accounts = accounts();
        let reg = CapRegistry::new();
        let id = ConnectionId(2);
        s.connect(id, None, &mut |_| {});

        // The floor does not recognize it: it reports "unhandled" and emits
        // nothing, leaving the caller to route it to the admin table.
        let mut out = Vec::new();
        let handled = s.account_command(
            id,
            "bogus",
            &world,
            &mut accounts,
            &reg,
            first_player_choose,
            &mut |o| out.push(o),
        );
        assert!(!handled);
        assert!(out.is_empty());
    }

    #[test]
    fn lifecycle_command_is_handled() {
        let mut s = Sessions::default();
        let world = World::new();
        let mut accounts = accounts();
        let reg = CapRegistry::new();
        let id = ConnectionId(2);
        s.connect(id, None, &mut |_| {});
        let handled = s.account_command(
            id,
            "who",
            &world,
            &mut accounts,
            &reg,
            first_player_choose,
            &mut |_| {},
        );
        assert!(handled);
    }

    #[test]
    fn operator_attaches_only_from_loopback() {
        let mut s = Sessions::default();
        let world = World::new();
        let mut accounts = accounts();
        let reg = CapRegistry::new();
        let op = accounts.stub_operator().expect("a bootstrapped operator");

        // A loopback peer may elevate to the operator account.
        let local = ConnectionId(1);
        s.connect(local, loopback(), &mut |_| {});
        s.account_command(
            local,
            "operator",
            &world,
            &mut accounts,
            &reg,
            first_player_choose,
            &mut |_| {},
        );
        assert_eq!(s.account_of(local), Some(op));

        // A peerless (in-process) connection is refused: no account attached.
        let remote = ConnectionId(2);
        s.connect(remote, None, &mut |_| {});
        s.account_command(
            remote,
            "operator",
            &world,
            &mut accounts,
            &reg,
            first_player_choose,
            &mut |_| {},
        );
        assert_eq!(s.account_of(remote), None);
    }

    #[test]
    fn quell_toggles_suppression() {
        let mut s = Sessions::default();
        let world = World::new();
        let mut accounts = accounts();
        let reg = CapRegistry::new();
        let id = ConnectionId(1);
        s.connect(id, loopback(), &mut |_| {});

        assert!(!s.is_quelled(id));
        s.account_command(
            id,
            "quell",
            &world,
            &mut accounts,
            &reg,
            first_player_choose,
            &mut |_| {},
        );
        assert!(s.is_quelled(id));
        s.account_command(
            id,
            "quell",
            &world,
            &mut accounts,
            &reg,
            first_player_choose,
            &mut |_| {},
        );
        assert!(!s.is_quelled(id));
    }

    #[test]
    fn login_attaches_to_an_account_by_handle() {
        let mut s = Sessions::default();
        let world = World::new();
        let mut accounts = accounts();
        let reg = CapRegistry::new();
        let builder = accounts.create_account("builder");
        let id = ConnectionId(1);
        s.connect(id, loopback(), &mut |_| {});

        s.account_command(
            id,
            "login builder",
            &world,
            &mut accounts,
            &reg,
            first_player_choose,
            &mut |_| {},
        );
        assert_eq!(s.account_of(id), Some(builder));
    }

    #[test]
    fn login_refused_from_a_non_loopback_peer() {
        let mut s = Sessions::default();
        let world = World::new();
        let mut accounts = accounts();
        let reg = CapRegistry::new();
        accounts.create_account("builder");
        let id = ConnectionId(1);
        s.connect(id, None, &mut |_| {});

        s.account_command(
            id,
            "login builder",
            &world,
            &mut accounts,
            &reg,
            first_player_choose,
            &mut |_| {},
        );
        assert_eq!(
            s.account_of(id),
            None,
            "a peerless connection cannot log in via the stub"
        );
    }

    #[test]
    fn account_admin_requires_the_operator() {
        let mut s = Sessions::default();
        let world = World::new();
        let mut accounts = accounts();
        let reg = CapRegistry::new();
        let id = ConnectionId(1);
        // Connected but never elevated: a guest cannot create accounts.
        s.connect(id, loopback(), &mut |_| {});

        s.account_command(
            id,
            "account new builder",
            &world,
            &mut accounts,
            &reg,
            first_player_choose,
            &mut |_| {},
        );
        assert_eq!(
            accounts.account_by_handle("builder"),
            None,
            "a guest cannot create accounts"
        );
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
        world.relate::<Controls>(robot, character).unwrap();
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
