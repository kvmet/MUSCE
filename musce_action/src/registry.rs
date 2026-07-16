//! The capability interner: the one place a capability *name* becomes a [`CapId`].
//!
//! The app declares its capability vocabulary here while wiring its gates, and the
//! same registry resolves an account's stored grant *names* to the same ids, so a
//! gate's id and a grant's id denote one capability. Ids are minted in registration
//! order and are meaningful only within a single run: they are runtime handles, not
//! stable keys, which is why grants persist as names and resolve through here at
//! load. The engine registers no capabilities of its own (superuser is a separate
//! account axis, not a capability); every name here is the app's.

use std::collections::HashMap;

use crate::{CapId, CapSet};

/// A name -> [`CapId`] interner. Built once while the app wires its gates, then read
/// to resolve grants; it does not shrink, so an id, once minted, stays valid for the
/// run.
#[derive(Debug, Default)]
pub struct CapRegistry {
    ids: HashMap<String, CapId>,
}

impl CapRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern a capability name, returning its id. Idempotent: a name already known
    /// returns its existing id, so a gate and a grant naming the same capability
    /// converge on one id regardless of which registers first.
    pub fn register(&mut self, name: &str) -> CapId {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = CapId(self.ids.len() as u32);
        self.ids.insert(name.to_owned(), id);
        id
    }

    /// Resolve a name registered earlier, or `None` if it was never registered.
    pub fn resolve(&self, name: &str) -> Option<CapId> {
        self.ids.get(name).copied()
    }

    /// Resolve a batch of grant names into a [`CapSet`], returning the names that did
    /// not resolve *separately* rather than dropping them. An unknown name means an
    /// account holds a grant the current vocabulary no longer defines (a removed
    /// capability, or drift); the caller surfaces it (a log line) rather than
    /// silently losing the grant.
    pub fn resolve_set<I, S>(&self, names: I) -> (CapSet, Vec<String>)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut resolved = CapSet::new();
        let mut unknown = Vec::new();
        for name in names {
            let name = name.as_ref();
            match self.resolve(name) {
                Some(id) => {
                    resolved.insert(id);
                }
                None => unknown.push(name.to_owned()),
            }
        }
        (resolved, unknown)
    }

    /// How many capabilities are registered.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether no capability is registered.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_is_idempotent_and_ids_are_distinct() {
        let mut reg = CapRegistry::new();
        let build = reg.register("build");
        assert_eq!(reg.register("build"), build, "same name, same id");

        let teleport = reg.register("teleport");
        assert_ne!(build, teleport, "distinct names get distinct ids");
        assert_eq!(reg.len(), 2, "the repeat did not mint a third id");
    }

    #[test]
    fn resolve_reports_unknown_names() {
        let mut reg = CapRegistry::new();
        let build = reg.register("build");
        assert_eq!(reg.resolve("build"), Some(build));
        assert_eq!(reg.resolve("nonesuch"), None);
    }

    #[test]
    fn resolve_set_separates_the_unknown_from_the_resolved() {
        let mut reg = CapRegistry::new();
        let build = reg.register("build");
        let teleport = reg.register("teleport");

        // A grant list mixing known names with one the vocabulary no longer defines:
        // the known ones resolve, the stray one comes back named, not dropped.
        let grants = [
            "build".to_string(),
            "ghost".to_string(),
            "teleport".to_string(),
        ];
        let (set, unknown) = reg.resolve_set(&grants);

        assert!(set.contains(build));
        assert!(set.contains(teleport));
        assert_eq!(set.len(), 2);
        assert_eq!(unknown, ["ghost".to_string()]);
    }

    #[test]
    fn resolve_set_of_all_known_names_reports_nothing_unknown() {
        let mut reg = CapRegistry::new();
        reg.register("build");
        let (set, unknown) = reg.resolve_set(["build"]);
        assert_eq!(set.len(), 1);
        assert!(unknown.is_empty());
    }
}
