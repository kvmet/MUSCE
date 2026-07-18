//! The arbiter: which goal an agent pursues this tick, and the commitment that
//! keeps it from thrashing between two near-equal goals every tick.
//!
//! A [`Goal`] pairs a predicate (the [`Clause`] the planner regresses toward) with
//! an urgency. Drives emit goals; the arbiter ranks them and *commits* to one. Its
//! substance is not the ranking (a max) but the hysteresis: an incumbent goal holds
//! its commitment until a challenger's urgency exceeds it by a margin, so a small
//! tick-to-tick wobble in two comparable urgencies does not flip the agent back and
//! forth. See `docs/architecture/agency/arbiter.md`.
//!
//! The arbiter never reads the world. "Highest-urgency *unsatisfied* goal" is met
//! without a satisfaction test here: a met need is expressed as low urgency by its
//! drive (upstream), and an already-true goal surfaces as an empty plan the
//! [`Driver`](crate::Driver) reports as [`Progress::Achieved`](crate::Progress),
//! after which the caller calls [`release`](Arbiter::release) (downstream). So
//! satisfaction is detected once, by the planner, not duplicated into a second
//! `holds` pass the arbiter would have to get right for existential goals too.

use musce_action::Clause;

/// A goal's priority; higher is more urgent. A drive computes it from the NPC's own
/// need-state (a `Hunger`-driven curve); an imperative goal injects a fixed value.
pub type Urgency = u32;

/// A want handed to the arbiter: the predicate to make true and how urgently. The
/// predicate is a goal clause, ground or existential, the planner regresses toward.
/// Goal *identity* is its predicate, not its urgency: the same want re-offered with
/// a shifted urgency is the same commitment, refreshed, not a new goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    pub predicate: Clause,
    pub urgency: Urgency,
}

/// Picks the goal an agent pursues and commits to it across ticks. An incumbent
/// keeps its commitment until a challenger's urgency clears `incumbent + hysteresis`;
/// `hysteresis` is the anti-thrash margin (zero means "always take the current max",
/// no commitment).
pub struct Arbiter {
    committed: Option<Goal>,
    hysteresis: Urgency,
}

impl Arbiter {
    /// An arbiter with the given anti-thrash margin. A larger margin makes an agent
    /// more stubborn about its current goal; zero disables hysteresis.
    pub fn new(hysteresis: Urgency) -> Self {
        Arbiter {
            committed: None,
            hysteresis,
        }
    }

    /// The goal to pursue this tick, given the current candidate set (from drives or
    /// injected imperatives). Keeps the committed goal, its urgency refreshed from
    /// this tick's offering, unless it has left the set or some challenger's urgency
    /// exceeds it by more than `hysteresis`. An empty set commits to nothing.
    pub fn select(&mut self, goals: &[Goal]) -> Option<Goal> {
        self.committed = match self.committed.take() {
            Some(incumbent) => match goals.iter().find(|g| g.predicate == incumbent.predicate) {
                // The incumbent is still on offer: hold it unless a challenger clears
                // the hysteresis band. Refresh to this tick's urgency for the compare.
                Some(current) => {
                    let challenger = goals
                        .iter()
                        .filter(|g| g.predicate != incumbent.predicate)
                        .max_by_key(|g| g.urgency);
                    match challenger {
                        Some(c) if c.urgency > current.urgency.saturating_add(self.hysteresis) => {
                            Some(c.clone())
                        }
                        _ => Some(current.clone()),
                    }
                }
                // The incumbent's drive retired it: re-pick freely.
                None => highest(goals),
            },
            None => highest(goals),
        };
        self.committed.clone()
    }

    /// Drop the committed goal so the next [`select`](Arbiter::select) re-picks. The
    /// caller calls this when a pursuit ends: satisfied (an empty plan / an achieved
    /// run) or failed (no plan survived the exclusion set).
    pub fn release(&mut self) {
        self.committed = None;
    }

    /// The goal currently committed to, if any.
    pub fn committed(&self) -> Option<&Goal> {
        self.committed.as_ref()
    }
}

