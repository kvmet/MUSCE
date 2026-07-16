//! The single command entry point the tick loop calls as it drains the inbox.
//! It owns input-stack routing: the `@`-namespace always goes to the account
//! floor; a bare command goes to the active in-game frame (the embodiment frame),
//! which this slice realizes as the connection's session attachment plus the
//! injected game's command table. Keeping this seam means the loop holds no
//! command knowledge: it drains the inbox and calls `handle`. See
//! `docs/architecture/actions.md` and `docs/architecture/engine-and-game.md`.

use musce_action::{Caller, ColdOp, CommandTable, dispatch_command, run_systems};
use musce_core::World;
use musce_proto::{Command, ConnectionId, Delivery, EventKind, Input, Outgoing};

use crate::accounts::{AccountOp, AccountOutcome};
use crate::session::{Sessions, resolve_actor};
use crate::{Game, TickCtx};

/// A host-level async op a command produced, routed by the sim loop to the task
/// that owns the resource it touches: a cold read/write to the cold task, or an
/// account op to the account task.
pub(crate) enum HostOp {
    Cold(ColdOp),
    Account(AccountOp),
}

pub struct Dispatch {
    /// The always-present account/session floor: `@`-commands, lifecycle, and the
    /// conn->actor attachments that back the embodiment frame. Also holds each
    /// connection's cached authorization (account, resolved caps, su), which the
    /// account task fills in on a successful login.
    floor: Sessions,
    /// The injected game: its command table drives bare commands and its
    /// `choose_actor` policy backs the floor's `@play`. The runtime holds no game
    /// content beyond this value.
    game: Game,
}

impl Dispatch {
    pub fn new(game: Game) -> Self {
        Self {
            floor: Sessions::default(),
            game,
        }
    }

    /// Route one inbound command, pushing output through `emit` and returning any
    /// host ops the handler queued: a cold-store request, or an account op the floor
    /// raised. Lifecycle and the `@`-namespace land on the floor; a bare command
    /// acts through the connection's attached actor, or reports having none.
    pub fn handle(
        &mut self,
        cmd: Command,
        world: &mut World,
        emit: &mut impl FnMut(Outgoing),
    ) -> Vec<HostOp> {
        let id = cmd.connection;
        match cmd.input {
            Input::Connected { peer, .. } => {
                self.floor.connect(id, peer, emit);
                Vec::new()
            }
            Input::Disconnected => {
                self.floor.disconnect(id);
                Vec::new()
            }
            Input::Line(line) => self.handle_line(id, line.trim(), world, emit),
        }
    }

    /// Apply an account-task outcome against session state: feed its line back, and
    /// bind the connection (a completed login) or refresh an account's cached caps
    /// wherever it is online (a completed grant/revoke), as the outcome directs.
    pub fn apply_account_outcome(
        &mut self,
        outcome: AccountOutcome,
        emit: &mut impl FnMut(Outgoing),
    ) {
        // Any authentication outcome, admit or refuse, ends the pending state; a
        // refused login must not wedge the connection. Safe to clear unconditionally:
        // only an authenticate op sets pending, and a pending connection can issue
        // nothing else, so no other op's outcome reaches a pending connection.
        self.floor.clear_pending(outcome.conn);
        emit(Outgoing::Event(Delivery::new(
            outcome.conn,
            EventKind::Feedback,
            outcome.line,
        )));
        if let Some(authz) = outcome.authenticated {
            self.floor.bind(outcome.conn, authz);
        }
        if let Some(authz) = outcome.refreshed {
            self.floor.refresh(authz);
        }
    }

    /// Run the game's systems for one tick, in registration order. Each system
    /// mutates the world and emits semantic output into its own buffer; that
    /// output is then audience-resolved to connections through `emit`, exactly as
    /// `dispatch_command` does for a verb. The audience index is built once
    /// (owned, so it does not borrow the world the systems mutate).
    pub fn run_systems(&self, world: &mut World, ctx: &TickCtx, emit: &mut impl FnMut(Outgoing)) {
        let actors = self.floor.audience_index(world);
        run_systems(world, &self.game.systems, &actors, ctx.tick, ctx.now, emit);
    }

