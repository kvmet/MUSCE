//! The reference game's starting world and its `@play` actor policy. The seed is
//! spawned into an empty database on first boot so there is ground truth to play
//! and test against before any builder tools exist; `choose_actor` is the policy
//! the runtime injects for `@play`, choosing which actor a connection comes to
//! drive. Both are game content over the world API the engine exposes. See
//! `docs/architecture/engine-and-game.md`.

use musce_core::hecs::EntityBuilder;
use musce_core::{Controls, Description, EntityId, Locus, Name, World};

use crate::exits::{LeadsFrom, LeadsTo};

use crate::kinds::{Container, Creature, Exit, Item, Player};
use crate::names::Aliases;
use crate::sequences::{Intent, Step, Steps, attach};
use crate::verbs::{Health, Special};

/// Ticks between the patrolling sentry's steps, and the torch's burn-out lifetime.
/// Sized against the e2e harness rather than arbitrarily small: at its 10ms tick
/// rate a 40-tick patrol step is 400ms, longer than the 300ms read gap the e2e
/// uses to delimit a response burst, so the sentry's ambient narration never
/// starves that gap and hangs an unrelated test. The torch outlives the `@play`
/// handshake so its burn-out cry has a connected listener.
const PATROL_STEP: u32 = 40;
const TORCH_LIFETIME: u32 = 60;

/// Build the starter map into an empty world: a hall, a garden to its north, and
/// a cellar below it; a takeable key in the garden; a player avatar standing in
/// the hall; and a patrol drone beside it that the avatar controls, to exercise
/// `pilot`/`release`. Matches the `fn(&mut World)` shape the runtime's
/// `Game.seed` expects.
///
/// The drone ships with a `Controls` edge to the avatar so there is a controllable
/// thing in the world out of the box for `pilot`/`release` to exercise. See
/// `docs/architecture/networking-and-sessions.md`.
pub fn seed(world: &mut World) {
    let hall = room(world, "a stone hall, its flagstones worn smooth");
    let garden = room(world, "a quiet walled garden");
    let cellar = room(world, "a damp, low-ceilinged cellar");

    link(world, hall, garden, "north");
    link(world, hall, cellar, "down");
    link(world, garden, hall, "south");
    link(world, cellar, hall, "up");

    let key = item(
        world,
        "a brass key",
        "A small brass key, its teeth worn smooth. It may fit an old lock.",
    );
    world.move_entity(key, garden).expect("seed: place key");

    // A container and a loose item to exercise `put`/`give`: a heavy chest that
    // stays put (no `Item` marker, so it is not takeable) and a coin the player can
    // pick up and either stash in the chest or hand to a being.
    let chest = spawn(world, |b| {
        b.add(Container);
        b.add(Name("a wooden chest".into()));
        b.add(Description(
            "A banded wooden chest, lid thrown back, roomy enough to stash things in.".into(),
        ));
    });
    world.move_entity(chest, hall).expect("seed: place chest");
    let coin = item(
        world,
        "a copper coin",
        "A dull copper coin, edges worn round.",
    );
    world.move_entity(coin, hall).expect("seed: place coin");

    let avatar = avatar(
        world,
        "a weathered adventurer",
        "A weathered adventurer, road dust still on well-worn boots.",
    );
    world.move_entity(avatar, hall).expect("seed: place avatar");

    let drone = creature(
        world,
        "a patrol drone",
        "A battered patrol drone idles on its treads, lenses whirring as they track you.",
    );
    world.move_entity(drone, hall).expect("seed: place drone");
    world
        .relate::<Controls>(drone, avatar)
        .expect("seed: wire control");

    // The two sequence demonstrators on one skeleton: a clockwork sentry that
    // patrols hall <-> garden forever (a repeating movement program, its "wait"
    // beats expressed as the inter-step delays), and a torch that burns out (a
    // finite program whose terminal beat destroys the carrier, which the
    // `death_cry` reaction then narrates). See `docs/architecture/sequences.md`.
    let patrol = program(
        world,
        vec![
            Step {
                delay: PATROL_STEP,
                intent: Intent::Move {
                    dir: "north".into(),
                },
            },
            Step {
                delay: PATROL_STEP,
                intent: Intent::Move {
                    dir: "south".into(),
                },
            },
        ],
    );
    let sentry = creature(
        world,
        "a clockwork sentry",
        "A clockwork sentry paces a fixed beat, gears ticking behind a dented breastplate.",
    );
    world.move_entity(sentry, hall).expect("seed: place sentry");
    attach(world, sentry, patrol, true).expect("seed: attach patrol");

    let burn = program(
        world,
        vec![Step {
            delay: TORCH_LIFETIME,
            intent: Intent::Destroy,
        }],
    );
    // The torch carries aliases (`light`, `flame`) that its name does not contain,
    // to exercise the resolver's alias tier out of the box.
    let torch = spawn(world, |b| {
        b.add(Item);
        b.add(Name("a guttering torch".into()));
        b.add(Description(
            "A pitch-soaked torch, its flame guttering low as it burns toward the grip.".into(),
        ));
        b.add(Aliases(vec!["light".into(), "flame".into()]));
    });
    world.move_entity(torch, hall).expect("seed: place torch");
    attach(world, torch, burn, false).expect("seed: attach torch burn-out");

    // A stationary foe for combat: a giant rat in the cellar, with hit points but
    // no `Wander` marker so it holds still to be fought. The avatar's Strength-5
    // blows drop its 8 HP in two hits, the second firing the death cry.
    let rat = spawn(world, |b| {
        b.add(Creature);
        b.add(Name("a giant rat".into()));
        b.add(Description(
            "A giant rat the size of a dog, yellow teeth bared, cornered and hissing.".into(),
        ));
        b.add(Health { current: 8, max: 8 });
    });
    world.move_entity(rat, cellar).expect("seed: place rat");
}

