//! The executor: the single structural mutation path. `execute` applies one
//! typed `Action` to the world and enforces only the invariants that hold for
//! every source (the entity exists, the containment graph stays acyclic). It runs
//! no gameplay rules and emits no perception events; those live one layer up in
//! the verb handlers. See `docs/architecture/actions.md`.

use std::fmt;

use musce_core::{EntityId, MutateError, RelationError, Value, World};
use musce_proto::Event;

/// The structural mutation vocabulary: the typed reflection of the bucket-1
/// `World` mutators. Movement, lifecycle, and type-erased component edits. The
/// executor stays this small; gameplay rules and perception live one layer up.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Move an entity into a container (a room, a pack, another container).
    Move { entity: EntityId, into: EntityId },
    /// Spawn a root entity from a tag->value component blob. Location-less:
    /// placement is a separate `Move` the caller composes when it makes sense.
    Create { components: Value },
    /// Despawn an entity. Its contents reparent up per the containment cascade.
    Destroy { entity: EntityId },
    /// Deserialize-and-overwrite one component on a live entity (whole-component).
    SetComponent {
        entity: EntityId,
        tag: String,
        value: Value,
    },
    /// Remove one component by tag from a live entity.
    RemoveComponent { entity: EntityId, tag: String },
}

/// A structural violation from `execute`. A correct handler validates its rules
/// before committing, so an `ExecError` signals a bug (a handler skipped a check
/// or computed a bad destination), not ordinary rejected play. Thin wrapper over
/// the core mutation errors.
#[derive(Debug)]
pub enum ExecError {
    Relation(RelationError),
    Mutate(MutateError),
}

impl From<RelationError> for ExecError {
    fn from(e: RelationError) -> Self {
        ExecError::Relation(e)
    }
}

impl From<MutateError> for ExecError {
    fn from(e: MutateError) -> Self {
        ExecError::Mutate(e)
    }
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::Relation(e) => write!(f, "structural error: {e}"),
            ExecError::Mutate(e) => write!(f, "structural error: {e}"),
        }
    }
}

impl std::error::Error for ExecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ExecError::Relation(e) => Some(e),
            ExecError::Mutate(e) => Some(e),
        }
    }
}

