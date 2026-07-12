//! The single command entry point the tick loop calls as it drains the inbox.
//! It owns input-stack routing: the `@`-namespace always goes to the account
//! floor; a bare command goes to the active in-game frame (the embodiment frame),
//! which this slice realizes as the connection's session attachment plus the
//! injected game's command table. Keeping this seam means the loop holds no
//! command knowledge: it drains the inbox and calls `handle`. See
//! `docs/architecture/actions.md` and `docs/architecture/engine-and-game.md`.

use musce_action::{CommandTable, Outbound, SystemCtx, dispatch_command, resolve};
use musce_core::World;
use musce_proto::{Command, ConnectionId, Event, EventKind, Input, Outgoing};

use crate::auth::Accounts;
use crate::session::{Sessions, resolve_actor};
use crate::{Game, TickCtx};

pub struct Dispatch {
    /// The always-present account/session floor: `@`-commands, lifecycle, and the
    /// conn->actor attachments that back the embodiment frame.
    floor: Sessions,
    /// The injected game: its command table drives bare commands and its
    /// `choose_actor` policy backs the floor's `@play`. The runtime holds no game
    /// content beyond this value.
    game: Game,
    /// The account authority: resolves a connection's account to the authorization
    /// verdict a gate (and a game's inline rules) run under. Sim-thread-owned.
    accounts: Accounts,
}

impl Dispatch {
    pub fn new(game: Game, accounts: Accounts) -> Self {
        Self {
            floor: Sessions::default(),
            game,
            accounts,
        }
    }

    /// Route one inbound command, pushing output through `emit`. Lifecycle and
    /// the `@`-namespace land on the floor; a bare command acts through the
    /// connection's attached actor, or reports having none.
    pub fn handle(&mut self, cmd: Command, world: &mut World, emit: &mut impl FnMut(Outgoing)) {
        let id = cmd.connection;
        match cmd.input {
            Input::Connected { peer, .. } => self.floor.connect(id, peer, emit),
            Input::Disconnected => self.floor.disconnect(id),
            Input::Line(line) => self.handle_line(id, line.trim(), world, emit),
        }
    }

    /// Run the game's systems for one tick, in registration order. Each system
    /// mutates the world and emits semantic output into its own buffer; that
    /// output is then audience-resolved to connections through `emit`, exactly as
    /// `dispatch_command` does for a verb. The audience index is built once
    /// (owned, so it does not borrow the world the systems mutate).
    pub fn run_systems(&self, world: &mut World, ctx: &TickCtx, emit: &mut impl FnMut(Outgoing)) {
        let actors = self.floor.audience_index(world);
        // Drain the tick's structural facts once, before the loop: every system
        // sees the same batch, and a fact a system emits buffers for the next tick
        // rather than being seen within this pass (so system order is cosmetic).
        // Unconditional even with no reactions registered, or facts would leak.
        let facts = world.take_facts();
        for system in &self.game.systems {
            let mut out: Vec<Outbound> = Vec::new();
            {
                let mut sctx = SystemCtx::new(world, ctx.tick, ctx.now, &facts, &mut out);
                system(&mut sctx);
            }
            for ob in out {
                resolve(world, &actors, ob, emit);
            }
        }
    }

    fn handle_line(
        &mut self,
        id: ConnectionId,
        line: &str,
        world: &mut World,
        emit: &mut impl FnMut(Outgoing),
    ) {
        if !self.floor.is_live(id) || line.is_empty() {
            return;
        }

        if let Some(rest) = line.strip_prefix('@') {
            // The floor owns the lifecycle and account verbs (@quit/@who/@help/@play,
            // @operator/@quell); any other @-verb is an admin/builder command,
            // dispatched against the game's admin table with the same actor resolution
            // the bare frame uses. The floor reports whether it recognized the verb.
            if !self.floor.account_command(
                id,
                rest,
                world,
                &mut self.accounts,
                self.game.choose_actor,
                emit,
            ) {
                dispatch_through_actor(
                    &self.floor,
                    &self.accounts,
                    &self.game.admin,
                    id,
                    rest,
                    world,
                    emit,
                );
            }
        } else {
            dispatch_through_actor(
                &self.floor,
                &self.accounts,
                &self.game.commands,
                id,
                line,
                world,
                emit,
            );
        }
    }
}

