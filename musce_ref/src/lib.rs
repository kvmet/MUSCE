//! `musce_ref`: the minimal reference game that ships in this repo. It owns all
//! game content (the verbs and how they parse, the seed world, name resolution,
//! narration prose, the takeable rule, the `@play` actor policy) and builds the
//! [`Game`] the engine runtime is parameterized over. The engine crates stay
//! content-free; a real game forks this crate and replaces its content. See
//! `docs/architecture/engine-and-game.md`.

mod admin;
mod exits;
mod kinds;
mod names;
mod seed;
mod sequences;
mod systems;
mod verbs;

use std::sync::Arc;

use musce::Game;
use musce::action::{Action, execute};
use musce::auth::CapRegistry;
use musce::world::World;

/// Build the reference game: its bare and admin command tables, its world seed,
/// its `@play` actor policy, the tick-loop systems it runs, the world-type
/// registration the runtime applies before load, and its capability vocabulary. The
/// admin table interns its caps into the registry as it wires its gates, so the
/// registry is built first and handed over alongside the tables. `main` (and the
/// end-to-end test) pass this to `musce::run`.
pub fn game() -> Game {
    let mut caps = CapRegistry::new();
    let admin = admin::commands(&mut caps);
    Game {
        commands: verbs::commands(),
        admin,
        seed: seed::seed,
        choose_actor: seed::choose_actor,
        systems: vec![
            systems::wander,
            sequences::sequence_sweep,
            systems::death_cry,
        ],
        register: systems::register,
        caps: Arc::new(caps),
        // This game's cold content is a book's text, stored as UTF-8. Decoding is
        // the game's job (it owns the encoding); the engine's cold task calls this
        // and never interprets the bytes. Non-UTF-8 is corruption on this path, so
        // it surfaces a line rather than showing replacement glyphs.
        decode_cold: |bytes| {
            String::from_utf8(bytes.to_vec()).map_err(|_| "The text is unreadable.".to_string())
        },
    }
}

/// Commit an action whose failure modes the caller has already structurally ruled
/// out (a being into a room cannot cycle; a freshly resolved target exists; a
/// hardcoded relation kind is registered). The `Ok` subject is discarded; on the
/// should-never-happen `ExecError` this logs loudly with `context` and returns
/// `false`, so the caller degrades (a bland message, a skipped step) without
/// swallowing the bug silently or panicking the long-lived sim over one wedged
/// mutation. Use `execute` directly when you need the new id back, or when the
/// failure is reachable play (a real cycle a player can construct, as in `take`).
pub(crate) fn commit_or_log(world: &mut World, action: Action, context: &str) -> bool {
    match execute(world, action) {
        Ok(_) => true,
        Err(e) => {
            tracing::error!(error = %e, context, "a structural action failed unexpectedly");
            false
        }
    }
}