    fn handle_line(
        &mut self,
        id: ConnectionId,
        line: &str,
        world: &mut World,
        emit: &mut impl FnMut(Outgoing),
    ) -> Vec<HostOp> {
        if !self.floor.is_live(id) || line.is_empty() {
            return Vec::new();
        }

        // A connection waiting on an authentication result runs nothing else: one
        // in-flight auth per connection, so a line arriving mid-auth is rejected
        // rather than queued (it caps the argon2 work a connection can provoke).
        if self.floor.is_pending(id) {
            emit(Outgoing::Event(Delivery::new(
                id,
                EventKind::Feedback,
                "Still authenticating; one moment.",
            )));
            return Vec::new();
        }

        if let Some(rest) = line.strip_prefix('@') {
            // The floor owns the lifecycle and account verbs; any other @-verb is an
            // admin/builder command, dispatched against the game's admin table with
            // the same actor resolution the bare frame uses. Recognized account verbs
            // may raise account ops (an authenticate, a grant); those flow out here.
            let mut ops = Vec::new();
            if self
                .floor
                .account_command(id, rest, world, self.game.choose_actor, &mut ops, emit)
            {
                ops.into_iter().map(HostOp::Account).collect()
            } else {
                dispatch_through_actor(&self.floor, &self.game.admin, id, rest, world, emit)
                    .into_iter()
                    .map(HostOp::Cold)
                    .collect()
            }
        } else {
            dispatch_through_actor(&self.floor, &self.game.commands, id, line, world, emit)
                .into_iter()
                .map(HostOp::Cold)
                .collect()
        }
    }
}

