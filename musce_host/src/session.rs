//! The session floor: the always-present bottom of the input stack. Once a
//! connection is authenticated it has an account bound here, and `@`-namespaced
//! account commands route to it no matter what is overlaid on top. Authentication
//! is asynchronous: the loopback `@operator` bootstrap and the password-checked
//! `@login` raise an authenticate op the account task resolves off-thread, and the
//! resulting authorization (account, resolved caps, su) is cached on the session
//! when the outcome lands.
//!
//! The floor also includes `@play`, which binds the connection to a character so
//! bare commands have something to act through. Which character is game policy,
//! injected as the game's `choose_actor`; the floor only renders the confirmation.
//! The binding is session state, not world state; the driven actor is resolved live
//! from the character's `Focus`. See `docs/architecture/networking-and-sessions.md`
//! and `docs/architecture/authorization.md`.

use std::collections::HashMap;
use std::net::SocketAddr;

use musce_action::{Actors, CapSet, Verdict};
use musce_auth::AccountId;
use musce_core::{EntityId, World};
use musce_proto::{ConnectionId, Delivery, EventKind, Outgoing};

use crate::ChooseActor;
use crate::accounts::{AccountOp, Authorization, OPERATOR_USERNAME};

/// One live session: the per-connection state the floor owns. The character it
/// drives (set by `@play`), the account it is authenticated as plus that account's
/// cached authorization (resolved caps and su, filled by the account task on login),
/// whether superuser is suppressed for this connection (`@quell`), whether its peer
/// is loopback (gates the operator/login stubs), and whether an authentication is in
/// flight (a pending connection runs nothing else). All session state, since
/// connections are ephemeral and cannot live in the world.
#[derive(Default)]
struct Session {
    character: Option<EntityId>,
    account: Option<AccountId>,
    caps: CapSet,
    su: bool,
    quelled: bool,
    loopback: bool,
    pending_auth: bool,
}

