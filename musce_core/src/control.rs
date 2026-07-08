//! Embodiment primitives: the control wiring and the control cursor, the two
//! relation instances a session resolves a driven actor through. See
//! `docs/architecture/networking-and-sessions.md` for how they back a session
//! and `docs/architecture/ecs-and-relations.md` for the relation layer they
//! reuse. Modeled on `containment.rs`: a `Relation` impl plus a small typed
//! `World` helper block.

use crate::id::EntityId;
use crate::relation::{Cascade, Relation, RelationError};
use crate::world::World;

/// The capability wiring: which controller an entity is plugged into and *may* be
/// driven by. Source = the controlled entity (it has one controller); target =
/// the controller (it has many sources). Acyclic chains (character -> mech ->
/// drone); a controller's death detaches its controlled entities, reverting each
/// to its own AI rather than destroying it.
pub struct Controls;

impl Relation for Controls {
    const ACYCLIC: bool = true;
    const ON_TARGET_DESPAWN: Cascade = Cascade::Detach;
    const TARGET_TAG: &'static str = "controlled_by";
}

/// The control cursor: which single entity in a controller's chain its input is
/// live on *right now*. Source = the controller; target = the focused entity. One
/// per controller (a source has one target), stored as the forward link on the
/// character and persisted, so a character piloting a robot resumes piloting it
/// after a reboot. Absence means "drive yourself".
///
/// `Focus` is a relation, not a lone component, so a focused entity's despawn
/// clears the focuser's cursor through the ordinary `Detach` cascade: the engine
/// tracks focus -> target directly and never infers the focuser from the control
/// wiring.
pub struct Focus;

impl Relation for Focus {
    const ACYCLIC: bool = true;
    const ON_TARGET_DESPAWN: Cascade = Cascade::Detach;
    const TARGET_TAG: &'static str = "focus";
}

impl World {
    /// The entity a controller is currently piloting, if any.
    pub fn focus_of(&self, controller: EntityId) -> Option<EntityId> {
        self.target_of::<Focus>(controller)
    }

    /// The root of an entity's control chain: the topmost controller, where
    /// `Focus` lives. For an unpuppeted entity that is the entity itself; for a
    /// puppet it is the controlling character at the top of the `Controls` chain.
    /// Walks `Controls` upward, so it is correct at any chain depth.
    ///
    /// This is the inverse of the session's actor resolution (`focus_of(root)`,
    /// which walks back down): together they map a connection's character to the
    /// entity it drives and back. See
    /// `docs/architecture/networking-and-sessions.md`.
    pub fn control_root(&self, entity: EntityId) -> EntityId {
        self.ancestors::<Controls>(entity)
            .last()
            .copied()
            .unwrap_or(entity)
    }

    /// Point a controller's cursor at `target`. `Focus` is, by definition, the
    /// cursor *within the control chain*, so this enforces that `target` is
    /// something the controller controls (transitively, via `Controls`); a cursor
    /// outside the chain is a structurally invalid state, not rejected play.
    /// Whether a controller may *establish* control over something in the first
    /// place stays game policy (the `pilot`/`@possess` gate); this governs only
    /// where an existing controller's cursor may land.
    pub fn set_focus(&mut self, controller: EntityId, target: EntityId) -> Result<(), FocusError> {
        if target == controller || !self.ancestors::<Controls>(target).contains(&controller) {
            return Err(FocusError::NotControlled);
        }
        self.relate::<Focus>(controller, target)
            .map_err(FocusError::Structural)
    }

    /// Drop a controller's cursor back to itself.
    pub fn clear_focus(&mut self, controller: EntityId) {
        self.unrelate::<Focus>(controller);
    }
}

