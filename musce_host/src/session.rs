//! The session floor: the always-present bottom of the input stack. Once a
//! connection is authenticated it has a session here, and `@`-namespaced account
//! commands route to it no matter what is overlaid on top. This slice is the
//! floor only: auth is stubbed (every connection is an anonymous guest), and the
//! control/embodiment stack above it is not built yet, so bare commands have no
//! frame to act through.
//!
//! Session state is server-side and keyed by connection (later: by account, so
//! it survives a reconnect). It is *not* world state. See
//! `docs/architecture/networking-and-sessions.md`.

use std::collections::HashMap;

use musce_net::{Command, ConnectionId, Event, EventKind, Input, Outgoing};

/// One live session. A marker for now (its presence means the connection is
/// authenticated); it will grow to hold the account id and character slots.
struct Session;

/// The floor for every connection. Owns the session table and dispatches
/// commands. Holds no world reference: nothing here mutates the world yet.
#[derive(Default)]
pub struct Sessions {
    map: HashMap<ConnectionId, Session>,
}

impl Sessions {
    /// Dispatch one command, pushing any output through `emit`. The caller owns
    /// the outbox; this stays free of the channel so it is trivially testable.
    pub fn handle(&mut self, cmd: Command, emit: &mut impl FnMut(Outgoing)) {
        let id = cmd.connection;
        match cmd.input {
            Input::Connected { .. } => {
                self.map.insert(id, Session);
                emit(Outgoing::Event(Event::to_connection(
                    id,
                    EventKind::System,
                    "Welcome to MUSCE. Type @quit to disconnect, @help for commands.",
                )));
            }
            Input::Line(line) => self.handle_line(id, line.trim(), emit),
            Input::Disconnected => {
                self.map.remove(&id);
            }
        }
    }

    fn handle_line(&mut self, id: ConnectionId, line: &str, emit: &mut impl FnMut(Outgoing)) {
        if !self.map.contains_key(&id) {
            // Input from a connection with no session: net got ahead of us, or a
            // command landed after teardown. Nothing to act on.
            return;
        }
        if line.is_empty() {
            return;
        }

        if let Some(rest) = line.strip_prefix('@') {
            self.account_command(id, rest, emit);
        } else {
            // No control stack yet, so a bare command has no frame to act on.
            feedback(id, "You aren't controlling anything yet. Try @help.", emit);
        }
    }

    /// `@`-namespaced account commands. These are the floor and stay reachable
    /// regardless of what will later sit on top of the input stack.
    fn account_command(&mut self, id: ConnectionId, rest: &str, emit: &mut impl FnMut(Outgoing)) {
        let mut parts = rest.split_whitespace();
        let verb = parts.next().unwrap_or("");
        match verb {
            "quit" => {
                feedback(id, "Goodbye.", emit);
                emit(Outgoing::Close(id));
            }
            "who" => {
                feedback(id, &format!("{} connection(s) online.", self.map.len()), emit);
            }
            "help" => {
                feedback(id, "Commands: @quit, @who, @help", emit);
            }
            other => {
                feedback(id, &format!("Unknown command: @{other}"), emit);
            }
        }
    }
}

fn feedback(id: ConnectionId, text: &str, emit: &mut impl FnMut(Outgoing)) {
    emit(Outgoing::Event(Event::to_connection(id, EventKind::Feedback, text)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_net::{Audience, Capabilities};

    fn caps() -> Capabilities {
        Capabilities { color: false, line_mode_only: true, size: None }
    }

    fn drain(sessions: &mut Sessions, cmd: Command) -> Vec<Outgoing> {
        let mut out = Vec::new();
        sessions.handle(cmd, &mut |o| out.push(o));
        out
    }

    fn connect(sessions: &mut Sessions, id: ConnectionId) {
        drain(sessions, Command { connection: id, input: Input::Connected { caps: caps(), peer: None } });
    }

    fn line(sessions: &mut Sessions, id: ConnectionId, s: &str) -> Vec<Outgoing> {
        drain(sessions, Command { connection: id, input: Input::Line(s.into()) })
    }

    #[test]
    fn connect_greets() {
        let mut s = Sessions::default();
        let id = ConnectionId(1);
        let out = drain(&mut s, Command { connection: id, input: Input::Connected { caps: caps(), peer: None } });
        assert!(matches!(
            out.as_slice(),
            [Outgoing::Event(Event { kind: EventKind::System, to: Audience::Connection(c), .. })] if *c == id
        ));
    }

    #[test]
    fn quit_emits_close_for_that_connection() {
        let mut s = Sessions::default();
        let id = ConnectionId(7);
        connect(&mut s, id);
        let out = line(&mut s, id, "@quit");

        // A goodbye, then a Close addressed to this connection.
        assert!(matches!(out[0], Outgoing::Event(Event { kind: EventKind::Feedback, .. })));
        assert!(matches!(out[1], Outgoing::Close(c) if c == id));
    }

    #[test]
    fn unknown_at_command_feeds_back() {
        let mut s = Sessions::default();
        let id = ConnectionId(2);
        connect(&mut s, id);
        let out = line(&mut s, id, "@bogus");
        match &out[..] {
            [Outgoing::Event(Event { kind: EventKind::Feedback, text, .. })] => {
                assert!(text.contains("@bogus"));
            }
            other => panic!("expected one feedback event, got {other:?}"),
        }
    }

    #[test]
    fn bare_command_reports_no_embodiment() {
        let mut s = Sessions::default();
        let id = ConnectionId(3);
        connect(&mut s, id);
        let out = line(&mut s, id, "look");
        match &out[..] {
            [Outgoing::Event(Event { kind: EventKind::Feedback, .. })] => {}
            other => panic!("expected one feedback event, got {other:?}"),
        }
    }

    #[test]
    fn line_after_disconnect_is_silent() {
        let mut s = Sessions::default();
        let id = ConnectionId(4);
        connect(&mut s, id);
        drain(&mut s, Command { connection: id, input: Input::Disconnected });
        assert!(line(&mut s, id, "@who").is_empty());
    }
}