/// The highest-urgency goal in a set, or `None` if it is empty. On a tie the later
/// goal wins (`max_by_key`'s rule); commitment, not this, is what stabilizes choice.
fn highest(goals: &[Goal]) -> Option<Goal> {
    goals.iter().max_by_key(|g| g.urgency).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce_action::{Predicate, Term};

    // Distinct one-literal goal predicates, keyed by a tag name, so tests can name
    // goals without building the full vocabulary each time.
    fn want(tag: &str, urgency: Urgency) -> Goal {
        Goal {
            predicate: Clause(vec![
                Predicate::Tag {
                    e: Term::var("actor"),
                    comp: tag.into(),
                }
                .into(),
            ]),
            urgency,
        }
    }

    fn tag_of(goal: &Goal) -> String {
        match &goal.predicate.0[0].predicate {
            Predicate::Tag { comp, .. } => comp.clone(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn picks_the_highest_urgency_goal() {
        let mut a = Arbiter::new(0);
        let chosen = a.select(&[want("eat", 3), want("flee", 7), want("rest", 1)]);
        assert_eq!(tag_of(&chosen.unwrap()), "flee");
    }

    #[test]
    fn empty_candidate_set_commits_to_nothing() {
        let mut a = Arbiter::new(0);
        assert!(a.select(&[]).is_none());
        assert!(a.committed().is_none());
    }

    #[test]
    fn commitment_survives_a_near_equal_challenger() {
        // Committed to "eat" at urgency 5; next tick "flee" edges ahead at 6, but the
        // hysteresis band is 3, so the agent holds its goal rather than thrashing.
        let mut a = Arbiter::new(3);
        assert_eq!(tag_of(&a.select(&[want("eat", 5)]).unwrap()), "eat");
        let held = a.select(&[want("eat", 5), want("flee", 6)]).unwrap();
        assert_eq!(tag_of(&held), "eat");
    }

    #[test]
    fn a_challenger_clearing_the_band_steals_commitment() {
        // Same setup, but "flee" spikes to 9, clearing 5 + 3: now it wins.
        let mut a = Arbiter::new(3);
        a.select(&[want("eat", 5)]);
        let stolen = a.select(&[want("eat", 5), want("flee", 9)]).unwrap();
        assert_eq!(tag_of(&stolen), "flee");
    }

    #[test]
    fn the_incumbents_own_urgency_is_refreshed_each_tick() {
        // Committed to "eat" at 5. Next tick the drive has dropped "eat" to 1 while
        // "flee" sits at 3: 3 > 1 + 1, so the faded incumbent yields even though 3
        // never cleared the *original* 5. Identity is the predicate; urgency is live.
        let mut a = Arbiter::new(1);
        a.select(&[want("eat", 5)]);
        let now = a.select(&[want("eat", 1), want("flee", 3)]).unwrap();
        assert_eq!(tag_of(&now), "flee");
    }

    #[test]
    fn a_retired_incumbent_is_dropped_and_the_field_re_picked() {
        // "eat" is committed, then its drive stops offering it: the arbiter re-picks
        // from what remains rather than clinging to an absent goal.
        let mut a = Arbiter::new(100);
        a.select(&[want("eat", 9)]);
        let next = a.select(&[want("flee", 2), want("rest", 4)]).unwrap();
        assert_eq!(tag_of(&next), "rest");
    }

    #[test]
    fn release_drops_a_commitment_the_band_would_have_held() {
        // Committed to "eat" at 9, band 100. Next tick "eat" has faded to 1 and
        // "rest" is at 4; the band would hold the faded incumbent (4 < 1 + 100).
        // Releasing first clears the commitment, so the now-highest "rest" wins:
        // the release is what changed the outcome.
        let mut a = Arbiter::new(100);
        a.select(&[want("eat", 9)]);
        a.release();
        assert!(a.committed().is_none());
        let next = a.select(&[want("eat", 1), want("rest", 4)]).unwrap();
        assert_eq!(tag_of(&next), "rest");
    }
}