/// Resolve a connection's character to its live actor and run `line` against
/// `table`. The character is session state; the driven actor is derived live from
/// its `Focus` (so piloting redirects bare commands and admin verbs alike). A
/// connection with no character has nothing to act through and is told to `@play`.
fn dispatch_through_actor(
    floor: &Sessions,
    accounts: &Accounts,
    table: &CommandTable,
    id: ConnectionId,
    line: &str,
    world: &mut World,
    emit: &mut impl FnMut(Outgoing),
) {
    let Some(character) = floor.character_of(id) else {
        emit(Outgoing::Event(Event::to_connection(
            id,
            EventKind::Feedback,
            "You have no character. Use @play to enter the world.",
        )));
        return;
    };
    let actor = resolve_actor(world, character);
    // The verdict keys off the connection's account and quell state, never the
    // resolved actor, so a possessed or `@play`-selected body cannot borrow
    // authority. Resolved once here, gating the game table and the admin table alike.
    let verdict = accounts.verdict_for(floor.account_of(id), floor.is_quelled(id));
    let actors = floor.audience_index(world);
    dispatch_command(table, world, &actors, actor, id, &verdict, line, emit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{CapRegistry, MemoryAccountStore};
    use musce_action::{Ctx, Gate};
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Description, EntityId, Id, Locus};
    use musce_proto::{Audience, Capabilities};
    use std::net::SocketAddr;
    use std::sync::Arc;

    /// A local stand-in for a game's player kind: the engine has no `Player`
    /// concept, so the router test defines its own marker to pick an actor by.
    struct Avatar;

    /// An engine-only `Game` so the router can be exercised without depending on
    /// a real game. Its seed makes one described room with a player avatar in it,
    /// `choose_actor` selects that avatar, and `look` echoes the avatar's room
    /// description, so the routing is observable end to end.
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
                .ecs
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
        // `poke` is capability-gated, so only su (the operator) or an account holding
        // the cap runs it. No account is granted the cap in these tests; the operator
        // passes by su bypass.
        let mut caps = CapRegistry::new();
        let poke_cap = caps.register_cap("poke");
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
        }
    }

    /// Build a dispatcher for `game`, booting its account authority (an empty store,
    /// so one su operator is bootstrapped).
    fn dispatcher(game: Game) -> Dispatch {
        let accounts = Accounts::boot(&MemoryAccountStore::new(), game.caps.clone()).unwrap();
        Dispatch::new(game, accounts)
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

    /// Connect from a loopback peer, so `@operator` elevation is available.
    fn connect(d: &mut Dispatch, world: &mut World, id: ConnectionId) {
        connect_from(d, world, id, loopback());
    }

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

    /// The `@`-namespace still reaches the floor: connect then `@quit` closes.
    #[test]
    fn at_command_routes_to_floor() {
        let mut d = dispatcher(test_game());
        let mut world = World::new();
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        let out = line(&mut d, &mut world, id, "@quit");
        assert!(
            out.iter()
                .any(|o| matches!(o, Outgoing::Close(c) if *c == id))
        );
    }

    /// A non-lifecycle `@`-verb routes to the game's admin table and runs, once the
    /// connection has elevated to the su operator (via the loopback `@operator` stub),
    /// rather than being swallowed by the floor.
    #[test]
    fn at_admin_verb_routes_to_admin_table() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = dispatcher(game);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        line(&mut d, &mut world, id, "@operator");
        line(&mut d, &mut world, id, "@play");
        let out = line(&mut d, &mut world, id, "@poke");

        let texts: Vec<String> = out
            .iter()
            .filter_map(|o| match o {
                Outgoing::Event(Event {
                    text,
                    to: Audience::Connection(_),
                    ..
                }) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.contains("poked")), "got: {texts:?}");
    }

    /// A bare command before `@play` reports having no character.
    #[test]
    fn bare_without_actor_reports_no_character() {
        let mut d = dispatcher(test_game());
        let mut world = World::new();
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        let out = line(&mut d, &mut world, id, "look");
        assert!(matches!(
            out.as_slice(),
            [Outgoing::Event(Event {
                kind: EventKind::Feedback,
                ..
            })]
        ));
    }

    /// End to end through the router: @play then a bare command acts through the
    /// injected game's table against the injected game's seeded world.
    #[test]
    fn play_then_bare_command_acts_through_the_game() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = dispatcher(game);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        line(&mut d, &mut world, id, "@play");
        let out = line(&mut d, &mut world, id, "look");

        let rendered: Vec<String> = out
            .iter()
            .filter_map(|o| match o {
                Outgoing::Event(Event {
                    text,
                    to: Audience::Connection(_),
                    ..
                }) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(rendered.iter().any(|t| t.contains("a test chamber")));
    }

    /// `Game.systems` is a `Vec`, so the pipeline runs every registered system in
    /// one tick. Two distinct systems each leave a different mark on the world;
    /// both marks present proves both ran (not one twice).
    #[test]
    fn run_systems_runs_every_registered_system() {
        use musce_action::SystemCtx;
        use std::time::SystemTime;

        // Two distinct local marks: each system leaves its own, so both present
        // proves both systems ran (not one twice). Plain markers, since the engine
        // has no kinds of its own to borrow.
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
        };
        let dispatch = dispatcher(game);
        let mut world = World::new();
        let ctx = TickCtx {
            tick: 1,
            now: SystemTime::UNIX_EPOCH,
        };

        dispatch.run_systems(&mut world, &ctx, &mut |_| {});

        assert_eq!(world.ecs.query::<&MarkA>().iter().count(), 1);
        assert_eq!(world.ecs.query::<&MarkB>().iter().count(), 1);
    }

    /// The connection-addressed feedback texts in an output batch.
    fn conn_texts(out: &[Outgoing]) -> Vec<String> {
        out.iter()
            .filter_map(|o| match o {
                Outgoing::Event(Event {
                    text,
                    to: Audience::Connection(_),
                    ..
                }) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// A guest (a connection that never elevated) is refused a capability-gated verb:
    /// no account means no caps and no su.
    #[test]
    fn guest_is_refused_a_capability_gated_verb() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = dispatcher(game);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        line(&mut d, &mut world, id, "@play");
        let out = line(&mut d, &mut world, id, "@poke");
        assert!(
            conn_texts(&out)
                .iter()
                .any(|t| t.contains("aren't allowed")),
            "a guest should be refused the gated verb, got: {out:?}"
        );
    }

    /// `@quell` drops the operator to its own (empty) caps: su suppressed, the gated
    /// verb is refused; un-quelling restores su and it passes again.
    #[test]
    fn quell_drops_the_operator_to_guest_authority() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = dispatcher(game);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);
        line(&mut d, &mut world, id, "@operator");
        line(&mut d, &mut world, id, "@play");

        line(&mut d, &mut world, id, "@quell");
        let refused = line(&mut d, &mut world, id, "@poke");
        assert!(
            conn_texts(&refused)
                .iter()
                .any(|t| t.contains("aren't allowed")),
            "a quelled operator holds no poke cap, got: {refused:?}"
        );

        line(&mut d, &mut world, id, "@quell");
        let allowed = line(&mut d, &mut world, id, "@poke");
        assert!(
            conn_texts(&allowed).iter().any(|t| t.contains("poked")),
            "un-quelled, su is restored, got: {allowed:?}"
        );
    }

    /// The loopback-only stub refuses a peerless (in-process) connection: a `None`
    /// peer must read as not-loopback, never default-permit, and the connection stays
    /// unable to run the gated verb.
    #[test]
    fn operator_refused_from_a_non_loopback_peer() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = dispatcher(game);
        let id = ConnectionId(1);
        connect_from(&mut d, &mut world, id, None);

        let out = line(&mut d, &mut world, id, "@operator");
        assert!(
            conn_texts(&out)
                .iter()
                .any(|t| t.contains("only available locally")),
            "a peerless connection must be refused elevation, got: {out:?}"
        );

        line(&mut d, &mut world, id, "@play");
        let poke = line(&mut d, &mut world, id, "@poke");
        assert!(
            conn_texts(&poke)
                .iter()
                .any(|t| t.contains("aren't allowed")),
            "still a guest, so still refused, got: {poke:?}"
        );
    }

    /// Authority is per-account, not per-body. Two connections drive the *same*
    /// seeded avatar; only the one elevated to the operator account passes the gated
    /// verb, so no shared body can lend its authority to the other.
    #[test]
    fn authority_follows_the_account_not_the_body() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = dispatcher(game);
        let op = ConnectionId(1);
        let guest = ConnectionId(2);
        connect(&mut d, &mut world, op);
        connect(&mut d, &mut world, guest);

        line(&mut d, &mut world, op, "@operator");
        line(&mut d, &mut world, op, "@play");
        line(&mut d, &mut world, guest, "@play");

        let op_out = line(&mut d, &mut world, op, "@poke");
        let guest_out = line(&mut d, &mut world, guest, "@poke");
        assert!(
            conn_texts(&op_out).iter().any(|t| t.contains("poked")),
            "the operator passes, got: {op_out:?}"
        );
        assert!(
            conn_texts(&guest_out)
                .iter()
                .any(|t| t.contains("aren't allowed")),
            "the guest is refused driving the same body, got: {guest_out:?}"
        );
    }

    /// The composable model end to end through the router: the operator creates a
    /// builder account and grants it the (quellable) poke cap; a second connection
    /// logs in as the builder, passes the gated verb, and loses it under `@quell`.
    /// The whole point of the account surface: a non-su account holding a real cap,
    /// reachable by login, with quell dropping the elevated grant.
    #[test]
    fn a_granted_builder_runs_a_gated_verb_and_quell_drops_it() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = dispatcher(game);

        // The operator mints a builder account and grants it poke.
        let op = ConnectionId(1);
        connect(&mut d, &mut world, op);
        line(&mut d, &mut world, op, "@operator");
        line(&mut d, &mut world, op, "@account new builder");
        line(&mut d, &mut world, op, "@grant builder poke");

        // A second connection logs in as the builder and takes the seeded body.
        let builder = ConnectionId(2);
        connect(&mut d, &mut world, builder);
        line(&mut d, &mut world, builder, "@login builder");
        line(&mut d, &mut world, builder, "@play");

        let ok = line(&mut d, &mut world, builder, "@poke");
        assert!(
            conn_texts(&ok).iter().any(|t| t.contains("poked")),
            "the granted builder passes the gated verb, got: {ok:?}"
        );

        // Quell sets aside the quellable poke cap: refused.
        line(&mut d, &mut world, builder, "@quell");
        let refused = line(&mut d, &mut world, builder, "@poke");
        assert!(
            conn_texts(&refused)
                .iter()
                .any(|t| t.contains("aren't allowed")),
            "a quelled builder loses its elevated cap, got: {refused:?}"
        );

        // Un-quell restores it.
        line(&mut d, &mut world, builder, "@quell");
        let restored = line(&mut d, &mut world, builder, "@poke");
        assert!(
            conn_texts(&restored).iter().any(|t| t.contains("poked")),
            "un-quelled, the cap is back, got: {restored:?}"
        );
    }
}
