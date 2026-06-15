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

    /// Point a controller's cursor at `target`. Pure mechanism: it sets the cursor
    /// and enforces no policy (whether the controller may pilot `target` is the
    /// game's rule). Fails only on the structural grounds `relate` checks.
    pub fn set_focus(
        &mut self,
        controller: EntityId,
        target: EntityId,
    ) -> Result<(), RelationError> {
        self.relate::<Focus>(controller, target)
    }

    /// Drop a controller's cursor back to itself.
    pub fn clear_focus(&mut self, controller: EntityId) {
        self.unrelate::<Focus>(controller);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{Creature, Description, Player};
    use hecs::EntityBuilder;

    fn being<M: hecs::Component>(w: &mut World, marker: M, desc: &str) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(marker);
        b.add(Description(desc.into()));
        w.spawn(b)
    }

    #[test]
    fn focus_set_and_clear() {
        let mut w = World::new();
        let character = being(&mut w, Player, "a pilot");
        let robot = being(&mut w, Creature, "a robot");

        assert_eq!(w.focus_of(character), None);
        w.set_focus(character, robot).unwrap();
        assert_eq!(w.focus_of(character), Some(robot));
        w.clear_focus(character);
        assert_eq!(w.focus_of(character), None);
    }

    #[test]
    fn controls_and_focus_round_trip() {
        let mut w = World::new();
        let character = being(&mut w, Player, "a pilot");
        let robot = being(&mut w, Creature, "a robot");
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
        let character = being(&mut w, Player, "a pilot");
        let robot = being(&mut w, Creature, "a robot");
        w.relate::<Controls>(robot, character).unwrap();

        w.despawn(character);

        // The robot outlives its controller, now controlled by no one (Detach).
        assert!(w.entity(robot).is_some());
        assert_eq!(w.target_of::<Controls>(robot), None);
    }

    #[test]
    fn despawning_puppet_resets_focus() {
        let mut w = World::new();
        let character = being(&mut w, Player, "a pilot");
        let robot = being(&mut w, Creature, "a robot");
        w.relate::<Controls>(robot, character).unwrap();
        w.set_focus(character, robot).unwrap();
        assert_eq!(w.focus_of(character), Some(robot));

        w.despawn(robot);

        // The Focus Detach cascade cleared the cursor; the controller is intact.
        assert!(w.entity(character).is_some());
        assert_eq!(w.focus_of(character), None);
    }
}
