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
pub use relation::{Cascade, RelSources, RelTarget, Relation, RelationError};
pub use snapshot::{EntityBlob, Snapshot};
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
    // `Description`: item/container are game kinds and no longer live here.
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
        assert_eq!(w.contents(hall), vec![sword]);
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
    fn moving_reparents() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let chest = container(&mut w, "chest");
        let gem = item(&mut w, "gem");
        w.move_entity(gem, hall).unwrap();
        w.move_entity(gem, chest).unwrap();

        assert_eq!(w.container_of(gem), Some(chest));
        assert_eq!(w.contents(hall), Vec::<EntityId>::new());
        assert_eq!(w.contents(chest), vec![gem]);
    }

    #[test]
    fn cycles_rejected() {
        let mut w = World::new();
        let a = container(&mut w, "a");
        let b = container(&mut w, "b");
        w.move_entity(b, a).unwrap();
        assert!(matches!(w.move_entity(a, b), Err(RelationError::Cycle)));
        assert!(matches!(w.move_entity(a, a), Err(RelationError::Cycle)));
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
        let mut contents = w.contents(hall);
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
    fn descendants_predicate_stops() {
        let mut w = World::new();
        let hall = locus(&mut w, "hall");
        let bag = container(&mut w, "bag");
        let coin = item(&mut w, "coin");
        w.move_entity(bag, hall).unwrap();
        w.move_entity(coin, bag).unwrap();

        // descend everywhere: see bag and coin
        let mut all = Vec::new();
        w.descendants::<Containment, _, _>(hall, |_| true, |e| all.push(e));
        all.sort();
        let mut expect = vec![bag, coin];
        expect.sort();
        assert_eq!(all, expect);

        // never descend: see only direct contents (bag), not coin
        let mut shallow = Vec::new();
        w.descendants::<Containment, _, _>(hall, |_| false, |e| shallow.push(e));
        assert_eq!(shallow, vec![bag]);
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
    #[cfg(debug_assertions)]
    #[should_panic(expected = "disagrees with its Id component")]
    fn load_rejects_mismatched_id() {
        let mut w = World::new();
        let _ = item(&mut w, "x");
        let mut snap = w.snapshot();
        snap.entities[0].id = EntityId(99_999); // disagree with the Id in data

        let mut w2 = World::new();
        let _ = w2.load(&snap.entities, snap.next_id);
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
        assert_eq!(w.contents(chest), Vec::<EntityId>::new());
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
        assert_eq!(w2.contents(bag), vec![coin]);
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
    // `Description` is a registered component, so it stands in for a tracked game
    // component here (the engine ships no trackable game vocabulary of its own).

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
        w.insert(it, Description("z".into()));
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
        w.insert(it, Description("z".into()));
        w.remove::<Description>(it);
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
        assert!(w.modify::<Description>(it, |d| d.0 = "new".into()));
        assert_eq!(
            w.component_value(it, "description"),
            Some(serde_json::json!("new"))
        );

        // Absent component: no mutation, report false, emit nothing.
        assert!(!w.modify::<Description>(bare, |d| d.0 = "unreached".into()));

        assert_eq!(changed_tags(&w.take_facts()), ["description"]);
    }

    #[test]
    fn raw_ecs_mutation_bypasses_the_trigger() {
        // The footgun `modify` and `forbid_tracking` exist for: an in-place write
        // through the raw ecs handle mutates below the mutator layer and emits
        // nothing, so a tracked index over it would silently desync.
        let mut w = World::new();
        w.track_component::<Description>();
        let it = item(&mut w, "x");
        let _ = w.take_facts();

        {
            // The in-crate raw path (the `ecs` field) is the only remaining way to
            // reach a `&mut` component borrow; the public API exposes none.
            let e = w.index().get(it).unwrap();
            let mut d = w.ecs.get::<&mut Description>(e).unwrap(); // hygiene:allow-raw-mut
            d.0 = "silently changed".into();
        }
        assert!(
            w.take_facts().is_empty(),
            "a raw &mut write cannot signal the change"
        );
    }

    #[test]
    #[should_panic(expected = "cannot be tracked")]
    fn tracking_a_forbidden_component_panics() {
        let mut w = World::new();
        w.forbid_tracking::<Description>();
        w.track_component::<Description>();
    }

    #[test]
    #[should_panic(expected = "already tracked")]
    fn forbidding_a_tracked_component_panics() {
        let mut w = World::new();
        w.track_component::<Description>();
        w.forbid_tracking::<Description>();
    }
}
