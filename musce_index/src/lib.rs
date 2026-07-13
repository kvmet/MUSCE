//! `musce_index`: a generic, type-agnostic secondary index over a single
//! component, held as an unpersisted World singleton and rebuilt on load. A game
//! names the component and an optional key function (a spatial hash is one such
//! key) and gets exact and range lookups without scanning the world.
//!
//! Incremental maintenance rides the engine's component-change signal; until that
//! signal exists this crate is a scaffold with no public surface yet.