/// Resolve the entity a character's bare commands drive: the entity it is
/// piloting (its `Focus`) if that is live, otherwise the character itself. Read
/// live so a `pilot` that moves `Focus` redirects subsequent commands at once.
///
/// The liveness check is a **defensive backstop only**: a focused entity's
/// despawn clears the cursor through the `Focus` `Detach` cascade, so a `Focus`
/// aimed at a despawned entity means corrupt or partially loaded state, not
/// ordinary play. We log it rather than hand a verb a dead actor.
pub(crate) fn resolve_actor(world: &World, character: EntityId) -> EntityId {
    match world.focus_of(character) {
        Some(focus) if world.contains(focus) => focus,
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
/// `@`-namespace.
#[derive(Default)]
pub struct Sessions {
    map: HashMap<ConnectionId, Session>,
}

impl Sessions {
    /// Allocate a session for a freshly connected client and greet it. `peer` is the
    /// remote address (`None` for an in-process connection); a loopback peer gates
    /// the operator/login stubs, so it is recorded now.
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
        emit(Outgoing::Event(Delivery::new(
            id,
            EventKind::System,
            "Welcome to MUSCE. @play to enter the world, @help for commands.",
        )));
    }

    /// Tear a session down. The caller also clears the actor binding.
    pub fn disconnect(&mut self, id: ConnectionId) {
        self.map.remove(&id);
    }

    /// Whether this connection has a live session.
    pub fn is_live(&self, id: ConnectionId) -> bool {
        self.map.contains_key(&id)
    }

    pub fn online_count(&self) -> usize {
        self.map.len()
    }

    /// The character a connection drives, if it has attached via `@play`.
    pub fn character_of(&self, id: ConnectionId) -> Option<EntityId> {
        self.map.get(&id).and_then(|s| s.character)
    }

    /// Whether an authentication is in flight for this connection.
    pub fn is_pending(&self, id: ConnectionId) -> bool {
        self.map.get(&id).is_some_and(|s| s.pending_auth)
    }

    /// Clear the pending-auth flag, whatever the authentication outcome. A refused
    /// login must lift it too, or the connection would be wedged, unable to retry.
    pub fn clear_pending(&mut self, id: ConnectionId) {
        if let Some(s) = self.map.get_mut(&id) {
            s.pending_auth = false;
        }
    }

    /// The authorization verdict this connection runs under, from its cached account
    /// caps and su with quell applied. A connection with no session, or none bound to
    /// an account, resolves to the guest verdict.
    pub fn verdict_of(&self, id: ConnectionId) -> Verdict {
        match self.map.get(&id) {
            Some(s) => Verdict::resolved(s.caps.clone(), s.su, s.quelled),
            None => Verdict::guest(),
        }
    }

    /// Bind a connection to an authenticated account, caching its resolved
    /// authorization and clearing the pending flag. Applied from an account-task
    /// `Authenticated` outcome.
    pub fn bind(&mut self, id: ConnectionId, authz: Authorization) {
        if let Some(s) = self.map.get_mut(&id) {
            s.account = Some(authz.account);
            s.caps = authz.caps;
            s.su = authz.su;
            s.pending_auth = false;
        }
    }

    /// Refresh the cached authorization of every connection bound to `authz.account`.
    /// Applied from a grant/revoke outcome so an online target sees the change at
    /// once, without re-authenticating.
    pub fn refresh(&mut self, authz: Authorization) {
        for s in self.map.values_mut() {
            if s.account == Some(authz.account) {
                s.caps = authz.caps.clone();
                s.su = authz.su;
            }
        }
    }

    /// Attach a connection to the character its bare commands will drive.
    fn attach(&mut self, id: ConnectionId, character: EntityId) {
        if let Some(s) = self.map.get_mut(&id) {
            s.character = Some(character);
        }
    }

    /// Build the audience index the action layer's resolver consumes.
    pub fn audience_index(&self, world: &World) -> Actors {
        let mut actors = Actors::default();
        for (&id, session) in &self.map {
            if let Some(character) = session.character {
                actors.bind(id, resolve_actor(world, character));
            }
        }
        actors
    }

    /// Handle one `@`-namespaced account command (the leading `@` already stripped).
    /// Sync verbs (`@quit`/`@who`/`@help`/`@play`/`@quell`) act inline; the async
    /// account verbs push an [`AccountOp`] into `ops` for the account task and their
    /// confirmation returns later as an outcome. Returns whether the floor recognized
    /// the verb; an unrecognized one returns `false` for the caller to route onward.
    pub fn account_command(
        &mut self,
        id: ConnectionId,
        rest: &str,
        world: &World,
        choose_actor: ChooseActor,
        ops: &mut Vec<AccountOp>,
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
                feedback(
                    id,
                    "Commands: @play, @operator, @login, @password, @account, @grant, \
                     @revoke, @quell, @quit, @who, @help.",
                    emit,
                );
            }
            "play" => self.play(id, world, choose_actor, emit),
            "operator" => self.operator(id, ops, emit),
            "login" => {
                let username = parts.next().unwrap_or("");
                let password = parts.next();
                self.login(id, username, password, ops, emit);
            }
            "password" | "pw" => {
                let old = parts.next();
                let new = parts.next();
                self.change_password(id, old, new, ops, emit);
            }
            "account" => {
                let sub = parts.next().unwrap_or("");
                let username = parts.next().unwrap_or("");
                let password = parts.next();
                self.account_admin(id, sub, username, password, ops, emit);
            }
            "grant" => {
                let username = parts.next().unwrap_or("");
                let cap = parts.next().unwrap_or("");
                self.grant_cap(id, username, cap, true, ops, emit);
            }
            "revoke" => {
                let username = parts.next().unwrap_or("");
                let cap = parts.next().unwrap_or("");
                self.grant_cap(id, username, cap, false, ops, emit);
            }
            "quell" => self.quell(id, emit),
            _ => return false,
        }
        true
    }

    /// Mark the connection pending and raise an authenticate op. `password` is `None`
    /// only for the loopback `@operator` bootstrap; `@login` always carries one.
    fn begin_auth(
        &mut self,
        id: ConnectionId,
        username: String,
        password: Option<String>,
        ops: &mut Vec<AccountOp>,
    ) {
        if let Some(s) = self.map.get_mut(&id) {
            s.pending_auth = true;
        }
        ops.push(AccountOp::Authenticate {
            conn: id,
            username,
            password,
        });
    }

    /// `@operator`: loopback-only elevation to the bootstrap superuser. Raises a
    /// passwordless authenticate op for the seeded `operator` account; a remote or
    /// in-process connection is refused, so unauthenticated god-mode is never
    /// reachable over a bound port.
    fn operator(
        &mut self,
        id: ConnectionId,
        ops: &mut Vec<AccountOp>,
        emit: &mut impl FnMut(Outgoing),
    ) {
        if !self.map.get(&id).is_some_and(|s| s.loopback) {
            feedback(id, "Operator elevation is only available locally.", emit);
            return;
        }
        self.begin_auth(id, OPERATOR_USERNAME.to_string(), None, ops);
    }

    /// `@login <username> <password>`: authenticate as `username`, verified against
    /// the stored credential by the account task. Unlike `@operator` this is not
    /// loopback-gated: a real password is the credential, so it works from anywhere.
    fn login(
        &mut self,
        id: ConnectionId,
        username: &str,
        password: Option<&str>,
        ops: &mut Vec<AccountOp>,
        emit: &mut impl FnMut(Outgoing),
    ) {
        if username.is_empty() || password.is_none() {
            feedback(id, "Log in: @login <username> <password>.", emit);
            return;
        }
        self.begin_auth(id, username.to_string(), password.map(str::to_string), ops);
    }

    /// `@password <old> <new>` (alias `@pw`): change the password on this connection's
    /// own account. Requires an authenticated connection; the account task verifies
    /// `old` against the stored credential before hashing and storing `new`. Not
    /// operator-gated: an account changes its own password.
    fn change_password(
        &self,
        id: ConnectionId,
        old: Option<&str>,
        new: Option<&str>,
        ops: &mut Vec<AccountOp>,
        emit: &mut impl FnMut(Outgoing),
    ) {
        let (Some(old), Some(new)) = (old, new) else {
            feedback(id, "Change your password: @password <old> <new>.", emit);
            return;
        };
        let Some(account) = self.map.get(&id).and_then(|s| s.account) else {
            feedback(id, "Log in before changing your password.", emit);
            return;
        };
        ops.push(AccountOp::SetPassword {
            conn: id,
            account,
            old: old.to_string(),
            new: new.to_string(),
        });
    }

    /// `@account new <username> <password>`: create an account with a password
    /// (hashed by the account task). Operator-only.
    fn account_admin(
        &mut self,
        id: ConnectionId,
        sub: &str,
        username: &str,
        password: Option<&str>,
        ops: &mut Vec<AccountOp>,
        emit: &mut impl FnMut(Outgoing),
    ) {
        if !self.is_operator(id) {
            feedback(id, "Only the operator may administer accounts.", emit);
            return;
        }
        if sub != "new" || username.is_empty() || password.is_none() {
            feedback(id, "Usage: @account new <username> <password>.", emit);
            return;
        }
        ops.push(AccountOp::Create {
            conn: id,
            username: username.to_string(),
            password: password.unwrap().to_string(),
        });
    }

    /// `@grant`/`@revoke <username> <capability>`: add or remove a capability on an
    /// account. Operator-only, raising a grant op (the account task validates the
    /// capability name against the vocabulary and refreshes an online target).
    fn grant_cap(
        &mut self,
        id: ConnectionId,
        username: &str,
        cap: &str,
        add: bool,
        ops: &mut Vec<AccountOp>,
        emit: &mut impl FnMut(Outgoing),
    ) {
        if !self.is_operator(id) {
            let action = if add { "grant" } else { "revoke" };
            feedback(
                id,
                &format!("Only the operator may {action} capabilities."),
                emit,
            );
            return;
        }
        if username.is_empty() || cap.is_empty() {
            let verb = if add { "@grant" } else { "@revoke" };
            feedback(id, &format!("Usage: {verb} <username> <capability>."), emit);
            return;
        }
        ops.push(AccountOp::Grant {
            conn: id,
            target: username.to_string(),
            cap: cap.to_string(),
            add,
        });
    }

    /// Whether this connection acts with superuser in force. Reads the same verdict a
    /// gate would, so a quelled operator is refused (quell means "act as a normal
    /// player"), consistent with the gate check.
    fn is_operator(&self, id: ConnectionId) -> bool {
        self.verdict_of(id).is_su()
    }

    /// `@quell`: toggle superuser suppression for this connection. Per connection and
    /// ephemeral, so a fresh connection always starts with its authority live.
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
    /// `choose_actor`; the floor records the attachment and renders the confirmation.
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
    emit(Outgoing::Event(Delivery::new(
        id,
        EventKind::Feedback,
        text,
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Controls, Description, EntityId, Id};

    struct Avatar;

    fn loopback() -> Option<std::net::SocketAddr> {
        Some("127.0.0.1:9000".parse().unwrap())
    }

    fn first_player_choose(world: &World) -> Option<EntityId> {
        world
            .query::<(&Id, &Avatar)>()
            .iter()
            .next()
            .map(|(id, _)| id.0)
    }

    fn spawn_avatar(world: &mut World) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Avatar);
        b.add(Description("a tester".into()));
        world.spawn(b)
    }

    /// Drive one account command, returning both the connection output and the ops
    /// it raised.
    fn cmd(
        s: &mut Sessions,
        world: &World,
        id: ConnectionId,
        rest: &str,
    ) -> (Vec<Outgoing>, Vec<AccountOp>) {
        let mut out = Vec::new();
        let mut ops = Vec::new();
        s.account_command(id, rest, world, first_player_choose, &mut ops, &mut |o| {
            out.push(o)
        });
        (out, ops)
    }

    fn su_authz() -> Authorization {
        Authorization {
            account: AccountId::generate(),
            caps: CapSet::new(),
            su: true,
        }
    }

    #[test]
    fn connect_greets() {
        let mut s = Sessions::default();
        let id = ConnectionId(1);
        let mut out = Vec::new();
        s.connect(id, None, &mut |o| out.push(o));
        assert!(matches!(
            out.as_slice(),
            [Outgoing::Event(Delivery { kind: EventKind::System, to: c, .. })] if *c == id
        ));
        assert!(s.is_live(id));
    }

    #[test]
    fn quit_emits_close() {
        let mut s = Sessions::default();
        let world = World::new();
        let id = ConnectionId(7);
        s.connect(id, None, &mut |_| {});
        let (out, _) = cmd(&mut s, &world, id, "quit");
        assert!(matches!(
            out[0],
            Outgoing::Event(Delivery {
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
        s.connect(id, None, &mut |_| {});
        cmd(&mut s, &world, id, "play");
        assert_eq!(s.character_of(id), Some(avatar));
        assert!(s.audience_index(&world).conns_for(avatar).any(|c| c == id));
    }

    #[test]
    fn unknown_at_command_is_unhandled_and_silent() {
        let mut s = Sessions::default();
        let world = World::new();
        let id = ConnectionId(2);
        s.connect(id, None, &mut |_| {});
        let mut out = Vec::new();
        let mut ops = Vec::new();
        let handled = s.account_command(
            id,
            "bogus",
            &world,
            first_player_choose,
            &mut ops,
            &mut |o| out.push(o),
        );
        assert!(!handled);
        assert!(out.is_empty() && ops.is_empty());
    }

    #[test]
    fn operator_raises_an_auth_op_only_from_loopback() {
        let mut s = Sessions::default();
        let world = World::new();

        // Loopback: raises an authenticate op for the operator account and marks
        // the connection pending.
        let local = ConnectionId(1);
        s.connect(local, loopback(), &mut |_| {});
        let (_, ops) = cmd(&mut s, &world, local, "operator");
        assert!(matches!(
            ops.as_slice(),
            [AccountOp::Authenticate { conn, username, password: None }]
                if *conn == local && username == OPERATOR_USERNAME
        ));
        assert!(s.is_pending(local));

        // Peerless: refused, no op, not pending.
        let remote = ConnectionId(2);
        s.connect(remote, None, &mut |_| {});
        let (out, ops) = cmd(&mut s, &world, remote, "operator");
        assert!(ops.is_empty() && !s.is_pending(remote));
        assert!(
            conn_texts(&out)
                .iter()
                .any(|t| t.contains("only available locally"))
        );
    }

    #[test]
    fn login_raises_an_auth_op_with_username_and_password() {
        let mut s = Sessions::default();
        let world = World::new();
        let id = ConnectionId(1);
        s.connect(id, loopback(), &mut |_| {});
        let (_, ops) = cmd(&mut s, &world, id, "login builder secret");
        assert!(matches!(
            ops.as_slice(),
            [AccountOp::Authenticate { username, password: Some(pw), .. }]
                if username == "builder" && pw == "secret"
        ));
        assert!(s.is_pending(id));
    }

    #[test]
    fn login_requires_a_password_but_is_not_loopback_gated() {
        let mut s = Sessions::default();
        let world = World::new();

        // No password: refused with usage, no op, regardless of peer.
        let bare = ConnectionId(1);
        s.connect(bare, loopback(), &mut |_| {});
        let (out, ops) = cmd(&mut s, &world, bare, "login builder");
        assert!(ops.is_empty() && !s.is_pending(bare));
        assert!(
            conn_texts(&out)
                .iter()
                .any(|t| t.contains("@login <username> <password>"))
        );

        // With a password, a peerless (non-loopback) connection still logs in:
        // password auth is not loopback-gated the way `@operator` is.
        let remote = ConnectionId(2);
        s.connect(remote, None, &mut |_| {});
        let (_, ops) = cmd(&mut s, &world, remote, "login builder secret");
        assert!(matches!(ops.as_slice(), [AccountOp::Authenticate { .. }]));
        assert!(s.is_pending(remote));
    }

    #[test]
    fn account_admin_and_grant_require_the_operator() {
        let mut s = Sessions::default();
        let world = World::new();
        let id = ConnectionId(1);
        s.connect(id, loopback(), &mut |_| {});

        // A guest (never bound to su) raises no op.
        let (_, ops) = cmd(&mut s, &world, id, "account new builder pw");
        assert!(ops.is_empty(), "a guest cannot create accounts");
        let (_, ops) = cmd(&mut s, &world, id, "grant builder build");
        assert!(ops.is_empty(), "a guest cannot grant");

        // Bound to su, the ops flow.
        s.bind(id, su_authz());
        let (_, ops) = cmd(&mut s, &world, id, "account new builder pw");
        assert!(matches!(
            ops.as_slice(),
            [AccountOp::Create { username, password, .. }] if username == "builder" && password == "pw"
        ));
        let (_, ops) = cmd(&mut s, &world, id, "grant builder build");
        assert!(matches!(
            ops.as_slice(),
            [AccountOp::Grant { target, cap, add: true, .. }] if target == "builder" && cap == "build"
        ));
    }

    #[test]
    fn password_change_raises_a_set_password_op_for_the_bound_account() {
        let mut s = Sessions::default();
        let world = World::new();
        let id = ConnectionId(1);
        s.connect(id, None, &mut |_| {});

        // Unauthenticated: refused, no op.
        let (out, ops) = cmd(&mut s, &world, id, "password old new");
        assert!(ops.is_empty() && !out.is_empty());
        assert!(conn_texts(&out).iter().any(|t| t.contains("Log in")));

        // Bound to an account: the op carries that account and both passwords.
        let authz = su_authz();
        let account = authz.account;
        s.bind(id, authz);
        let (_, ops) = cmd(&mut s, &world, id, "password old new");
        assert!(matches!(
            ops.as_slice(),
            [AccountOp::SetPassword { account: a, old, new, .. }]
                if *a == account && old == "old" && new == "new"
        ));

        // `@pw` is the same handler.
        let (_, ops) = cmd(&mut s, &world, id, "pw old new");
        assert!(matches!(ops.as_slice(), [AccountOp::SetPassword { .. }]));
    }

    #[test]
    fn password_change_without_both_arguments_shows_usage() {
        let mut s = Sessions::default();
        let world = World::new();
        let id = ConnectionId(1);
        s.connect(id, None, &mut |_| {});
        s.bind(id, su_authz());

        let (out, ops) = cmd(&mut s, &world, id, "password only-old");
        assert!(ops.is_empty(), "a missing new password raises no op");
        assert!(
            conn_texts(&out)
                .iter()
                .any(|t| t.contains("@password <old> <new>"))
        );
    }

    #[test]
    fn bind_caches_authorization_and_clears_pending() {
        let mut s = Sessions::default();
        let id = ConnectionId(1);
        s.connect(id, loopback(), &mut |_| {});
        // Simulate an in-flight auth, then a completed one.
        s.map.get_mut(&id).unwrap().pending_auth = true;
        s.bind(id, su_authz());
        assert!(!s.is_pending(id));
        assert!(
            s.verdict_of(id).is_su(),
            "the cached authorization is in force"
        );
    }

    #[test]
    fn refresh_updates_every_session_on_that_account() {
        let mut s = Sessions::default();
        let a = ConnectionId(1);
        let b = ConnectionId(2);
        s.connect(a, loopback(), &mut |_| {});
        s.connect(b, loopback(), &mut |_| {});
        let authz = su_authz();
        let account = authz.account;
        s.bind(a, authz);
        // b binds to the same account.
        s.bind(
            b,
            Authorization {
                account,
                caps: CapSet::new(),
                su: false,
            },
        );

        // Refresh that account to su: both sessions on it update.
        s.refresh(Authorization {
            account,
            caps: CapSet::new(),
            su: true,
        });
        assert!(s.verdict_of(a).is_su());
        assert!(s.verdict_of(b).is_su());
    }

    #[test]
    fn quell_toggles_suppression() {
        // Observed through the verdict: an su session quelled drops to guest, and
        // un-quelling restores su.
        let mut s = Sessions::default();
        let world = World::new();
        let id = ConnectionId(1);
        s.connect(id, loopback(), &mut |_| {});
        s.bind(id, su_authz());
        assert!(s.verdict_of(id).is_su());
        cmd(&mut s, &world, id, "quell");
        assert!(!s.verdict_of(id).is_su(), "quelled su is not in force");
        cmd(&mut s, &world, id, "quell");
        assert!(s.verdict_of(id).is_su(), "un-quelled, su is back");
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
        assert!(!world.contains(ghost));
        assert_eq!(resolve_actor(&world, character), character);
    }

    fn conn_texts(out: &[Outgoing]) -> Vec<String> {
        out.iter()
            .filter_map(|o| match o {
                Outgoing::Event(Delivery { text, .. }) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }
}
