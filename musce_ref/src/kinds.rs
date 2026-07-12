//! The reference game's kind markers: zero-sized components that tag what a thing
//! *is* (an item, a creature, a container, a player avatar). These are game
//! vocabulary, not engine machinery: the engine reasons about identity, relations,
//! containment, and the room perception boundary, but never about what a
//! "creature" is. So they live here and register through `Game.register`, exactly
//! like `Wander`/`Locked`/`Aliases`. A game built on the engine defines its own
//! kinds the same way, without modifying the engine. See
//! `docs/architecture/engine-and-game.md`.

use musce_core::{NamedComponent, World};
use serde::{Deserialize, Serialize};

macro_rules! kind {
    ($name:ident, $tag:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
        pub(crate) struct $name;
        impl NamedComponent for $name {
            const TAG: &'static str = $tag;
        }
    };
}

kind!(
    Item,
    "item",
    "A movable object: takeable, not a fixture or a being."
);
kind!(
    Creature,
    "creature",
    "A living or animate being: an NPC, a mount, a drone."
);
kind!(
    Container,
    "container",
    "A thing other things can be put *in*."
);
kind!(
    Player,
    "player",
    "A player avatar: the entity a connection comes to drive."
);
kind!(
    Exit,
    "exit",
    "An exit entity's kind tag. Connectivity itself (the `LeadsFrom`/`LeadsTo` \
     relations) is game vocabulary too, defined in `crate::exits` over the engine's \
     public relation layer; this marks the kind game rules filter on (`go`, \
     `is_takeable`)."
);

/// Register the game's kind markers so they persist and reload. Called from the
/// game's `register` hook, before any world loads or seeds. The persisted tags
/// (`"item"`, `"creature"`, ...) are unchanged from when these lived in the
/// engine, so existing databases load without migration.
pub(crate) fn register(world: &mut World) {
    world.register_component::<Item>();
    world.register_component::<Creature>();
    world.register_component::<Container>();
    world.register_component::<Player>();
    world.register_component::<Exit>();
}
