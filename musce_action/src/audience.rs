//! Sim-side audience resolution. Verb handlers address output semantically (to a
//! locus, to an entity, to a connection); turning `Locus`/`Entity` into the
//! connections that should actually see it needs world state (who is in the locus)
//! and the connection<->actor map, so it happens here, before output reaches net.
//! Resolution produces `Delivery`s (already bound to a connection), so net is left
//! a pure pipe that can never receive an unresolved audience. See
//! `docs/architecture/actions.md`.

use musce_core::{EntityId, World};
use musce_proto::{Delivery, Outgoing};

use crate::bindings::Actors;
use crate::event::{Audience, Event};

/// One handler-emitted piece of output before audience resolution. `exclude` names
/// the entities to omit when expanding a broadcast: a verb sends the actor a
/// first-person line directly and a third-person line to the locus *except* the
/// actor, so the actor never sees both. A directed act (A waves at B) excludes both
/// parties from the locus line, since each already got their own line. Exclusion is
/// by entity, not connection, because handlers speak entities; each excluded entity
/// resolves to its driving connection(s) here, where the `Actors` index is on hand.
#[derive(Debug, Clone)]
pub struct Outbound {
    pub event: Event,
    pub exclude: Vec<EntityId>,
}

impl Outbound {
    pub fn new(event: Event) -> Self {
        Outbound {
            event,
            exclude: Vec::new(),
        }
    }

    pub fn excluding(event: Event, entities: Vec<EntityId>) -> Self {
        Outbound {
            event,
            exclude: entities,
        }
    }
}

/// Expand one `Outbound` into connection-bound `Delivery`s, pushing each through
/// `emit`. A `Connection` audience passes through; `Entity` fans out to every
/// connection driving that entity; `Locus` fans out to every connection whose
/// actor stands directly in the locus.
pub fn resolve(world: &World, actors: &Actors, out: Outbound, emit: &mut impl FnMut(Outgoing)) {
    let Outbound { event, exclude } = out;

    let excluded_conns: Vec<musce_proto::ConnectionId> =
        exclude.iter().flat_map(|e| actors.conns_for(*e)).collect();

    let mut deliver = |conn| {
        if excluded_conns.contains(&conn) {
            return;
        }
        emit(Outgoing::Event(Delivery {
            to: conn,
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
        Audience::Locus(locus) => {
            for occupant in world.contents(locus) {
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
    use musce_core::Locus;
    use musce_core::hecs::EntityBuilder;
    use musce_proto::{ConnectionId, Delivery, EventKind};

    fn collect(world: &World, actors: &Actors, out: Outbound) -> Vec<Outgoing> {
        let mut v = Vec::new();
        resolve(world, actors, out, &mut |o| v.push(o));
        v
    }

    // An actor is just an entity a connection binds to; "player" is a game kind,
    // not needed to exercise audience routing.
    fn player(w: &mut World) -> EntityId {
        w.spawn(EntityBuilder::new())
    }

    #[test]
    fn locus_event_reaches_every_connected_actor() {
        let mut w = World::new();
        let locus = {
            let mut b = EntityBuilder::new();
            b.add(Locus);
            w.spawn(b)
        };
        let alice = player(&mut w);
        let bob = player(&mut w);
        w.move_entity(alice, locus).unwrap();
        w.move_entity(bob, locus).unwrap();

        let mut actors = Actors::default();
        actors.bind(ConnectionId(1), alice);
        actors.bind(ConnectionId(2), bob);

        let out = Outbound::new(Event::to_locus(locus, EventKind::Narration, "a bell rings"));
        let events = collect(&w, &actors, out);

        assert_eq!(events.len(), 2);
        let conns: Vec<ConnectionId> = events
            .iter()
            .map(|o| match o {
                Outgoing::Event(Delivery { to: c, .. }) => *c,
                other => panic!("expected connection event, got {other:?}"),
            })
            .collect();
        assert!(conns.contains(&ConnectionId(1)));
        assert!(conns.contains(&ConnectionId(2)));
    }

    #[test]
    fn exclude_drops_the_actor() {
        let mut w = World::new();
        let locus = {
            let mut b = EntityBuilder::new();
            b.add(Locus);
            w.spawn(b)
        };
        let alice = player(&mut w);
        let bob = player(&mut w);
        w.move_entity(alice, locus).unwrap();
        w.move_entity(bob, locus).unwrap();

        let mut actors = Actors::default();
        actors.bind(ConnectionId(1), alice);
        actors.bind(ConnectionId(2), bob);

        let out = Outbound::excluding(
            Event::to_locus(locus, EventKind::Narration, "Alice waves"),
            vec![alice],
        );
        let events = collect(&w, &actors, out);

        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            Outgoing::Event(Delivery { to: c, .. }) if c == ConnectionId(2)
        ));
    }

    #[test]
    fn exclude_drops_a_set() {
        let mut w = World::new();
        let locus = {
            let mut b = EntityBuilder::new();
            b.add(Locus);
            w.spawn(b)
        };
        let alice = player(&mut w);
        let bob = player(&mut w);
        let carol = player(&mut w);
        w.move_entity(alice, locus).unwrap();
        w.move_entity(bob, locus).unwrap();
        w.move_entity(carol, locus).unwrap();

        let mut actors = Actors::default();
        actors.bind(ConnectionId(1), alice);
        actors.bind(ConnectionId(2), bob);
        actors.bind(ConnectionId(3), carol);

        // Alice waves at Bob: both got their own line, so only Carol sees the locus's.
        let out = Outbound::excluding(
            Event::to_locus(locus, EventKind::Narration, "Alice waves at Bob"),
            vec![alice, bob],
        );
        let events = collect(&w, &actors, out);

        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            Outgoing::Event(Delivery { to: c, .. }) if c == ConnectionId(3)
        ));
    }
}