/// Spawn a program entity carrying a `Steps` list. A program is location-less,
/// shared content referenced by id from a `Sequences` instance.
fn program(world: &mut World, steps: Vec<Step>) -> EntityId {
    spawn(world, |b| {
        b.add(Steps(steps));
    })
}

/// The `@play` policy: choose which actor a connection comes to drive. The floor
/// records the attachment; this only selects. For now that is the seeded player
/// avatar; the real flow will resolve the account's chosen character.
pub fn choose_actor(world: &World) -> Option<EntityId> {
    find_player(world)
}

/// Find the player avatar in the world. Returns the first `Player` entity, of
/// which the seed makes exactly one. The real flow will instead resolve the
/// account's chosen character.
fn find_player(world: &World) -> Option<EntityId> {
    world
        .ecs
        .query::<(&musce_core::Id, &Player)>()
        .iter()
        .next()
        .map(|(id, _)| id.0)
}

fn room(world: &mut World, desc: &str) -> EntityId {
    spawn(world, |b| {
        b.add(Locus);
        b.add(Description(desc.into()));
    })
}

fn item(world: &mut World, name: &str, desc: &str) -> EntityId {
    spawn(world, |b| {
        b.add(Item);
        b.add(Name(name.into()));
        b.add(Description(desc.into()));
    })
}

fn avatar(world: &mut World, name: &str, desc: &str) -> EntityId {
    spawn(world, |b| {
        b.add(Player);
        // No permission marker on the body: admin authority is account-scoped now
        // (the loopback `@operator` stub in slice 1). See
        // docs/architecture/authorization.md.
        b.add(Name(name.into()));
        b.add(Description(desc.into()));
        // A stat block so the avatar can fight: `attack` reads Strength for damage.
        // No `Health` yet, because nothing damages the player back (retaliation is
        // a later consumer).
        b.add(Special {
            strength: 5,
            ..Default::default()
        });
    })
}

fn creature(world: &mut World, name: &str, desc: &str) -> EntityId {
    spawn(world, |b| {
        b.add(Creature);
        b.add(Name(name.into()));
        b.add(Description(desc.into()));
    })
}

fn spawn(world: &mut World, f: impl FnOnce(&mut EntityBuilder)) -> EntityId {
    let mut b = EntityBuilder::new();
    f(&mut b);
    world.spawn(b)
}

/// Spawn an exit entity leading from `from` to `to`, named `name` (its direction),
/// wiring both endpoint relations.
fn link(world: &mut World, from: EntityId, to: EntityId, name: &str) {
    let exit = spawn(world, |b| {
        b.add(Exit);
        b.add(Name(name.into()));
    });
    world
        .relate::<LeadsFrom>(exit, from)
        .expect("seed: exit origin");
    world
        .relate::<LeadsTo>(exit, to)
        .expect("seed: exit destination");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exits::ExitQueries;

    #[test]
    fn seed_links_rooms_and_places_things() {
        let mut w = World::new();
        seed(&mut w);

        // The seed makes exactly one player avatar, standing in a room.
        let avatar = find_player(&w).expect("seed places a player");
        let start = w.enclosing_locus(avatar).expect("avatar is in a room");

        // North out of the start room reaches a room.
        let north = w
            .exits_of(start)
            .into_iter()
            .find(|&e| w.name_of(e).as_deref() == Some("north"))
            .expect("a north exit out of the start room");
        assert!(w.exit_destination(north).is_some());
    }

    #[test]
    fn choose_actor_selects_the_seeded_avatar() {
        let mut w = World::new();
        seed(&mut w);
        assert_eq!(choose_actor(&w), find_player(&w));
        assert!(choose_actor(&w).is_some());
    }

    #[test]
    fn seed_wires_a_controllable_drone() {
        let mut w = World::new();
        seed(&mut w);
        let avatar = find_player(&w).expect("seed places a player");

        // The avatar controls exactly one thing, in the same room as the avatar.
        let controlled = w.sources_of::<Controls>(avatar);
        assert_eq!(controlled.len(), 1);
        let drone = controlled[0];
        assert_eq!(w.target_of::<Controls>(drone), Some(avatar));
        assert_eq!(w.enclosing_locus(drone), w.enclosing_locus(avatar));
    }

    #[test]
    fn seed_wires_the_sequence_demonstrators() {
        use crate::sequences::Sequences;
        let mut w = World::new();
        seed(&mut w);

        // The sentry runs one repeating patrol; the torch runs one finite burn.
        let sentry = find_described(&w, "clockwork sentry").expect("a seeded sentry");
        let patrol = sequences_of(&w, sentry);
        assert_eq!(patrol.len(), 1);
        assert!(patrol[0].repeat, "the patrol repeats");

        let torch = find_described(&w, "guttering torch").expect("a seeded torch");
        let burn = sequences_of(&w, torch);
        assert_eq!(burn.len(), 1);
        assert!(!burn[0].repeat, "the torch is a one-shot");

        fn sequences_of(w: &World, e: EntityId) -> Vec<crate::sequences::Instance> {
            w.entity(e)
                .and_then(|er| er.get::<&Sequences>().map(|s| s.0.clone()))
                .expect("entity carries Sequences")
        }
    }

    /// First entity whose `Name` contains `needle`, for finding seeded content by
    /// its handle in tests.
    fn find_described(w: &World, needle: &str) -> Option<EntityId> {
        w.ecs
            .query::<(&musce_core::Id, &Name)>()
            .iter()
            .find(|(_, n)| n.0.contains(needle))
            .map(|(id, _)| id.0)
    }
}
