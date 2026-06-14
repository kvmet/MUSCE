//! `musce_ref`: the minimal reference game that ships in this repo. It owns all
//! game content (the verbs and how they parse, the seed world, name resolution,
//! narration prose, the takeable rule, the `@play` actor policy) and builds the
//! [`Game`] the engine runtime is parameterized over. The engine crates stay
//! content-free; a real game forks this crate and replaces its content. See
//! `docs/architecture/engine-and-game.md`.

mod names;
mod seed;
mod verbs;

use musce_host::Game;

/// Build the reference game: its command table, its world seed, and its `@play`
/// actor policy. `main` (and the end-to-end test) pass this to `musce_host::run`.
pub fn game() -> Game {
    Game {
        commands: verbs::commands(),
        seed: seed::seed,
        choose_actor: seed::choose_actor,
    }
}
