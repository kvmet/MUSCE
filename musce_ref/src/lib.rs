//! `musce_ref`: the minimal reference game that ships in this repo. It owns all
//! game content (the verbs and how they parse, the seed world, name resolution,
//! narration prose, the takeable rule, the `@play` actor policy) and builds the
//! [`Game`] the engine runtime is parameterized over. The engine crates stay
//! content-free; a real game forks this crate and replaces its content. See
//! `docs/architecture/engine-and-game.md`.

mod admin;
mod names;
mod seed;
mod systems;
mod verbs;

use musce_host::Game;

/// Build the reference game: its bare and admin command tables, its world seed,
/// its `@play` actor policy, the tick-loop systems it runs, and the world-type
/// registration the runtime applies before load. `main` (and the end-to-end test)
/// pass this to `musce_host::run`.
pub fn game() -> Game {
    Game {
        commands: verbs::commands(),
        admin: admin::commands(),
        seed: seed::seed,
        choose_actor: seed::choose_actor,
        systems: vec![systems::wander, systems::death_cry],
        register: systems::register,
    }
}
