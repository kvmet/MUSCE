//! Core of the MUSCE MUD engine: the in-memory ECS world, the generic relation
//! layer, and the persistence-facing snapshot model. Pure: no networking, no DB.

pub mod component;
pub mod containment;
pub mod control;
pub mod fact;
pub mod id;
pub mod relation;
pub mod snapshot;
pub mod world;

// Re-export hecs so dependents can build entities without depending on it directly.
pub use hecs;
// Re-export serde_json's JSON types so the action layer names them without its own
// serde_json dependency.
pub use serde_json::{Map, Value};

pub use component::{ComponentBlob, Description, Id, Locus, Name, NamedComponent, RegistryError};
pub use containment::Containment;
pub use control::{Controls, Focus, FocusError};
pub use fact::{DestroyCause, Fact};
pub use id::{EntityId, EntityIndex};
pub use relation::{
    AcyclicRelation, Cascade, RelTarget, Relation, RelationError, RelationRole, Walk,
};
pub use snapshot::{EntityBlob, LoadError, Snapshot};
pub use world::{MutateError, World};

#[cfg(test)]
mod tests {
    use super::*;
    use hecs::EntityBuilder;

    fn locus(w: &mut World, name: &str) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Locus);
        b.add(Description(name.into()));
        w.spawn(b)
    }

    // The core tests exercise the engine machinery (containment, snapshot,
    // mutation), which is kind-agnostic, so these stand-in "things" carry only a
    // `Description`: item/container are app kinds and no longer live here.
    fn item(w: &mut World, name: &str) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Description(name.into()));
        w.spawn(b)
    }

    fn container(w: &mut World, name: &str) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Description(name.into()));
        w.spawn(b)
    }

    #[test]
    fn containment_basic() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let sword = item(&mut w, "sword");
        w.move_entity(sword, hall).unwrap();

        assert_eq!(w.container_of(sword), Some(hall));
        assert_eq!(w.contents(hall), &[sword]);
    }

    #[test]
    fn enclosing_locus_walks_up() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let bag = container(&mut w, "bag");
        let coin = item(&mut w, "coin");
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();

        assert_eq!(w.container_of(coin), Some(bag));
        assert_eq!(w.enclosing_locus(coin), Some(hall));
    }

    #[test]
    fn ancestors_stream_immediate_first_and_can_stop_early_or_collect() {
        let mut w = World::new();
        let root = container(&mut w, "root");
        let middle = container(&mut w, "middle");
        let leaf = item(&mut w, "leaf");
        let detached = item(&mut w, "detached");
        w.move_entity(middle, root).unwrap();
        w.move_entity(leaf, middle).unwrap();

        assert_eq!(
            w.ancestors::<Containment>(leaf).collect::<Vec<_>>(),
            [middle, root]
        );
        assert_eq!(w.ancestors::<Containment>(leaf).next(), Some(middle));
        assert_eq!(w.ancestors::<Containment>(detached).next(), None);
    }

    #[test]
    fn moving_reparents() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let chest = container(&mut w, "chest");
        let gem = item(&mut w, "gem");
        w.move_entity(gem, hall).unwrap();
        w.move_entity(gem, chest).unwrap();

        assert_eq!(w.container_of(gem), Some(chest));
        assert!(w.contents(hall).is_empty());
        assert_eq!(w.contents(chest), &[gem]);
    }

    #[test]
    fn descendant_walk_descends_prunes_and_stops() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let bag = container(&mut w, "bag");
        let coin = item(&mut w, "coin");
        let pebble = item(&mut w, "pebble");
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();
        w.move_entity(pebble, hall).unwrap();

        let mut all = Vec::new();
        w.walk_descendants::<Containment>(hall, |entity| {
            all.push(entity);
            Walk::Descend
        });
        all.sort();
        let mut expected = vec![bag, coin, pebble];
        expected.sort();
        assert_eq!(all, expected);

        let mut pruned = Vec::new();
        w.walk_descendants::<Containment>(hall, |entity| {
            pruned.push(entity);
            if entity == bag {
                Walk::Prune
            } else {
                Walk::Descend
            }
        });
        pruned.sort();
        let mut expected = vec![bag, pebble];
        expected.sort();
        assert_eq!(pruned, expected);

        let mut visits = 0;
        w.walk_descendants::<Containment>(hall, |_| {
            visits += 1;
            Walk::Stop
        });
        assert_eq!(visits, 1);
    }

    #[test]
    fn cycles_rejected() {
        let mut w = World::new();
        let a = container(&mut w, "a");
        let b = container(&mut w, "b");
        w.move_entity(b, a).unwrap();
        assert!(matches!(
            w.move_entity(a, b),
            Err(RelationError::Cycle { .. })
        ));
        assert!(matches!(
            w.move_entity(a, a),
            Err(RelationError::Cycle { .. })
        ));
    }

    #[test]
    fn relation_errors_retain_kind_endpoint_role_and_cycle_edge() {
        let mut w = World::new();
        let live = container(&mut w, "live");
        let missing = EntityId(99_999);

        let source_error = w.move_entity(missing, live).unwrap_err();
        assert!(matches!(
            &source_error,
            RelationError::NoSuchEntity {
                kind,
                role: RelationRole::Source,
                entity,
            } if kind == Containment::TARGET_TAG && *entity == missing
        ));
        assert!(source_error.to_string().contains("source"));

        let target_error = w.move_entity(live, missing).unwrap_err();
        assert!(matches!(
            &target_error,
            RelationError::NoSuchEntity {
                kind,
                role: RelationRole::Target,
                entity,
            } if kind == Containment::TARGET_TAG && *entity == missing
        ));
        assert!(target_error.to_string().contains("target"));

        let cycle_error = w.move_entity(live, live).unwrap_err();
        assert!(matches!(
            &cycle_error,
            RelationError::Cycle {
                kind,
                source,
                target,
            } if kind == Containment::TARGET_TAG && *source == live && *target == live
        ));
        let display = cycle_error.to_string();
        assert!(display.contains(Containment::TARGET_TAG));
        assert!(display.contains(&format!("{live:?}")));
    }

    #[test]
    fn despawn_reparents_contents() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let bag = container(&mut w, "bag");
        let coin = item(&mut w, "coin");
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();

        w.despawn(bag);

        // bag's contents spill up to the hall; bag is gone.
        assert_eq!(w.container_of(coin), Some(hall));
        assert_eq!(w.enclosing_locus(coin), Some(hall));
        assert!(!w.contains(bag));
        let mut contents = w.contents(hall).to_vec();
        contents.sort();
        assert_eq!(contents, vec![coin]);
    }

    #[test]
    fn despawn_located_named_entity_captures_locus_and_name() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let coin = item(&mut w, "a gold coin");
        w.move_entity(coin, hall).unwrap();
        let _ = w.take_facts(); // discard the setup move's facts

        w.despawn(coin);
        let facts = w.take_facts();

        assert_eq!(facts.len(), 1);
        let Fact::Destroyed {
            entity,
            last_locus,
            name,
            cause,
        } = &facts[0]
        else {
            panic!("expected Destroyed, got {:?}", facts[0]);
        };
        assert_eq!(*entity, coin);
        assert_eq!(*last_locus, Some(hall));
        assert_eq!(name.as_deref(), Some("a gold coin"));
        assert_eq!(*cause, DestroyCause::Direct);
    }

    #[test]
    fn despawn_unnamed_entity_has_no_name() {
        use hecs::EntityBuilder;
        let mut w = World::new();
        // An entity with neither a `Name` nor a `Description` has nothing to name
        // it, so its fact carries no name.
        let bare = w.spawn(EntityBuilder::new());

        w.despawn(bare);
        let facts = w.take_facts();

        assert_eq!(facts.len(), 1);
        let Fact::Destroyed { name, cause, .. } = &facts[0] else {
            panic!("expected Destroyed, got {:?}", facts[0]);
        };
        assert!(name.is_none(), "no Name or Description means no name");
        assert_eq!(*cause, DestroyCause::Direct);
    }

    #[test]
    fn move_within_a_locus_emits_moved_but_not_locus_changed() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let bag = container(&mut w, "bag");
        let coin = item(&mut w, "coin");
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, hall).unwrap(); // the coin starts in the hall
        let _ = w.take_facts(); // discard setup moves

        // Reparent into the bag: still enclosed by the hall, so Moved only, no
        // LocusChanged.
        w.move_entity(coin, bag).unwrap();
        let facts = w.take_facts();
        assert_eq!(
            facts.len(),
            1,
            "same-locus reparent is Moved only: {facts:?}"
        );
        assert!(matches!(
            facts[0],
            Fact::Moved { entity, from: Some(f), to: Some(t) }
                if entity == coin && f == hall && t == bag
        ));
    }

    #[test]
    fn move_across_loci_emits_moved_and_locus_changed() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let garden = locus(&mut w, "garden");
        let mover = item(&mut w, "a wanderer");
        w.move_entity(mover, hall).unwrap();
        let _ = w.take_facts();

        w.move_entity(mover, garden).unwrap();
        let facts = w.take_facts();
        assert_eq!(facts.len(), 2, "Moved + LocusChanged: {facts:?}");
        assert!(matches!(
            facts[0],
            Fact::Moved { entity, from: Some(f), to: Some(t) }
                if entity == mover && f == hall && t == garden
        ));
        assert!(matches!(
            facts[1],
            Fact::LocusChanged { entity, from: Some(f), to: Some(t) }
                if entity == mover && f == hall && t == garden
        ));
    }

    #[test]
    fn a_carried_subtree_emits_no_movement_facts_of_its_own() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let garden = locus(&mut w, "garden");
        let character = container(&mut w, "a character");
        let coin = item(&mut w, "a coin");
        w.move_entity(character, hall).unwrap();
        w.move_entity(coin, character).unwrap(); // the coin is carried
        let _ = w.take_facts();

        // The character walks to the garden. Only its own containment link changed.
        w.move_entity(character, garden).unwrap();
        let facts = w.take_facts();

        // Exactly the character's two facts; nothing for the coin, whose link never
        // changed even though its enclosing locus did.
        assert_eq!(facts.len(), 2, "only the character's facts: {facts:?}");
        assert!(facts.iter().all(|f| match f {
            Fact::Moved { entity, .. } | Fact::LocusChanged { entity, .. } => *entity == character,
            _ => false,
        }));

        // The coin's locus really did change, and is *derivable*: it encloses to the
        // garden now, exactly where the character went. That derivability is why the
        // engine does not emit a fact for it.
        assert_eq!(w.enclosing_locus(coin), Some(garden));
    }

    #[test]
    fn reparent_cascade_emits_movement_for_surviving_children() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let bag = container(&mut w, "bag");
        let coin = item(&mut w, "coin");
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();
        let _ = w.take_facts();

        w.despawn(bag); // the coin reparents up to the hall: its own link changes
        let facts = w.take_facts();

        assert!(
            facts.iter().any(|f| matches!(
                f,
                Fact::Moved { entity, from: Some(f), to: Some(t) }
                    if *entity == coin && *f == bag && *t == hall
            )),
            "coin moved bag->hall: {facts:?}"
        );
        assert!(
            facts
                .iter()
                .any(|f| matches!(f, Fact::Destroyed { entity, .. } if *entity == bag))
        );
        // The coin stayed enclosed by the hall throughout, so no LocusChanged.
        assert!(!facts.iter().any(|f| matches!(f, Fact::LocusChanged { .. })));
    }

    #[test]
    fn deletes_persist_until_confirmed() {
        let mut w = World::new();
        let a = item(&mut w, "a");
        w.despawn(a);

        // Snapshot copies the delete but does not drop it.
        let s1 = w.snapshot();
        assert_eq!(s1.deletes, vec![a]);

        // A second snapshot (e.g. after the first save failed) still has it.
        assert_eq!(w.snapshot().deletes, vec![a]);

        // Only an explicit confirm clears it.
        w.confirm_saved(&s1.deletes);
        assert!(w.snapshot().deletes.is_empty());
    }

    #[test]
    fn snapshot_serializes_only_the_dirty_delta() {
        let mut w = World::new();
        let a = item(&mut w, "a");
        let _b = item(&mut w, "b");

        // Both freshly spawned, so the first snapshot is the whole set.
        assert_eq!(w.snapshot().entities.len(), 2);

        // The dirty set drained: an unchanged world snapshots to nothing.
        assert!(
            w.snapshot().entities.is_empty(),
            "a delta snapshot writes only what changed since the last one"
        );

        // Mutating one re-includes exactly that one.
        w.set_component(a, "description", serde_json::json!("changed"))
            .unwrap();
        let ids: Vec<_> = w.snapshot().entities.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![a]);
    }

    #[test]
    fn a_loaded_world_starts_clean_and_mark_all_dirty_reincludes_it() {
        let mut w = World::new();
        let _ = item(&mut w, "x");
        let snap = w.snapshot();

        let mut w2 = World::new();
        w2.load(&snap.entities, snap.next_id).unwrap();
        // A loaded world already matches the store, so it has no delta to write;
        // else every boot would rewrite the whole world.
        assert!(w2.snapshot().entities.is_empty());

        // The migration path re-includes everything.
        w2.mark_all_dirty();
        assert_eq!(w2.snapshot().entities.len(), 1);
    }

    #[test]
    fn remark_dirty_restores_a_failed_delta_but_never_a_dead_id() {
        let mut w = World::new();
        let a = item(&mut w, "a");
        let snap = w.snapshot(); // drains {a}
        assert!(w.snapshot().entities.is_empty());

        // The save failed: the host hands the delta's ids back, and the next
        // snapshot re-serializes them.
        let ids: Vec<_> = snap.entities.iter().map(|e| e.id).collect();
        w.remark_dirty(&ids);
        let retry: Vec<_> = w.snapshot().entities.iter().map(|e| e.id).collect();
        assert_eq!(retry, vec![a]);

        // A stale delta naming a since-despawned id must not resurrect it into the
        // live set; it rides `deletes` instead.
        w.despawn(a);
        let _ = w.snapshot();
        w.remark_dirty(&[a]);
        assert!(w.snapshot().entities.is_empty());
    }

    #[test]
    fn load_rejects_mismatched_and_missing_ids_with_context() {
        let mut w = World::new();
        let _ = item(&mut w, "x");
        let mut snap = w.snapshot();
        snap.entities[0].id = EntityId(99_999); // disagree with the Id in data

        let mut w2 = World::new();
        assert!(matches!(
            w2.load(&snap.entities, snap.next_id),
            Err(LoadError::IdMismatch {
                blob_id: EntityId(99_999),
                component_id: Some(_),
            })
        ));

        snap.entities[0]
            .data
            .as_object_mut()
            .unwrap()
            .remove(Id::TAG);
        assert!(matches!(
            w2.load(&snap.entities, snap.next_id),
            Err(LoadError::IdMismatch {
                blob_id: EntityId(99_999),
                component_id: None,
            })
        ));
    }

    #[test]
    fn load_rejects_duplicates_before_spawning() {
        let mut source = World::new();
        let _ = item(&mut source, "x");
        let mut snap = source.snapshot();
        snap.entities.push(snap.entities[0].clone());

        let mut loaded = World::new();
        let id = snap.entities[0].id;
        assert!(matches!(
            loaded.load(&snap.entities, snap.next_id),
            Err(LoadError::DuplicateEntity(duplicate)) if duplicate == id
        ));
        assert!(loaded.index().is_empty());
    }

    #[test]
    fn load_clamps_a_stale_next_id_above_every_loaded_identity() {
        let mut source = World::new();
        let loaded_id = item(&mut source, "loaded");
        let snapshot = source.snapshot();

        let mut loaded = World::new();
        loaded.load(&snapshot.entities, loaded_id.0).unwrap();
        let spawned = item(&mut loaded, "new");

        assert!(spawned > loaded_id);
        assert_eq!(loaded.get::<Id>(loaded_id).unwrap().0, loaded_id);
        assert_eq!(loaded.get::<Id>(spawned).unwrap().0, spawned);
        assert_eq!(loaded.index().len(), 2);
    }

    #[test]
    fn load_distinguishes_non_object_and_malformed_id_data_and_remains_retryable() {
        let mut source = World::new();
        let id = item(&mut source, "valid");
        let snapshot = source.snapshot();
        let mut loaded = World::new();

        let non_object = EntityBlob {
            id,
            zone: None,
            data: serde_json::json!([id.0]),
        };
        assert!(matches!(
            loaded.load(&[non_object], snapshot.next_id),
            Err(LoadError::NonObjectBlob { blob_id }) if blob_id == id
        ));
        assert!(loaded.index().is_empty());

        let mut malformed = snapshot.entities[0].clone();
        malformed
            .data
            .as_object_mut()
            .unwrap()
            .insert(Id::TAG.into(), serde_json::json!("not an entity id"));
        assert!(matches!(
            loaded.load(&[malformed], snapshot.next_id),
            Err(LoadError::InvalidIdComponent { blob_id, .. }) if blob_id == id
        ));
        assert!(loaded.index().is_empty());

        loaded.load(&snapshot.entities, snapshot.next_id).unwrap();
        assert!(loaded.contains(id));
    }

    #[test]
    fn load_rejects_an_identity_with_no_allocatable_successor() {
        let exhausted = EntityId(u64::MAX);
        let blob = EntityBlob {
            id: exhausted,
            zone: None,
            data: serde_json::json!({"id": exhausted}),
        };
        let mut loaded = World::new();

        assert!(matches!(
            loaded.load(&[blob], u64::MAX),
            Err(LoadError::IdSpaceExhausted { highest_id }) if highest_id == exhausted
        ));
        assert!(loaded.index().is_empty());
    }

    #[test]
    fn failed_relation_load_is_clean_retryable_and_preserves_resources() {
        let mut source = World::new();
        let child = item(&mut source, "child");
        let target = item(&mut source, "target");
        source.move_entity(child, target).unwrap();
        let valid = source.snapshot();
        let mut invalid = valid.entities.clone();
        invalid.retain(|blob| blob.id != target);

        let mut loaded = World::new();
        loaded.insert_resource(String::from("startup wiring"));
        assert!(matches!(
            loaded.load(&invalid, valid.next_id),
            Err(LoadError::DanglingRelation {
                kind,
                source,
                target: missing,
            }) if kind == Containment::TARGET_TAG && source == child && missing == target
        ));
        assert!(loaded.index().is_empty());
        assert_eq!(
            loaded.resource::<String>().map(String::as_str),
            Some("startup wiring")
        );

        loaded.load(&valid.entities, valid.next_id).unwrap();
        assert_eq!(loaded.container_of(child), Some(target));
    }

    #[test]
    fn load_rejects_acyclic_cycles_but_accepts_declared_cyclic_relations() {
        let mut source = World::new();
        let a = item(&mut source, "a");
        let b = item(&mut source, "b");
        let mut blobs = source.snapshot().entities;
        for blob in &mut blobs {
            let target = if blob.id == a { b } else { a };
            blob.data
                .as_object_mut()
                .unwrap()
                .insert(Containment::TARGET_TAG.into(), serde_json::json!(target));
        }
        let mut loaded = World::new();
        assert!(matches!(
            loaded.load(&blobs, 3),
            Err(LoadError::RelationCycle { kind, cycle })
                if kind == Containment::TARGET_TAG && cycle.len() == 2
        ));
        assert!(loaded.index().is_empty());

        struct Peer;
        impl Relation for Peer {
            const ACYCLIC: bool = false;
            const ON_TARGET_DESPAWN: Cascade = Cascade::Detach;
            const TARGET_TAG: &'static str = "peer";
        }
        let mut cyclic_source = World::new();
        cyclic_source.register_relation::<Peer>();
        let x = item(&mut cyclic_source, "x");
        let y = item(&mut cyclic_source, "y");
        cyclic_source.relate::<Peer>(x, y).unwrap();
        cyclic_source.relate::<Peer>(y, x).unwrap();
        let snap = cyclic_source.snapshot();
        let mut cyclic_loaded = World::new();
        cyclic_loaded.register_relation::<Peer>();
        cyclic_loaded.load(&snap.entities, snap.next_id).unwrap();
        assert_eq!(cyclic_loaded.target_of::<Peer>(x), Some(y));
        assert_eq!(cyclic_loaded.target_of::<Peer>(y), Some(x));
    }

    #[test]
    fn load_requires_a_fresh_world() {
        let mut loaded = World::new();
        let prior = item(&mut loaded, "already here");
        loaded.despawn(prior);
        assert!(matches!(loaded.load(&[], 1), Err(LoadError::WorldNotFresh)));
    }

    // --- type-erased component mutation ----------------------------------

    #[test]
    fn create_from_blob_spawns_with_components_and_id() {
        let mut w = World::new();
        let before = w.index().len();
        let id = w
            .create(&serde_json::json!({
                "locus": null,
                "description": "a brass lamp",
            }))
            .unwrap();

        // The components landed, a fresh Id was assigned, and the index grew.
        assert!(w.has::<Locus>(id));
        assert_eq!(w.index().len(), before + 1);
        assert_eq!(w.get::<Description>(id).unwrap().0, "a brass lamp");
        assert_eq!(w.get::<Id>(id).unwrap().0, id);
        // Location-less: create never places it.
        assert_eq!(w.container_of(id), None);
    }

    #[test]
    fn create_rejects_relation_tag_and_spawns_nothing() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let before = w.index().len();

        let err = w.create(&serde_json::json!({
            "item": null,
            "contained_by": hall.0,
        }));
        assert!(matches!(err, Err(MutateError::RelationTag(_))));
        assert_eq!(w.index().len(), before, "nothing should have spawned");
    }

    #[test]
    fn set_component_round_trips() {
        let mut w = World::new();
        let it = item(&mut w, "plain");
        w.set_component(it, "description", serde_json::json!("a shiny coin"))
            .unwrap();
        assert_eq!(
            w.component_value(it, "description"),
            Some(serde_json::json!("a shiny coin"))
        );
    }

    #[test]
    fn set_component_unknown_tag_errors() {
        let mut w = World::new();
        let it = item(&mut w, "x");
        let err = w.set_component(it, "nonesuch", serde_json::json!(1));
        assert!(matches!(
            err,
            Err(MutateError::Registry(RegistryError::UnknownComponent(_)))
        ));
    }

    #[test]
    fn set_component_relation_tag_refused_and_containment_unchanged() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let chest = container(&mut w, "chest");
        let coin = item(&mut w, "coin");
        w.move_entity(coin, hall).unwrap();

        // Trying to retarget containment via the generic setter must fail and
        // leave the existing containment intact.
        let err = w.set_component(coin, "contained_by", serde_json::json!(chest.0));
        assert!(matches!(err, Err(MutateError::RelationTag(_))));
        assert_eq!(w.container_of(coin), Some(hall));
        assert!(w.contents(chest).is_empty());
    }

    #[test]
    fn set_component_identity_tag_refused() {
        let mut w = World::new();
        let it = item(&mut w, "x");
        let err = w.set_component(it, "id", serde_json::json!(42));
        assert!(matches!(err, Err(MutateError::IdentityTag(_))));
    }

    #[test]
    fn remove_component_removes_present() {
        let mut w = World::new();
        let it = item(&mut w, "x");
        assert!(w.component_value(it, "description").is_some());
        w.remove_component(it, "description").unwrap();
        assert_eq!(w.component_value(it, "description"), None);
    }

    #[test]
    fn remove_component_refuses_id_and_relation_tags() {
        let mut w = World::new();
        let it = item(&mut w, "x");
        assert!(matches!(
            w.remove_component(it, "id"),
            Err(MutateError::IdentityTag(_))
        ));
        assert!(matches!(
            w.remove_component(it, "contained_by"),
            Err(MutateError::RelationTag(_))
        ));
    }

    #[test]
    fn component_value_absent_is_none() {
        let mut w = World::new();
        let bare = w.spawn(EntityBuilder::new());
        assert_eq!(w.component_value(bare, "description"), None);
    }

    #[test]
    fn mutate_missing_entity_errors() {
        let mut w = World::new();
        let ghost = EntityId(9999);
        assert!(matches!(
            w.set_component(ghost, "description", serde_json::json!("x")),
            Err(MutateError::NoSuchEntity(_))
        ));
    }

    #[test]
    fn snapshot_roundtrip() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let bag = container(&mut w, "bag");
        let coin = item(&mut w, "coin");
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();

        let snap = w.snapshot();

        let mut w2 = World::new();
        w2.load(&snap.entities, snap.next_id).unwrap();

        // structure survives, reverse lists rebuilt
        assert_eq!(w2.container_of(coin), Some(bag));
        assert_eq!(w2.container_of(bag), Some(hall));
        assert_eq!(w2.enclosing_locus(coin), Some(hall));
        assert_eq!(w2.contents(bag), &[coin]);
        // A marker and a newtype both round-trip through the snapshot.
        assert!(w2.has::<Locus>(hall));
        assert_eq!(w2.get::<Description>(coin).unwrap().0, "coin");
        assert_eq!(w2.next_id(), snap.next_id);
    }

    #[test]
    fn resources_are_transient_and_snapshot_excluded() {
        #[derive(PartialEq, Debug)]
        struct Counter(u32);

        let mut w = World::new();
        assert!(w.resource::<Counter>().is_none());
        assert!(w.insert_resource(Counter(1)).is_none()); // no prior value
        assert_eq!(w.resource::<Counter>(), Some(&Counter(1)));
        assert_eq!(w.insert_resource(Counter(2)), Some(Counter(1))); // hands back the prior

        // A resource never reaches the snapshot: a reloaded world starts without it,
        // while the entity table still round-trips.
        let coin = item(&mut w, "coin");
        let snap = w.snapshot();
        let mut w2 = World::new();
        w2.load(&snap.entities, snap.next_id).unwrap();
        assert!(w2.resource::<Counter>().is_none());
        assert!(w2.contains(coin));

        // take_resource hands the value out and clears it.
        assert_eq!(w.take_resource::<Counter>(), Some(Counter(2)));
        assert!(w.resource::<Counter>().is_none());
    }

    // --- ComponentChanged triggers ---------------------------------------
    //
    // `Description` is a registered component, so it stands in for a tracked app
    // component here (the engine ships no trackable app vocabulary of its own).

    fn changed_tags(facts: &[Fact]) -> Vec<&'static str> {
        facts
            .iter()
            .filter_map(|f| match f {
                Fact::ComponentChanged { tag, .. } => Some(*tag),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn untracked_component_emits_no_fact() {
        let mut w = World::new();
        let it = item(&mut w, "x");
        let _ = w.take_facts();

        // Nothing tracked: a set is silent.
        w.set_component(it, "description", serde_json::json!("y"))
            .unwrap();
        w.insert(it, Description("z".into())).unwrap();
        assert!(w.take_facts().is_empty(), "untracked writes emit nothing");
    }

    #[test]
    fn tracked_set_insert_remove_emit_component_changed() {
        let mut w = World::new();
        w.track_component::<Description>();
        let it = item(&mut w, "x");
        let _ = w.take_facts();

        w.set_component(it, "description", serde_json::json!("y"))
            .unwrap();
        w.insert(it, Description("z".into())).unwrap();
        w.remove::<Description>(it).unwrap();
        let facts = w.take_facts();

        assert_eq!(changed_tags(&facts), ["description"; 3]);
        assert!(facts.iter().all(|f| matches!(
            f,
            Fact::ComponentChanged { entity, .. } if *entity == it
        )));
    }

    #[test]
    fn tracked_create_emits_only_for_tracked_tags() {
        let mut w = World::new();
        w.track_component::<Description>();

        // Blob carries a tracked tag (description) and an untracked one (locus).
        let id = w
            .create(&serde_json::json!({ "locus": null, "description": "a lamp" }))
            .unwrap();
        let facts = w.take_facts();

        assert_eq!(changed_tags(&facts), ["description"]);
        assert!(matches!(
            facts[0],
            Fact::ComponentChanged { entity, .. } if entity == id
        ));
    }

    #[test]
    fn modify_emits_when_present_and_is_silent_when_absent() {
        let mut w = World::new();
        w.track_component::<Description>();
        let it = item(&mut w, "old");
        let bare = w.spawn(EntityBuilder::new());
        let _ = w.take_facts();

        // Present: mutate in place, report true, emit one trigger.
        assert!(w.modify::<Description>(it, |d| d.0 = "new".into()).unwrap());
        assert_eq!(
            w.component_value(it, "description"),
            Some(serde_json::json!("new"))
        );

        // Absent component: no mutation, report false, emit nothing.
        assert!(
            !w.modify::<Description>(bare, |d| d.0 = "unreached".into())
                .unwrap()
        );

        assert_eq!(changed_tags(&w.take_facts()), ["description"]);
    }

    #[test]
    fn typed_mutators_reject_identity_and_registered_relation_components() {
        struct TestLink;
        impl Relation for TestLink {
            const ACYCLIC: bool = false;
            const ON_TARGET_DESPAWN: Cascade = Cascade::Detach;
            const TARGET_TAG: &'static str = "test_link";
        }

        let mut w = World::new();
        w.register_relation::<TestLink>();
        let source = item(&mut w, "source");
        let target = item(&mut w, "target");
        let other = item(&mut w, "other");
        w.relate::<TestLink>(source, target).unwrap();

        assert!(matches!(
            w.insert(source, Id(other)),
            Err(MutateError::IdentityTag(tag)) if tag == Id::TAG
        ));
        assert!(matches!(
            w.remove::<Id>(source),
            Err(MutateError::IdentityTag(tag)) if tag == Id::TAG
        ));
        assert!(matches!(
            w.modify::<Id>(source, |id| id.0 = other),
            Err(MutateError::IdentityTag(tag)) if tag == Id::TAG
        ));
        assert_eq!(w.get::<Id>(source).unwrap().0, source);

        assert!(matches!(
            w.insert(source, RelTarget::<TestLink>::new(other)),
            Err(MutateError::RelationTag(tag)) if tag == TestLink::TARGET_TAG
        ));
        assert!(matches!(
            w.remove::<RelTarget<TestLink>>(source),
            Err(MutateError::RelationTag(tag)) if tag == TestLink::TARGET_TAG
        ));
        assert!(matches!(
            w.modify::<RelTarget<TestLink>>(source, |link| link.0 = other),
            Err(MutateError::RelationTag(tag)) if tag == TestLink::TARGET_TAG
        ));
        assert_eq!(w.target_of::<TestLink>(source), Some(target));
        assert_eq!(w.sources_of::<TestLink>(target), &[source]);
        assert!(w.sources_of::<TestLink>(other).is_empty());
    }

    #[test]
    fn typed_mutator_absence_contract_is_precise() {
        let mut w = World::new();
        let entity = w.spawn(EntityBuilder::new());
        let missing = EntityId(u64::MAX);
        let _ = w.snapshot();
        let _ = w.take_facts();

        assert!(!w.remove::<Description>(entity).unwrap());
        assert!(!w.modify::<Description>(entity, |_| {}).unwrap());
        assert!(matches!(
            w.insert(missing, Description("x".into())),
            Err(MutateError::NoSuchEntity(id)) if id == missing
        ));
        assert!(matches!(
            w.remove::<Description>(missing),
            Err(MutateError::NoSuchEntity(id)) if id == missing
        ));
        assert!(matches!(
            w.modify::<Description>(missing, |_| {}),
            Err(MutateError::NoSuchEntity(id)) if id == missing
        ));
        assert!(w.take_facts().is_empty());
        assert!(w.snapshot().entities.is_empty());
    }
}
