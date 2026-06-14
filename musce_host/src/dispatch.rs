//! The single command entry point the tick loop calls as it drains the inbox.
//! It owns input-stack routing: the `@`-namespace always goes to the account
//! floor; a bare command goes to the active in-game frame (the embodiment frame),
//! which this slice realizes as the stub actor binding plus the action layer's
//! command table. Keeping this seam means the loop holds no command knowledge: it
//! drains the inbox and calls `handle`. See `docs/architecture/actions.md`.

use musce_action::{Actors, CommandTable, dispatch_bare};
use musce_core::World;
use musce_proto::{Command, ConnectionId, Event, EventKind, Input, Outgoing};

use crate::session::Sessions;

pub struct Dispatch {
    /// The always-present account/session floor (`@`-commands, lifecycle).
    floor: Sessions,
    /// Stub connection<->actor bindings; the embodiment frame and the audience
    /// resolver both read these.
    actors: Actors,
    /// The in-game verb registry, built once and shared read-only.
    table: CommandTable,
}

impl Dispatch {
    pub fn new() -> Self {
        Self {
            floor: Sessions::default(),
            actors: Actors::default(),
            table: CommandTable::default(),
        }
    }

    /// Route one inbound command, pushing output through `emit`. Lifecycle and
    /// the `@`-namespace land on the floor; a bare command acts through the
    /// connection's bound actor, or reports having none.
    pub fn handle(&mut self, cmd: Command, world: &mut World, emit: &mut impl FnMut(Outgoing)) {
        let id = cmd.connection;
        match cmd.input {
            Input::Connected { .. } => self.floor.connect(id, emit),
            Input::Disconnected => {
                self.actors.unbind(id);
                self.floor.disconnect(id);
            }
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
            self.floor.account_command(id, rest, world, &mut self.actors, emit);
        } else if let Some(actor) = self.actors.actor_of(id) {
            dispatch_bare(&self.table, world, &self.actors, actor, id, line, emit);
        } else {
            emit(Outgoing::Event(Event::to_connection(
                id,
                EventKind::Feedback,
                "You have no character. Use @play to enter the world.",
            )));
        }
    }
}

impl Default for Dispatch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_proto::{Audience, Capabilities};

    fn caps() -> Capabilities {
        Capabilities { color: false, line_mode_only: true, size: None }
    }

    fn connect(d: &mut Dispatch, world: &mut World, id: ConnectionId) {
        d.handle(
            Command { connection: id, input: Input::Connected { caps: caps(), peer: None } },
            world,
            &mut |_| {},
        );
    }

    fn line(d: &mut Dispatch, world: &mut World, id: ConnectionId, s: &str) -> Vec<Outgoing> {
        let mut out = Vec::new();
        d.handle(
            Command { connection: id, input: Input::Line(s.into()) },
            world,
            &mut |o| out.push(o),
        );
        out
    }

    /// The `@`-namespace still reaches the floor: connect then `@quit` closes.
    #[test]
    fn at_command_routes_to_floor() {
        let mut d = Dispatch::new();
        let mut world = World::new();
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        let out = line(&mut d, &mut world, id, "@quit");
        assert!(out.iter().any(|o| matches!(o, Outgoing::Close(c) if *c == id)));
    }

    /// A bare command before `@play` reports having no character.
    #[test]
    fn bare_without_actor_reports_no_character() {
        let mut d = Dispatch::new();
        let mut world = World::new();
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        let out = line(&mut d, &mut world, id, "look");
        assert!(matches!(
            out.as_slice(),
            [Outgoing::Event(Event { kind: EventKind::Feedback, .. })]
        ));
    }

    /// End to end through the router: @play then look renders the seeded room.
    #[test]
    fn play_then_look_renders_room() {
        let mut d = Dispatch::new();
        let mut world = World::new();
        musce_action::seed(&mut world);
        let id = ConnectionId(1);
        connect(&mut d, &mut world, id);

        line(&mut d, &mut world, id, "@play");
        let out = line(&mut d, &mut world, id, "look");

        let rendered: Vec<String> = out
            .iter()
            .filter_map(|o| match o {
                Outgoing::Event(Event { text, to: Audience::Connection(_), .. }) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(rendered.iter().any(|t| t.contains("stone hall")));
    }
}
