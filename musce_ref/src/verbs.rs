//! The reference game's in-game verb handlers: the meaning layer over the
//! engine's structural executor. Each is shaped validate -> mutate -> emit.
//! Fallible rule checks (reach, "you don't see that") run first and produce
//! player-facing feedback (a Rejection); only then does the handler commit
//! through `execute`, which cannot fail because the checks already ruled the
//! structural error out. Output is emitted through the engine's `Ctx` emit API;
//! the dispatcher resolves audiences afterward. See
//! `docs/architecture/actions.md`.
//!
//! The handlers are grouped by concern into submodules (`observe`, `movement`,
//! `manipulate`, `control`, `social`); this root owns the command table that
//! registers them and the `help` verb that lists them.

use musce_action::{CommandTable, Ctx, Gate};
use musce_proto::EventKind;

mod combat;
mod containers;
mod control;
mod manipulate;
mod movement;
mod observe;
mod social;
#[cfg(test)]
mod tests;

pub use combat::attack;
pub use containers::{give, put};
pub use control::{pilot, release};
pub use manipulate::{drop, take};
pub use movement::go;
pub use observe::{examine, inventory, look};
pub use social::{say, tell, wave};

// Shared with the tick-loop movers (`wander`, sequences), which route through the
// one rule-checked move path; the `Locked` marker is registered for persistence.
pub(crate) use movement::{Locked, MoveOutcome, do_move};
// The combat stat components: read by `attack`, seeded on the avatar and its foes,
// and registered for persistence in `systems::register`.
pub(crate) use combat::{Health, Special};

/// Build the reference game's command table. Movement is registered first so
/// single-letter direction abbreviations win their prefix ties (`s` is south, so
/// `say` needs `sa`).
pub fn commands() -> CommandTable {
    let mut t = CommandTable::new();
    t.register("north", Gate::Open, |c, _| go(c, "north"));
    t.register("south", Gate::Open, |c, _| go(c, "south"));
    t.register("east", Gate::Open, |c, _| go(c, "east"));
    t.register("west", Gate::Open, |c, _| go(c, "west"));
    t.register("up", Gate::Open, |c, _| go(c, "up"));
    t.register("down", Gate::Open, |c, _| go(c, "down"));
    t.register("look", Gate::Open, look);
    t.register("examine", Gate::Open, examine);
    t.register("x", Gate::Open, examine);
    t.register("inventory", Gate::Open, inventory);
    t.register("go", Gate::Open, go);
    t.register("take", Gate::Open, take);
    t.register("drop", Gate::Open, drop);
    t.register("put", Gate::Open, put);
    t.register("give", Gate::Open, give);
    t.register("pilot", Gate::Open, pilot);
    t.register("release", Gate::Open, release);
    t.register("say", Gate::Open, say);
    t.register("tell", Gate::Open, tell);
    t.register("wave", Gate::Open, wave);
    t.register("attack", Gate::Open, attack);
    t.register("kill", Gate::Open, attack);
    t.register("help", Gate::Open, help);
    t
}

/// `help`: list the in-world verbs. This is the game's surface, so the game
/// documents it; the engine floor's `@help` covers only the account commands.
pub fn help(ctx: &mut Ctx, _args: &str) {
    ctx.emit_self(
        EventKind::Feedback,
        "You can: look, examine <thing> (or x), inventory (or i), \
         go <direction> (or just a direction), take <item>, drop <item>, \
         put <item> in <container>, give <item> to <someone>, \
         pilot <thing>, release, say <message>, tell <someone> <message>, \
         wave (or wave at <someone>), attack <thing> (or kill), help.",
    );
}
