//! End-to-end command-dispatch latency against the reference game's real command
//! table and seeded world. Unlike the core microbenches, this runs a whole line
//! through the engine: parse the verb, gate-check it, run the handler (rule
//! checks, world mutation, output emission), and audience-resolve the events. It
//! measures a read path (`look`, which queries the room and names its contents)
//! and a write path (`go`, a rule-checked move that emits movement facts),
//! bounding both ends of a player's typical latency.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use musce::action::{Actors, Caller, CommandTable, Verdict, dispatch_command};
use musce::wire::ConnectionId;
use musce::world::{EntityId, World};
use musce::{ChooseActor, Register, Seed};

const CONN: ConnectionId = ConnectionId(1);

/// The reference game's real command table, seeded world, and the avatar its
/// `@play` policy picks, bound to one connection: everything `dispatch_command`
/// needs to run a line as a player.
struct Bench {
    commands: CommandTable,
    world: World,
    actor: EntityId,
    actors: Actors,
}

fn setup() -> Bench {
    let game = musce_ref::game();
    let register: Register = game.register;
    let seed: Seed = game.seed;
    let choose: ChooseActor = game.choose_actor;

    let mut world = World::new();
    register(&mut world);
    seed(&mut world);
    let actor = choose(&world).expect("the seed places a player avatar");

    let mut actors = Actors::default();
    actors.bind(CONN, actor);
    Bench {
        commands: game.commands,
        world,
        actor,
        actors,
    }
}

/// Run one command line as the bound actor under a guest verdict (every reference
/// verb is `Gate::Open`), dropping the resolved output.
fn run(bench: &mut Bench, line: &str) {
    let verdict = Verdict::guest();
    let caller = Caller {
        actor: bench.actor,
        conn: CONN,
        verdict: &verdict,
    };
    dispatch_command(
        &bench.commands,
        &mut bench.world,
        &bench.actors,
        caller,
        line,
        &mut |o| {
            black_box(o);
        },
    );
}

fn look(c: &mut Criterion) {
    // Read-only, so one world serves every iteration.
    let mut bench = setup();
    c.bench_function("dispatch/look", |b| {
        b.iter(|| run(&mut bench, "look"));
    });
}

fn go(c: &mut Criterion) {
    // `go` mutates (the avatar changes rooms), so a fresh seed per batch keeps
    // every measured move identical: hall -> garden, never garden -> onward.
    c.bench_function("dispatch/go", |b| {
        b.iter_batched_ref(
            setup,
            |bench| run(bench, "go north"),
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, look, go);
criterion_main!(benches);
