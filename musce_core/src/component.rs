use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::id::EntityId;

/// A component that can be persisted, identified by a stable string tag in the
/// per-entity JSON blob.
pub trait NamedComponent: hecs::Component + Serialize + for<'de> Deserialize<'de> {
    const TAG: &'static str;
}

// --- built-in components -------------------------------------------------

/// Global identity, present on every entity. Lets us recover an entity's
/// `EntityId` while iterating, and round-trips through the blob.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Id(pub EntityId);
impl NamedComponent for Id {
    const TAG: &'static str = "id";
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Description(pub String);
impl NamedComponent for Description {
    const TAG: &'static str = "description";
}

/// A short token a player types or sees to refer to an entity (an exit label
/// like "north", later an item keyword). General, not exit-specific; the match
/// key the name resolver keys off. Distinct from Description, which is prose.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Label(pub String);
impl NamedComponent for Label {
    const TAG: &'static str = "label";
}

// Kind markers. Zero-sized; let archetypal queries filter by kind.
macro_rules! marker {
    ($name:ident, $tag:literal) => {
        #[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
        pub struct $name;
        impl NamedComponent for $name {
            const TAG: &'static str = $tag;
        }
    };
}

marker!(Room, "room");
marker!(Item, "item");
// An exit is its own entity, not a field on a room: this marks the kind,
// filters exits in queries, and keeps them out of takeable. Its label is the
// general Label component; its origin and destination are the LeadsFrom and
// LeadsTo relations (see exit.rs).
marker!(Exit, "exit");
marker!(Creature, "creature");
marker!(Container, "container");
marker!(Player, "player");
// Permission marker: an actor carrying it may run staff-gated (admin) verbs. A
// stand-in until accounts own permissions; seeded for now, like `Player`.
marker!(Staff, "staff");

// --- registry ------------------------------------------------------------

type SerFn = for<'a> fn(hecs::EntityRef<'a>, &mut Map<String, Value>);
type DeserFn = fn(&mut hecs::EntityBuilder, Value) -> Result<(), serde_json::Error>;
type InsertFn = fn(&mut hecs::World, hecs::Entity, Value) -> Result<(), serde_json::Error>;
type RemoveFn = fn(&mut hecs::World, hecs::Entity);
type GetFn = for<'a> fn(hecs::EntityRef<'a>) -> Option<Value>;

fn ser_one<C: NamedComponent>(er: hecs::EntityRef, map: &mut Map<String, Value>) {
    if let Some(c) = er.get::<&C>() {
        map.insert(
            C::TAG.to_string(),
            serde_json::to_value(&*c).expect("component serialization is infallible"),
        );
    }
}

fn deser_one<C: NamedComponent>(
    b: &mut hecs::EntityBuilder,
    v: Value,
) -> Result<(), serde_json::Error> {
    let c: C = serde_json::from_value(v)?;
    b.add(c);
    Ok(())
}

/// Deserialize one component and overwrite it on a live entity.
fn insert_one<C: NamedComponent>(
    ecs: &mut hecs::World,
    e: hecs::Entity,
    v: Value,
) -> Result<(), serde_json::Error> {
    let c: C = serde_json::from_value(v)?;
    let _ = ecs.insert_one(e, c);
    Ok(())
}

/// Remove one component from a live entity (no-op if absent).
fn remove_one<C: NamedComponent>(ecs: &mut hecs::World, e: hecs::Entity) {
    let _ = ecs.remove_one::<C>(e);
}

/// Serialize just one named component back to JSON; `None` if absent.
fn get_one<C: NamedComponent>(er: hecs::EntityRef) -> Option<Value> {
    er.get::<&C>()
        .map(|c| serde_json::to_value(&*c).expect("component serialization is infallible"))
}

/// Drives per-entity JSON (de)serialization from a set of registered component
/// types. Only registered types are written; everything else is invisible to
/// persistence.
#[derive(Default)]
pub struct ComponentRegistry {
    sers: Vec<SerFn>,
    desers: HashMap<&'static str, DeserFn>,
    inserts: HashMap<&'static str, InsertFn>,
    removes: HashMap<&'static str, RemoveFn>,
    gets: HashMap<&'static str, GetFn>,
    relation_tags: HashSet<&'static str>,
}

impl ComponentRegistry {
    pub fn register<C: NamedComponent>(&mut self) {
        self.sers.push(ser_one::<C>);
        self.desers.insert(C::TAG, deser_one::<C>);
        self.inserts.insert(C::TAG, insert_one::<C>);
        self.removes.insert(C::TAG, remove_one::<C>);
        self.gets.insert(C::TAG, get_one::<C>);
    }

    /// Mark a tag as a relation forward-link. The live mutation paths refuse it,
    /// routing the caller to `Move`/`Relate`. Populated by `register_relation`.
    pub fn mark_relation_tag(&mut self, tag: &'static str) {
        self.relation_tags.insert(tag);
    }

    pub fn is_relation_tag(&self, tag: &str) -> bool {
        self.relation_tags.contains(tag)
    }

    /// Deserialize one component from `v` and overwrite it on a live entity.
    pub fn insert_component(
        &self,
        ecs: &mut hecs::World,
        e: hecs::Entity,
        tag: &str,
        v: Value,
    ) -> Result<(), RegistryError> {
        let f = self
            .inserts
            .get(tag)
            .ok_or_else(|| RegistryError::UnknownComponent(tag.to_string()))?;
        f(ecs, e, v)?;
        Ok(())
    }

    /// Remove one component by tag from a live entity.
    pub fn remove_component(
        &self,
        ecs: &mut hecs::World,
        e: hecs::Entity,
        tag: &str,
    ) -> Result<(), RegistryError> {
        let f = self
            .removes
            .get(tag)
            .ok_or_else(|| RegistryError::UnknownComponent(tag.to_string()))?;
        f(ecs, e);
        Ok(())
    }

    /// Serialize just one named component back to JSON; `None` if absent on the
    /// entity. Errors only when the tag is not registered.
    pub fn component_value(
        &self,
        er: hecs::EntityRef,
        tag: &str,
    ) -> Result<Option<Value>, RegistryError> {
        let f = self
            .gets
            .get(tag)
            .ok_or_else(|| RegistryError::UnknownComponent(tag.to_string()))?;
        Ok(f(er))
    }

    pub fn serialize_entity(&self, er: hecs::EntityRef) -> Value {
        let mut map = Map::new();
        for s in &self.sers {
            s(er, &mut map);
        }
        Value::Object(map)
    }

    pub fn deserialize_into(
        &self,
        data: &Value,
        b: &mut hecs::EntityBuilder,
    ) -> Result<(), RegistryError> {
        let obj = data.as_object().ok_or(RegistryError::NotObject)?;
        for (tag, v) in obj {
            let f = self
                .desers
                .get(tag.as_str())
                .ok_or_else(|| RegistryError::UnknownComponent(tag.clone()))?;
            f(b, v.clone())?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("entity data was not a JSON object")]
    NotObject,
    #[error("unknown component tag: {0}")]
    UnknownComponent(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
