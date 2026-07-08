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
}

impl Dispatch {
    pub fn new(game: Game) -> Self {
        Self {
            floor: Sessions::default(),
            game,
        }
    }

    /// Route one inbound command, pushing output through `emit`. Lifecycle and
    /// the `@`-namespace land on the floor; a bare command acts through the
    /// connection's attached actor, or reports having none.
    pub fn handle(&mut self, cmd: Command, world: &mut World, emit: &mut impl FnMut(Outgoing)) {
        let id = cmd.connection;
        match cmd.input {
            Input::Connected { .. } => self.floor.connect(id, emit),
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
            // The floor owns the lifecycle verbs (@quit/@who/@help/@play); any
            // other @-verb is an admin/builder command, dispatched against the
            // game's admin table with the same actor resolution the bare frame
            // uses. The floor reports whether it recognized the verb.
            if !self
                .floor
                .account_command(id, rest, world, self.game.choose_actor, emit)
            {
                dispatch_through_actor(&self.floor, &self.game.admin, id, rest, world, emit);
            }
        } else {
            dispatch_through_actor(&self.floor, &self.game.commands, id, line, world, emit);
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
    let actors = floor.audience_index(world);
    dispatch_command(table, world, &actors, actor, id, line, emit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_action::{Ctx, Gate};
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Description, EntityId, Id, Room, Staff};
    use musce_proto::{Audience, Capabilities};

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
                b.add(Room);
                b.add(Description("a test chamber".into()));
                world.spawn(b)
            };
            let avatar = {
                let mut b = EntityBuilder::new();
                b.add(Avatar);
                b.add(Staff); // staff, so the admin frame is exercisable
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
                .enclosing_room(ctx.actor)
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
        let mut admin = CommandTable::new();
        admin.register("poke", Gate::Staff, poke);
        Game {
            commands,
            admin,
            seed,
            choose_actor,
            systems: vec![],
            register: |_| {},
        }
    }

    fn caps() -> Capabilities {
        Capabilities {
            color: false,
            line_mode_only: true,
            size: None,
        }
    }

    fn connect(d: &mut Dispatch, world: &mut World, id: ConnectionId) {
        d.handle(
            Command {
                connection: id,
                input: Input::Connected {
                    caps: caps(),
                    peer: None,
                },
            },
            world,
            &mut |_| {},
        );
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

    /// A non-lifecycle `@`-verb routes to the game's admin table (and runs, since
    /// the seeded avatar is staff), rather than being swallowed by the floor.
    #[test]
    fn at_admin_verb_routes_to_admin_table() {
        let game = test_game();
        let mut world = World::new();
        (game.seed)(&mut world);
        let mut d = Dispatch::new(game);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

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
        let mut d = Dispatch::new(test_game());
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
        let mut d = Dispatch::new(game);
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
        };
        let dispatch = Dispatch::new(game);
        let mut world = World::new();
        let ctx = TickCtx {
            tick: 1,
            now: SystemTime::UNIX_EPOCH,
        };

        dispatch.run_systems(&mut world, &ctx, &mut |_| {});

        assert_eq!(world.ecs.query::<&MarkA>().iter().count(), 1);
        assert_eq!(world.ecs.query::<&MarkB>().iter().count(), 1);
    }
}