/// Apply one action to the world. The `sink` is the structural-event channel
/// (reaction systems read low-level facts here); no arm produces a structural
/// event yet, so the channel is unused for now. Returns the action's **subject**
/// (the moved/created/destroyed/edited entity); for `Create` that is the only way
/// the caller learns the new id. Returns `ExecError` only on a structural
/// violation, which the action's source is expected to have already ruled out.
pub fn execute(
    world: &mut World,
    action: Action,
    _sink: &mut impl FnMut(Event),
) -> Result<EntityId, ExecError> {
    match action {
        Action::Move { entity, into } => {
            world.move_entity(entity, into)?;
            Ok(entity)
        }
        Action::Create { components } => Ok(world.create(&components)?),
        Action::Destroy { entity } => {
            world.despawn(entity);
            Ok(entity)
        }
        Action::SetComponent { entity, tag, value } => {
            world.set_component(entity, &tag, value)?;
            Ok(entity)
        }
        Action::RemoveComponent { entity, tag } => {
            world.remove_component(entity, &tag)?;
            Ok(entity)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_core::hecs::EntityBuilder;
    use musce_core::{Container, Description, Item, Map, MutateError, Room};

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

    /// A component blob `{ "item": null, "description": <desc> }`, built through
    /// the re-exported JSON types (the action layer has no serde_json of its own).
    fn item_blob(desc: &str) -> Value {
        let mut m = Map::new();
        m.insert("item".into(), Value::Null);
        m.insert("description".into(), Value::String(desc.into()));
        Value::Object(m)
    }

    #[test]
    fn move_commits() {
        let mut w = World::new();
        let hall = room(&mut w);
        let sword = item(&mut w);

        execute(
            &mut w,
            Action::Move {
                entity: sword,
                into: hall,
            },
            &mut noop(),
        )
        .unwrap();

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
        assert!(matches!(
            err,
            Err(ExecError::Relation(RelationError::Cycle))
        ));
    }

    #[test]
    fn move_missing_entity_is_exec_error() {
        let mut w = World::new();
        let hall = room(&mut w);
        let ghost = EntityId(9999); // never spawned

        let err = execute(
            &mut w,
            Action::Move {
                entity: ghost,
                into: hall,
            },
            &mut noop(),
        );
        assert!(matches!(
            err,
            Err(ExecError::Relation(RelationError::NoSuchEntity(_)))
        ));
    }

    #[test]
    fn move_returns_its_subject() {
        let mut w = World::new();
        let hall = room(&mut w);
        let sword = item(&mut w);
        let subject = execute(
            &mut w,
            Action::Move {
                entity: sword,
                into: hall,
            },
            &mut noop(),
        )
        .unwrap();
        assert_eq!(subject, sword);
    }

    #[test]
    fn create_returns_new_id_with_components() {
        let mut w = World::new();
        let id = execute(
            &mut w,
            Action::Create {
                components: item_blob("a torch"),
            },
            &mut noop(),
        )
        .unwrap();

        assert!(w.has::<Item>(id));
        assert_eq!(
            w.entity(id).unwrap().get::<&Description>().unwrap().0,
            "a torch"
        );
    }

    #[test]
    fn destroy_removes_entity_and_reparents_contents() {
        let mut w = World::new();
        let hall = room(&mut w);
        let bag = container(&mut w);
        let coin = item(&mut w);
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();

        let subject = execute(&mut w, Action::Destroy { entity: bag }, &mut noop()).unwrap();

        assert_eq!(subject, bag);
        assert!(w.entity(bag).is_none());
        // The Reparent cascade spills the bag's contents up to the hall.
        assert_eq!(w.container_of(coin), Some(hall));
    }

    #[test]
    fn set_and_remove_component_apply() {
        let mut w = World::new();
        let it = item(&mut w);

        execute(
            &mut w,
            Action::SetComponent {
                entity: it,
                tag: "description".into(),
                value: Value::String("a worn map".into()),
            },
            &mut noop(),
        )
        .unwrap();
        assert_eq!(
            w.component_value(it, "description"),
            Some(Value::String("a worn map".into()))
        );

        execute(
            &mut w,
            Action::RemoveComponent {
                entity: it,
                tag: "description".into(),
            },
            &mut noop(),
        )
        .unwrap();
        assert_eq!(w.component_value(it, "description"), None);
    }

    #[test]
    fn structural_violations_surface_as_exec_error() {
        let mut w = World::new();
        let it = item(&mut w);

        // Relation tag on Create.
        let mut m = Map::new();
        m.insert("contained_by".into(), Value::from(1u64));
        let err = execute(
            &mut w,
            Action::Create {
                components: Value::Object(m),
            },
            &mut noop(),
        );
        assert!(matches!(
            err,
            Err(ExecError::Mutate(MutateError::RelationTag(_)))
        ));

        // Identity tag on SetComponent.
        let err = execute(
            &mut w,
            Action::SetComponent {
                entity: it,
                tag: "id".into(),
                value: Value::from(7u64),
            },
            &mut noop(),
        );
        assert!(matches!(
            err,
            Err(ExecError::Mutate(MutateError::IdentityTag(_)))
        ));
    }

    /// Merge as a read-modify-write done in the caller, standing in for the game:
    /// the engine exposes `component_value` (read) and `SetComponent` (overwrite)
    /// and owns neither the merge nor a verb. Here `Exits` is the multi-field
    /// component; we read it, append one exit, and write it back.
    #[test]
    fn merge_patch_is_a_caller_side_read_modify_write() {
        let mut w = World::new();
        let here = room(&mut w);
        let there = room(&mut w);

        // Seed one exit so there is something to merge into.
        let seed = {
            let mut arr = Map::new();
            arr.insert("direction".into(), Value::String("north".into()));
            arr.insert("to".into(), Value::from(there.0));
            Value::Array(vec![Value::Object(arr)])
        };
        execute(
            &mut w,
            Action::SetComponent {
                entity: here,
                tag: "exits".into(),
                value: seed,
            },
            &mut noop(),
        )
        .unwrap();

        // Read the current value, patch it (append a second exit), write it back.
        let mut current = w.component_value(here, "exits").unwrap();
        let new_exit = {
            let mut o = Map::new();
            o.insert("direction".into(), Value::String("south".into()));
            o.insert("to".into(), Value::from(here.0));
            Value::Object(o)
        };
        current.as_array_mut().unwrap().push(new_exit);
        execute(
            &mut w,
            Action::SetComponent {
                entity: here,
                tag: "exits".into(),
                value: current,
            },
            &mut noop(),
        )
        .unwrap();

        // Both exits are present after the read-modify-write.
        let exits = w
            .entity(here)
            .unwrap()
            .get::<&musce_core::Exits>()
            .unwrap()
            .0
            .clone();
        let dirs: Vec<&str> = exits.iter().map(|e| e.direction.as_str()).collect();
        assert_eq!(dirs, vec!["north", "south"]);
    }
}
