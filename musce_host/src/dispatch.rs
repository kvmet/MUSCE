//! The single command entry point the tick loop calls as it drains the inbox.
//! It owns input-stack routing: the `@`-namespace always goes to the account
//! floor; a bare command goes to the active in-game frame (the embodiment frame),
//! which this slice realizes as the connection's session attachment plus the
//! injected game's command table. Keeping this seam means the loop holds no
//! command knowledge: it drains the inbox and calls `handle`. See
//! `docs/architecture/actions.md` and `docs/architecture/engine-and-game.md`.

use musce_action::dispatch_bare;
use musce_core::World;
use musce_proto::{Command, ConnectionId, Event, EventKind, Input, Outgoing};

use crate::Game;
use crate::session::Sessions;

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
            self.floor
                .account_command(id, rest, world, self.game.choose_actor, emit);
        } else if let Some(actor) = self.floor.actor_of(id) {
            // The resolver needs the conn<->actor view; the attachments on the
            // floor are the source of truth, so derive a fresh index from them.
            let actors = self.floor.audience_index();
            dispatch_bare(&self.game.commands, world, &actors, actor, id, line, emit);
        } else {
            emit(Outgoing::Event(Event::to_connection(
                id,
                EventKind::Feedback,
                "You have no character. Use @play to enter the world.",
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_action::{CommandTable, Ctx, Gate};
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Description, EntityId, Id, Player, Room};
    use musce_proto::{Audience, Capabilities};

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
                b.add(Player);
                b.add(Description("a tester".into()));
                world.spawn(b)
            };
            world.move_entity(avatar, room).unwrap();
        }

        fn choose_actor(world: &World) -> Option<EntityId> {
            world
                .ecs
                .query::<(&Id, &Player)>()
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

        let mut commands = CommandTable::new();
        commands.register("look", Gate::Open, look);
        Game {
            commands,
            seed,
            choose_actor,
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
}
