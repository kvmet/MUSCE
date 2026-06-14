//! The executor: the single structural mutation path. `execute` applies one
//! typed `Action` to the world and enforces only the invariants that hold for
//! every source (the entity exists, the containment graph stays acyclic). It runs
//! no gameplay rules and emits no perception events; those live one layer up in
//! the verb handlers. See `docs/architecture/actions.md`.

use std::fmt;

use musce_core::{EntityId, RelationError, World};
use musce_proto::Event;

/// The structural mutation vocabulary. This slice has only containment movement,
/// the typed reflection of `World::move_entity`. The set grows (Relate, Create,
/// Destroy, SetComponent) in later increments; the executor stays this small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Move an entity into a container (a room, a pack, another container).
    Move { entity: EntityId, into: EntityId },
}

/// A structural violation from `execute`. A correct handler validates its rules
/// before committing, so an `ExecError` signals a bug (a handler skipped a check
/// or computed a bad destination), not ordinary rejected play. Thin wrapper over
/// the core `RelationError`.
#[derive(Debug)]
pub enum ExecError {
    Relation(RelationError),
}

impl From<RelationError> for ExecError {
    fn from(e: RelationError) -> Self {
        ExecError::Relation(e)
    }
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::Relation(e) => write!(f, "structural error: {e}"),
        }
    }
}

impl std::error::Error for ExecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ExecError::Relation(e) => Some(e),
        }
    }
}

/// Apply one action to the world. The `sink` is the structural-event channel
/// (reaction systems read low-level facts here); `Move` produces no structural
/// event this slice, so the channel is unused for now. Returns `ExecError` only
/// on a structural violation, which the action's source is expected to have
/// already ruled out.
pub fn execute(
    world: &mut World,
    action: Action,
    _sink: &mut impl FnMut(Event),
) -> Result<(), ExecError> {
    match action {
        Action::Move { entity, into } => {
            world.move_entity(entity, into)?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Container, Item, Room};

    fn noop() -> impl FnMut(Event) {
        |_| {}
    }

    fn room(w: &mut World) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Room);
        w.spawn(b)
    }

    fn container(w: &mut World) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Container);
        w.spawn(b)
    }

    fn item(w: &mut World) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Item);
        w.spawn(b)
    }

    #[test]
    fn move_commits() {
        let mut w = World::new();
        let hall = room(&mut w);
        let sword = item(&mut w);

        execute(&mut w, Action::Move { entity: sword, into: hall }, &mut noop()).unwrap();

        assert_eq!(w.container_of(sword), Some(hall));
        assert_eq!(w.contents(hall), vec![sword]);
    }

    #[test]
    fn move_cycle_is_exec_error() {
        let mut w = World::new();
        let a = container(&mut w);
        let b = container(&mut w);
        execute(&mut w, Action::Move { entity: b, into: a }, &mut noop()).unwrap();

        // a into b would close a loop; the executor reflects the structural reject.
        let err = execute(&mut w, Action::Move { entity: a, into: b }, &mut noop());
        assert!(matches!(err, Err(ExecError::Relation(RelationError::Cycle))));
    }

    #[test]
    fn move_missing_entity_is_exec_error() {
        let mut w = World::new();
        let hall = room(&mut w);
        let ghost = EntityId(9999); // never spawned

        let err = execute(&mut w, Action::Move { entity: ghost, into: hall }, &mut noop());
        assert!(matches!(err, Err(ExecError::Relation(RelationError::NoSuchEntity(_)))));
    }
}
