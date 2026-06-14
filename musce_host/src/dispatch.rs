//! The single command entry point the tick loop calls as it drains the inbox.
//! It owns input-stack routing: a command is directed to the right frame (modal
//! overlay, embodiment, or the always-present account/session floor), which
//! turns it into output events and, for in-game frames, world-mutating actions.
//!
//! This slice has only the floor frame, so every command goes there. When the
//! in-game command layer lands (the dispatch table in
//! `docs/architecture/actions.md`) it slots in here as the embodiment frame,
//! taking `&mut World` to run `execute`; the floor then narrows to the
//! `@`-namespace. Keeping this seam means the loop holds no command knowledge: it
//! drains the inbox and calls `handle`.

use musce_core::World;
use musce_net::{Command, Outgoing};

use crate::session::Sessions;

#[derive(Default)]
pub struct Dispatch {
    floor: Sessions,
}

impl Dispatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Route one inbound command, pushing output through `emit`. `world` is the
    /// authority in-game frames mutate through `execute`; the floor does not
    /// touch it yet.
    pub fn handle(
        &mut self,
        cmd: Command,
        _world: &mut World,
        emit: &mut impl FnMut(Outgoing),
    ) {
        self.floor.handle(cmd, emit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_net::{Capabilities, ConnectionId, Input};

    fn caps() -> Capabilities {
        Capabilities { color: false, line_mode_only: true, size: None }
    }

    /// The seam reaches the floor: a connect then `@quit` produces a `Close`.
    #[test]
    fn routes_to_floor() {
        let mut d = Dispatch::new();
        let mut world = World::new();
        let id = ConnectionId(1);
        let mut out = Vec::new();

        d.handle(
            Command { connection: id, input: Input::Connected { caps: caps(), peer: None } },
            &mut world,
            &mut |o| out.push(o),
        );
        d.handle(
            Command { connection: id, input: Input::Line("@quit".into()) },
            &mut world,
            &mut |o| out.push(o),
        );

        assert!(out.iter().any(|o| matches!(o, Outgoing::Close(c) if *c == id)));
    }
}
