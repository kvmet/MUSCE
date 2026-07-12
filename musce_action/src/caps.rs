//! Capability identity and the authorization verdict: the primitives the gate
//! check compares. This layer decides only whether a verdict admits a capability;
//! it knows nothing of accounts, sessions, or where grants come from. A [`CapId`]
//! is an opaque handle the account layer's caps registry mints (see the auth module
//! in `musce_host`); the same registry resolves an account's grant strings to the
//! same ids, so a gate's id and a grant's id denote the same capability. See
//! `docs/architecture/accounts.md`.

use std::collections::HashSet;

/// An interned capability id. Opaque: minted by the account layer's caps registry,
/// only compared here. Equality is identity, so two ids name the same capability iff
/// they came from the same registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CapId(pub u32);

/// The set of capabilities an account holds, resolved from its grant strings to ids
/// once at load. Membership is the whole query a gate asks of it.
#[derive(Clone, Debug, Default)]
pub struct CapSet(HashSet<CapId>);

impl CapSet {
    /// An empty grant set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a capability; returns whether it was newly inserted.
    pub fn insert(&mut self, cap: CapId) -> bool {
        self.0.insert(cap)
    }

    /// Remove a capability; returns whether it was present. The account layer's
    /// revoke path uses it to drop a grant without rebuilding the whole set.
    pub fn remove(&mut self, cap: CapId) -> bool {
        self.0.remove(&cap)
    }

    /// Whether this set holds `cap`.
    pub fn contains(&self, cap: CapId) -> bool {
        self.0.contains(&cap)
    }

    /// How many capabilities are granted.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no capability is granted.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<CapId> for CapSet {
    fn from_iter<I: IntoIterator<Item = CapId>>(iter: I) -> Self {
        CapSet(iter.into_iter().collect())
    }
}

/// The resolved authorization a command runs under: the account's granted
/// capabilities and whether superuser is in force (the account's su bit set, and the
/// connection not quelled). The account authority builds it from `conn -> account`,
/// carrying nothing derivable from the actor or the world, so possessing or
/// `@play`-selecting a privileged body cannot borrow authority. A gate and a game's
/// inline rules both read it; neither can mutate it.
#[derive(Clone, Debug)]
pub struct Verdict {
    caps: CapSet,
    su_override: bool,
}

impl Verdict {
    /// The verdict for an account: its resolved caps, and whether su is in force.
    pub fn new(caps: CapSet, su_override: bool) -> Self {
        Verdict { caps, su_override }
    }

    /// A connection with no account: no caps, no su. The `Open`-only floor an
    /// unauthenticated or guest connection runs under.
    pub fn guest() -> Self {
        Verdict {
            caps: CapSet::new(),
            su_override: false,
        }
    }

    /// Whether this verdict admits `cap`: su in force bypasses the grant set;
    /// otherwise it is plain membership. This is the entire gate check for a
    /// capability-gated verb.
    pub fn permits(&self, cap: CapId) -> bool {
        self.su_override || self.caps.contains(cap)
    }

    /// Whether superuser is in force. A game's inline rule reads this to decide
    /// whether to wave su through a scoped check the flat gate cannot express.
    pub fn is_su(&self) -> bool {
        self.su_override
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_is_membership_without_su() {
        let build = CapId(0);
        let ban = CapId(1);
        let verdict = Verdict::new([build].into_iter().collect(), false);
        assert!(verdict.permits(build), "granted cap is admitted");
        assert!(!verdict.permits(ban), "ungranted cap is refused");
        assert!(!verdict.is_su());
    }

    #[test]
    fn su_bypasses_the_grant_set() {
        // su in force admits a cap the account was never granted.
        let ban = CapId(1);
        let verdict = Verdict::new(CapSet::new(), true);
        assert!(verdict.permits(ban), "su bypasses the empty grant set");
        assert!(verdict.is_su());
    }

    #[test]
    fn guest_admits_nothing() {
        let build = CapId(0);
        let guest = Verdict::guest();
        assert!(!guest.permits(build));
        assert!(!guest.is_su());
    }
}