/// Resolve a connection's character to its live actor and run `line` against
/// `table`. The character is session state; the driven actor is derived live from
/// its `Focus` (so piloting redirects bare commands and admin verbs alike). A
/// connection with no character has nothing to act through and is told to `@play`.
fn dispatch_through_actor(
    floor: &Sessions,
    table: &CommandTable,
    id: ConnectionId,
    line: &str,
    world: &mut World,
    emit: &mut impl FnMut(Outgoing),
) -> Vec<ColdOp> {
    let Some(character) = floor.character_of(id) else {
        emit(Outgoing::Event(Delivery::new(
            id,
            EventKind::Feedback,
            "You have no character. Use @play to enter the world.",
        )));
        return Vec::new();
    };
    let actor = resolve_actor(world, character);
    // The verdict is the connection's cached authorization (account caps + su, quell
    // applied), never the resolved actor, so a possessed or `@play`-selected body
    // cannot borrow authority. Gates the game table and the admin table alike.
    let verdict = floor.verdict_of(id);
    let actors = floor.audience_index(world);
    dispatch_command(
        table,
        world,
        &actors,
        Caller {
            actor,
            conn: id,
            verdict: &verdict,
        },
        line,
        emit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::Authorization;
    use musce_action::{CapId, CapRegistry, CapSet, Ctx, Gate};
    use musce_auth::AccountId;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Description, EntityId, Id, Locus};
    use musce_proto::Capabilities;
    use std::net::SocketAddr;
    use std::sync::Arc;

    /// A local stand-in for a game's player kind: the engine has no `Player`
    /// concept, so the router test defines its own marker to pick an actor by.
    struct Avatar;

    /// An engine-only `Game` so the router can be exercised without a real game. Its
    /// seed makes one described room with a player avatar in it, `choose_actor`
    /// selects that avatar, `look` echoes the room description, and the admin table
    /// holds a capability-gated `poke`.
    fn test_game() -> Game {
        fn seed(world: &mut World) {
            let room = {
                let mut b = EntityBuilder::new();
                b.add(Locus);
                b.add(Description("a test chamber".into()));
                world.spawn(b)
            };
            let avatar = {
                let mut b = EntityBuilder::new();
                b.add(Avatar);
                b.add(Description("a tester".into()));
                world.spawn(b)
            };
            world.move_entity(avatar, room).unwrap();
        }

        fn choose_actor(world: &World) -> Option<EntityId> {
            world
                .ecs()
                .query::<(&Id, &Avatar)>()
                .iter()
                .next()
                .map(|(id, _)| id.0)
        }

        fn look(ctx: &mut Ctx, _args: &str) {
            let text = ctx
                .world
                .enclosing_locus(ctx.actor)
                .and_then(|r| {
                    ctx.world
                        .entity(r)
                        .and_then(|er| er.get::<&Description>().map(|d| d.0.clone()))
                })
                .unwrap_or_else(|| "nowhere".into());
            ctx.emit_self(EventKind::Narration, text);
        }

        fn poke(ctx: &mut Ctx, _args: &str) {
            ctx.emit_self(EventKind::Feedback, "poked");
        }

        let mut commands = CommandTable::new();
        commands.register("look", Gate::Open, look);
        let mut caps = CapRegistry::new();
        let poke_cap = caps.register("poke");
        let mut admin = CommandTable::new();
        admin.register("poke", Gate::Cap(poke_cap), poke);
        Game {
            commands,
            admin,
            seed,
            choose_actor,
            systems: vec![],
            register: |_| {},
            caps: Arc::new(caps),
            login_veto: |_| Ok(()),
            decode_cold: |_| Ok(String::new()),
        }
    }

    fn client_caps() -> Capabilities {
        Capabilities {
            color: false,
            line_mode_only: true,
            size: None,
        }
    }

    fn loopback() -> Option<SocketAddr> {
        Some("127.0.0.1:9000".parse().unwrap())
    }

    fn connect_from(
        d: &mut Dispatch,
        world: &mut World,
        id: ConnectionId,
        peer: Option<SocketAddr>,
    ) {
        d.handle(
            Command {
                connection: id,
                input: Input::Connected {
                    caps: client_caps(),
                    peer,
                },
            },
            world,
            &mut |_| {},
        );
    }

    fn connect(d: &mut Dispatch, world: &mut World, id: ConnectionId) {
        connect_from(d, world, id, loopback());
    }

    /// Drive one line and collect the connection-facing output.
    fn line(d: &mut Dispatch, world: &mut World, id: ConnectionId, s: &str) -> Vec<Outgoing> {
        let mut out = Vec::new();
        d.handle(
            Command {
                connection: id,
                input: Input::Line(s.into()),
            },
            world,
            &mut |o| out.push(o),
        );
        out
    }

    /// Drive one line and return the host ops it raised (dropping the output).
    fn ops(d: &mut Dispatch, world: &mut World, id: ConnectionId, s: &str) -> Vec<HostOp> {
        d.handle(
            Command {
                connection: id,
                input: Input::Line(s.into()),
            },
            world,
            &mut |_| {},
        )
    }

    /// Bind a connection to a fresh superuser account, simulating a completed login
    /// (the account task normally hands this back as an `Authenticated` outcome).
    fn bind_su(d: &mut Dispatch, id: ConnectionId) {
        d.floor.bind(
            id,
            Authorization {
                account: AccountId::generate(),
                caps: CapSet::new(),
                su: true,
            },
        );
    }

    /// Bind a connection to a fresh plain account holding exactly `cap`.
    fn bind_capped(d: &mut Dispatch, id: ConnectionId, cap: CapId) {
        d.floor.bind(
            id,
            Authorization {
                account: AccountId::generate(),
                caps: [cap].into_iter().collect(),
                su: false,
            },
        );
    }

    fn conn_texts(out: &[Outgoing]) -> Vec<String> {
        out.iter()
            .filter_map(|o| match o {
                Outgoing::Event(Delivery { text, .. }) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// The `@`-namespace still reaches the floor: connect then `@quit` closes.
    #[test]
    fn at_command_routes_to_floor() {
        let mut d = Dispatch::new(test_game());
        let mut world = World::new();
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        let out = line(&mut d, &mut world, id, "@quit");
        assert!(
            out.iter()
                .any(|o| matches!(o, Outgoing::Close(c) if *c == id))
        );
    }

    /// A bare command before `@play` reports having no character.
    #[test]
    fn bare_without_actor_reports_no_character() {
        let mut d = Dispatch::new(test_game());
        let mut world = World::new();
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        let out = line(&mut d, &mut world, id, "look");
        assert!(matches!(
            out.as_slice(),
            [Outgoing::Event(Delivery {
                kind: EventKind::Feedback,
                ..
            })]
        ));
    }

    /// End to end through the router: `@play` then a bare command acts through the
    /// injected game's table against its seeded world.
    #[test]
    fn play_then_bare_command_acts_through_the_game() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = Dispatch::new(game);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        line(&mut d, &mut world, id, "@play");
        let out = line(&mut d, &mut world, id, "look");
        assert!(
            conn_texts(&out)
                .iter()
                .any(|t| t.contains("a test chamber"))
        );
    }

    /// A superuser session passes a capability-gated admin verb by su bypass.
    #[test]
    fn su_session_passes_a_gated_verb() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = Dispatch::new(game);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);
        bind_su(&mut d, id);
        line(&mut d, &mut world, id, "@play");

        let out = line(&mut d, &mut world, id, "@poke");
        assert!(
            conn_texts(&out).iter().any(|t| t.contains("poked")),
            "got: {out:?}"
        );
    }

    /// A plain account holding the cap passes it, without su.
    #[test]
    fn capped_session_passes_its_granted_verb() {
        let game = test_game();
        let poke = game.caps.resolve("poke").unwrap();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = Dispatch::new(game);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);
        bind_capped(&mut d, id, poke);
        line(&mut d, &mut world, id, "@play");

        let out = line(&mut d, &mut world, id, "@poke");
        assert!(
            conn_texts(&out).iter().any(|t| t.contains("poked")),
            "got: {out:?}"
        );
    }

    /// A guest (a connection bound to no account) is refused a gated verb.
    #[test]
    fn guest_is_refused_a_gated_verb() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = Dispatch::new(game);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);
        line(&mut d, &mut world, id, "@play");

        let out = line(&mut d, &mut world, id, "@poke");
        assert!(
            conn_texts(&out)
                .iter()
                .any(|t| t.contains("aren't allowed")),
            "a guest should be refused, got: {out:?}"
        );
    }

    /// `@quell` drops a superuser session to its character: the gated verb is then
    /// refused, and un-quelling restores it.
    #[test]
    fn quell_drops_su_to_the_character() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = Dispatch::new(game);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);
        bind_su(&mut d, id);
        line(&mut d, &mut world, id, "@play");

        line(&mut d, &mut world, id, "@quell");
        let refused = line(&mut d, &mut world, id, "@poke");
        assert!(
            conn_texts(&refused)
                .iter()
                .any(|t| t.contains("aren't allowed")),
            "quelled su holds nothing, got: {refused:?}"
        );

        line(&mut d, &mut world, id, "@quell");
        let allowed = line(&mut d, &mut world, id, "@poke");
        assert!(
            conn_texts(&allowed).iter().any(|t| t.contains("poked")),
            "un-quelled, su is back, got: {allowed:?}"
        );
    }

    /// `@login` (from loopback) raises an authenticate op and marks the connection
    /// pending; a line arriving before the result lands is rejected, not run.
    #[test]
    fn login_marks_pending_and_rejects_in_flight_lines() {
        let mut d = Dispatch::new(test_game());
        let mut world = World::new();
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        let raised = ops(&mut d, &mut world, id, "@login builder secret");
        assert!(
            matches!(
                raised.as_slice(),
                [HostOp::Account(AccountOp::Authenticate { .. })]
            ),
            "@login raises exactly one authenticate op"
        );

        let out = line(&mut d, &mut world, id, "look");
        assert!(
            conn_texts(&out)
                .iter()
                .any(|t| t.contains("Still authenticating")),
            "a line mid-auth is rejected, got: {out:?}"
        );
    }

    /// Applying an `Authenticated` outcome binds the connection and feeds its line
    /// back; the bound authority then passes a gated verb.
    #[test]
    fn apply_outcome_binds_the_connection() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = Dispatch::new(game);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        let mut out = Vec::new();
        d.apply_account_outcome(
            AccountOutcome {
                conn: id,
                line: "You are now logged in as operator.".into(),
                authenticated: Some(Authorization {
                    account: AccountId::generate(),
                    caps: CapSet::new(),
                    su: true,
                }),
                refreshed: None,
            },
            &mut |o| out.push(o),
        );
        assert!(conn_texts(&out).iter().any(|t| t.contains("logged in")));

        line(&mut d, &mut world, id, "@play");
        let poke = line(&mut d, &mut world, id, "@poke");
        assert!(
            conn_texts(&poke).iter().any(|t| t.contains("poked")),
            "bound su passes"
        );
    }

    /// A refused authentication (no binding) still clears the pending flag, or the
    /// connection would be wedged, rejecting every retry.
    #[test]
    fn refused_auth_outcome_clears_pending() {
        let mut d = Dispatch::new(test_game());
        let mut world = World::new();
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        // Enter pending via a login, then apply a refusal (authenticated: None).
        ops(&mut d, &mut world, id, "@login builder secret");
        d.apply_account_outcome(
            AccountOutcome {
                conn: id,
                line: "Incorrect password.".into(),
                authenticated: None,
                refreshed: None,
            },
            &mut |_| {},
        );

        // The next line runs (reporting no character) rather than being rejected as
        // still-authenticating.
        let out = line(&mut d, &mut world, id, "look");
        assert!(
            !conn_texts(&out)
                .iter()
                .any(|t| t.contains("Still authenticating")),
            "pending cleared after a refusal, got: {out:?}"
        );
    }

    /// `Game.systems` is a `Vec`, so the pipeline runs every registered system in
    /// one tick. Two distinct systems each leave a mark; both present proves both ran.
    #[test]
    fn run_systems_runs_every_registered_system() {
        use musce_action::SystemCtx;
        use std::time::SystemTime;

        struct MarkA;
        struct MarkB;

        fn add_a(ctx: &mut SystemCtx) {
            let mut b = EntityBuilder::new();
            b.add(MarkA);
            ctx.world.spawn(b);
        }
        fn add_b(ctx: &mut SystemCtx) {
            let mut b = EntityBuilder::new();
            b.add(MarkB);
            ctx.world.spawn(b);
        }

        let game = Game {
            commands: CommandTable::new(),
            admin: CommandTable::new(),
            seed: |_| {},
            choose_actor: |_| None,
            systems: vec![add_a, add_b],
            register: |_| {},
            caps: Arc::new(CapRegistry::new()),
            login_veto: |_| Ok(()),
            decode_cold: |_| Ok(String::new()),
        };
        let dispatch = Dispatch::new(game);
        let mut world = World::new();
        let ctx = TickCtx {
            tick: 1,
            now: SystemTime::UNIX_EPOCH,
        };

        dispatch.run_systems(&mut world, &ctx, &mut |_| {});

        assert_eq!(world.ecs().query::<&MarkA>().iter().count(), 1);
        assert_eq!(world.ecs().query::<&MarkB>().iter().count(), 1);
    }
}
