//! The narrated act: the single owner of *default* affordance narration. One
//! `perform_narrated` runs the silent, guarded, gate-checked act
//! ([`crate::agency::perform`]) and, on the outcome, emits the first- and
//! third-person prose an affordance produces, so a typed verb, a clicked control,
//! and an autonomous agent all narrate the same act the same way. Before this, the
//! prose lived in each verb handler, which is why a click narrated nothing to the
//! room and an NPC's beats were silent.
//!
//! Two things make it work across every performer:
//!
//! - **First-person is entity-addressed** (`to_entity(actor)`, not
//!   `to_connection`). It resolves to whatever connection drives the actor, so a
//!   player reads "You take it", a *piloting* player reads it through the body they
//!   drive, and an NPC's first-person reaches no one, all from one push. The room
//!   line reaches bystanders either way.
//! - **The frame is already ground.** A verb resolves it by name, a click grounds
//!   it by id, a planner binds it mid-search; by the time an act reaches here every
//!   role a guard needs is bound, so this only dispatches and narrates.
//!
//! This owns the *default* prose. A performer that wants bespoke, goal-flavored
//! narration (the magpie's "tucks it into its nest" for a `put` serving its hoard
//! drive) opts out: it runs the silent [`crate::agency::perform`] and emits its own
//! line, suppressing the default rather than layering on it. See
//! `docs/architecture/actions.md`.

use musce::action::{Event, Outbound, Verdict};
use musce::agency::{Affordance, Frame};
use musce::wire::EventKind;
use musce::world::{EntityId, World};

use crate::names::display_name;
use crate::verbs::Outcome;

/// Perform `affordance` for `actor` on the already-ground `frame`, then narrate the
/// outcome into `out`: on commit, the first-person line to the actor and the
/// third-person line to the room; on refusal, the reason to the actor. Returns the
/// [`Outcome`] so a driver can read committed-or-refused for its next beat. The act
/// itself (gate, guards, mutation) is the shared [`crate::agency::perform`]; this
/// adds only the prose, so a verb, a click, and a plan step cannot narrate the same
/// act differently.
pub(crate) fn perform_narrated(
    world: &mut World,
    actor: EntityId,
    affordance: &Affordance,
    frame: &Frame,
    verdict: &Verdict,
    out: &mut Vec<Outbound>,
) -> Outcome {
    // Read the prose before the act mutates, so a name the act consumes or moves is
    // still the thing the actor reached for.
    let (first, third) = narrate(world, actor, &affordance.name, frame);
    let outcome = crate::agency::perform(world, affordance, frame, verdict);
    match outcome {
        Outcome::Committed => {
            out.push(Outbound::new(Event::to_entity(
                actor,
                EventKind::Feedback,
                first,
            )));
            // take/drop/put/eat do not move the actor, so its locus now is the room
            // the act happened in; the actor reads the first-person line above, not
            // this one.
            if let (Some(third), Some(room)) = (third, world.enclosing_locus(actor)) {
                out.push(Outbound::excluding(
                    Event::to_locus(room, EventKind::Narration, third),
                    vec![actor],
                ));
            }
        }
        Outcome::Refused(reason) => {
            out.push(Outbound::new(Event::to_entity(
                actor,
                EventKind::Feedback,
                reason,
            )));
        }
    }
    outcome
}

/// The default prose for an affordance: `(first_person, third_person)`, the second
/// `None` for an act with no room-facing line. Keyed by affordance name, the same
/// key [`crate::agency::perform`] dispatches on, so prose and dispatch cannot drift.
/// An affordance with no template (a click on `go`, whose real narration is the
/// deferred dual-locus movement) falls back to a bare first-person confirmation,
/// preserving the pre-narrator click behavior until it joins the narrated set.
fn narrate(world: &World, actor: EntityId, name: &str, frame: &Frame) -> (String, Option<String>) {
    let who = display_name(world, actor);
    let object = frame.object.map(|o| display_name(world, o));
    let target = frame.target.map(|t| display_name(world, t));
    match name {
        "take" | "drop" | "eat" => {
            let thing = object.unwrap_or_else(|| "that".into());
            let (first_verb, third_verb) = match name {
                "take" => ("take", "takes"),
                "drop" => ("drop", "drops"),
                _ => ("eat", "eats"),
            };
            (
                format!("You {first_verb} {thing}."),
                Some(format!("{who} {third_verb} {thing}.")),
            )
        }
        "put" => {
            let thing = object.unwrap_or_else(|| "that".into());
            let into = target.unwrap_or_else(|| "that".into());
            (
                format!("You put {thing} in {into}."),
                Some(format!("{who} puts {thing} in {into}.")),
            )
        }
        other => {
            // The clicked-`go` fallback: name whatever role bound the focus (the
            // exit is the target), first-person only. No room line: the real
            // movement narration lands with the derived-destination work.
            let thing = object.or(target).unwrap_or_else(|| "that".into());
            (format!("You {other} {thing}."), None)
        }
    }
}
