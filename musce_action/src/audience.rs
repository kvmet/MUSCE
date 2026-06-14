//! Sim-side audience resolution. Verb handlers address output semantically (to a
//! room, to an entity, to a connection); turning `Room`/`Entity` into the
//! connections that should actually see it needs world state (who is in the room)
//! and the connection<->actor map, so it happens here, before output reaches net.
//! Net is left a pure `Connection` pipe. See `docs/architecture/actions.md`.

use musce_core::World;
use musce_proto::{Audience, Event, Outgoing};

use crate::bindings::Actors;

/// One handler-emitted piece of output before audience resolution. `exclude` is
/// the connection to omit when expanding a broadcast: a verb sends the actor a
/// first-person line directly and a third-person line to the room *except* the
/// actor, so the actor never sees both.
#[derive(Debug, Clone)]
pub struct Outbound {
    pub event: Event,
    pub exclude: Option<musce_proto::ConnectionId>,
}

impl Outbound {
    pub fn new(event: Event) -> Self {
        Outbound {
            event,
            exclude: None,
        }
    }

    pub fn excluding(event: Event, conn: musce_proto::ConnectionId) -> Self {
        Outbound {
            event,
            exclude: Some(conn),
        }
    }
}

/// Expand one `Outbound` into `Connection`-addressed `Outgoing` events, pushing
/// each through `emit`. A `Connection` audience passes through; `Entity` fans out
/// to every connection driving that entity; `Room` fans out to every connection
/// whose actor stands directly in the room.
pub fn resolve(world: &World, actors: &Actors, out: Outbound, emit: &mut impl FnMut(Outgoing)) {
    let Outbound { event, exclude } = out;

    let mut deliver = |conn| {
        if Some(conn) == exclude {
            return;
        }
        emit(Outgoing::Event(Event {
            to: Audience::Connection(conn),
            kind: event.kind,
            text: event.text.clone(),
        }));
    };

    match event.to {
        Audience::Connection(conn) => deliver(conn),
        Audience::Entity(entity) => {
            for conn in actors.conns_for(entity) {
                deliver(conn);
            }
        }
        Audience::Room(room) => {
            for occupant in world.contents(room) {
                for conn in actors.conns_for(occupant) {
                    deliver(conn);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::EntityId;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Player, Room};
    use musce_proto::{ConnectionId, EventKind};

    fn collect(world: &World, actors: &Actors, out: Outbound) -> Vec<Outgoing> {
        let mut v = Vec::new();
        resolve(world, actors, out, &mut |o| v.push(o));
        v
    }

    fn player(w: &mut World) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Player);
        w.spawn(b)
    }

    #[test]
    fn room_event_reaches_every_connected_actor() {
        let mut w = World::new();
        let room = {
            let mut b = EntityBuilder::new();
            b.add(Room);
            w.spawn(b)
        };
        let alice = player(&mut w);
        let bob = player(&mut w);
        w.move_entity(alice, room).unwrap();
        w.move_entity(bob, room).unwrap();

        let mut actors = Actors::default();
        actors.bind(ConnectionId(1), alice);
        actors.bind(ConnectionId(2), bob);

        let out = Outbound::new(Event::to_room(room, EventKind::Narration, "a bell rings"));
        let events = collect(&w, &actors, out);

        assert_eq!(events.len(), 2);
        let conns: Vec<ConnectionId> = events
            .iter()
            .map(|o| match o {
                Outgoing::Event(Event {
                    to: Audience::Connection(c),
                    ..
                }) => *c,
                other => panic!("expected connection event, got {other:?}"),
            })
            .collect();
        assert!(conns.contains(&ConnectionId(1)));
        assert!(conns.contains(&ConnectionId(2)));
    }

    #[test]
    fn exclude_drops_the_actor() {
        let mut w = World::new();
        let room = {
            let mut b = EntityBuilder::new();
            b.add(Room);
            w.spawn(b)
        };
        let alice = player(&mut w);
        let bob = player(&mut w);
        w.move_entity(alice, room).unwrap();
        w.move_entity(bob, room).unwrap();

        let mut actors = Actors::default();
        actors.bind(ConnectionId(1), alice);
        actors.bind(ConnectionId(2), bob);

        let out = Outbound::excluding(
            Event::to_room(room, EventKind::Narration, "Alice waves"),
            ConnectionId(1),
        );
        let events = collect(&w, &actors, out);

        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            Outgoing::Event(Event { to: Audience::Connection(c), .. }) if c == ConnectionId(2)
        ));
    }
}
