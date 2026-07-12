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

/// The short prose name a player types or sees to refer to an entity (an exit's
/// direction like "north", an item's noun phrase like "a brass key"). The primary
/// in-character handle: the match key the name resolver keys off and the token
/// narration displays. General, not exit-specific. Distinct from Description,
/// which is the longer prose a `look`/`examine` reveals.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Name(pub String);
impl NamedComponent for Name {
    const TAG: &'static str = "name";
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

// The one kind the engine itself reasons about: `Locus`, a scope boundary in the
// containment tree. The engine finds an entity's nearest enclosing `Locus`
// (`enclosing_locus`) and snapshots it at destruction (the `Fact` channel's
// `last_locus`); it assigns the boundary no further meaning. A game decides what a
// Locus *is*: the reference game tags its rooms with it, so co-located entities in
// a room share a perception scope, but a non-MUD application could make its loci
// anything. Everything the engine only stores and never interprets (item,
// creature, container, a player avatar, an exit, and the connectivity between
// loci) is game vocabulary and lives in the game, registered through
// `Game.register`; see docs/architecture/engine-and-game.md. Permissions are not a
// marker on the actor: authorization is account-scoped (see
// docs/architecture/authorization.md).
marker!(Locus, "locus");

// --- typed blob builder --------------------------------------------------

/// Builds a tag->value component blob from typed components, so statically-known
/// content names Rust types and the tags fall out of `NamedComponent::TAG`. The
/// finished `Value` is the same tag-keyed object `Action::Create`/`World::create`
/// already consume; only its construction is typed. A typo is now a type error
/// rather than a runtime one. The raw tag->value path stays for genuinely runtime
/// input (e.g. `@set` from a user).
#[derive(Default)]
pub struct ComponentBlob {
    map: Map<String, Value>,
}

impl ComponentBlob {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one typed component; its tag is `C::TAG`, its value is `c` serialized.
    pub fn with<C: NamedComponent>(mut self, c: C) -> Self {
        self.map.insert(
            C::TAG.to_string(),
            serde_json::to_value(&c).expect("component serialization is infallible"),
        );
        self
    }

    /// The finished tag->value object, ready for `Action::Create { components }`.
    pub fn build(self) -> Value {
        Value::Object(self.map)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_blob_keys_by_tag_with_marker_null_and_newtype_inner() {
        let blob = ComponentBlob::new()
            .with(Locus)
            .with(Description("x".into()))
            .build();
        assert_eq!(
            blob,
            serde_json::json!({ "locus": null, "description": "x" })
        );
    }
}