/// Why a `set_focus` was refused.
#[derive(Debug, thiserror::Error)]
pub enum FocusError {
    /// `target` is not in the controller's `Controls` chain. `Focus` is the cursor
    /// within that chain, so it cannot point outside it (the controller itself
    /// included: absence of `Focus`, not a self-cursor, means "drive yourself").
    #[error("target is not in the controller's control chain")]
    NotControlled,
    /// A structural failure from the underlying `relate` (a cycle or a missing
    /// entity).
    #[error(transparent)]
    Structural(#[from] RelationError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Description;
    use hecs::EntityBuilder;

    // Controls/Focus are kind-agnostic, so a test "being" is just a described
    // entity; player/creature are game kinds and no longer live in core.
    fn being(w: &mut World, desc: &str) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Description(desc.into()));
        w.spawn(b)
    }

    #[test]
    fn focus_set_and_clear() {
        let mut w = World::new();
        let character = being(&mut w, "a pilot");
        let robot = being(&mut w, "a robot");
        w.relate::<Controls>(robot, character).unwrap();

        assert_eq!(w.focus_of(character), None);
        w.set_focus(character, robot).unwrap();
        assert_eq!(w.focus_of(character), Some(robot));
        w.clear_focus(character);
        assert_eq!(w.focus_of(character), None);
    }

    #[test]
    fn set_focus_rejects_an_uncontrolled_target() {
        let mut w = World::new();
        let character = being(&mut w, "a pilot");
        let stranger = being(&mut w, "a robot it does not control");

        // No `Controls` edge: the cursor cannot land outside the chain.
        assert!(matches!(
            w.set_focus(character, stranger),
            Err(FocusError::NotControlled)
        ));
        assert_eq!(w.focus_of(character), None);

        // Nor on the controller itself (that is "drive yourself", i.e. no Focus).
        assert!(matches!(
            w.set_focus(character, character),
            Err(FocusError::NotControlled)
        ));
    }

    #[test]
    fn control_root_walks_to_the_top() {
        let mut w = World::new();
        let character = being(&mut w, "a pilot");
        let mech = being(&mut w, "a mech");
        let drone = being(&mut w, "a drone");
        w.relate::<Controls>(mech, character).unwrap();
        w.relate::<Controls>(drone, mech).unwrap();

        // From any depth, the root is the topmost controller; an uncontrolled
        // entity is its own root.
        assert_eq!(w.control_root(drone), character);
        assert_eq!(w.control_root(mech), character);
        assert_eq!(w.control_root(character), character);
    }

    #[test]
    fn controls_and_focus_round_trip() {
        let mut w = World::new();
        let character = being(&mut w, "a pilot");
        let robot = being(&mut w, "a robot");
        w.relate::<Controls>(robot, character).unwrap();
        w.set_focus(character, robot).unwrap();

        let snap = w.snapshot();
        let mut w2 = World::new();
        w2.load(&snap.entities, snap.next_id).unwrap();

        // Still piloting: the control wiring and the cursor both survived, and
        // their reverse indexes were rebuilt on load.
        assert_eq!(w2.target_of::<Controls>(robot), Some(character));
        assert_eq!(w2.focus_of(character), Some(robot));
        assert_eq!(w2.sources_of::<Controls>(character), vec![robot]);
    }

    #[test]
    fn controller_death_detaches_controlled() {
        let mut w = World::new();
        let character = being(&mut w, "a pilot");
        let robot = being(&mut w, "a robot");
        w.relate::<Controls>(robot, character).unwrap();

        w.despawn(character);

        // The robot outlives its controller, now controlled by no one (Detach).
        assert!(w.entity(robot).is_some());
        assert_eq!(w.target_of::<Controls>(robot), None);
    }

    #[test]
    fn despawning_puppet_resets_focus() {
        let mut w = World::new();
        let character = being(&mut w, "a pilot");
        let robot = being(&mut w, "a robot");
        w.relate::<Controls>(robot, character).unwrap();
        w.set_focus(character, robot).unwrap();
        assert_eq!(w.focus_of(character), Some(robot));

        w.despawn(robot);

        // The Focus Detach cascade cleared the cursor; the controller is intact.
        assert!(w.entity(character).is_some());
        assert_eq!(w.focus_of(character), None);
    }
}
