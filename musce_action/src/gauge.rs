//! Foundational gauge values: stable gauge names, normalized ordinal readings,
//! directional change, and inclusive runtime targets. Evaluation, qualitative
//! regions, and planner integration are separate layers built over this algebra.

/// The stable symbolic name of a gauge in app vocabulary.
///
/// The representation is private so callers depend on symbolic identity rather
/// than allocation strategy; a later registry may intern names without changing
/// this API.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GaugeId(String);

impl GaugeId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for GaugeId {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for GaugeId {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

/// A normalized, ordinal gauge reading. Every `u8` is valid; `0` and `255` are
/// saturated endpoints. Interior values carry order, not universal magnitude,
/// units, or labels.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GaugeLevel(u8);

impl GaugeLevel {
    pub const MIN: Self = Self(u8::MIN);
    pub const MAX: Self = Self(u8::MAX);

    pub const fn new(level: u8) -> Self {
        Self(level)
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub const fn is_min(self) -> bool {
        self.0 == u8::MIN
    }

    pub const fn is_max(self) -> bool {
        self.0 == u8::MAX
    }
}

impl From<u8> for GaugeLevel {
    fn from(level: u8) -> Self {
        Self::new(level)
    }
}

impl From<GaugeLevel> for u8 {
    fn from(level: GaugeLevel) -> Self {
        level.get()
    }
}

/// Which way a gauge must move. Direction carries no magnitude or desirability;
/// those belong to the action and goal that use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GaugeDirection {
    Down,
    Up,
}

/// An inclusive runtime target interval on a gauge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GaugeTarget {
    min: GaugeLevel,
    max: GaugeLevel,
}

impl GaugeTarget {
    pub const MIN: Self = Self::at(GaugeLevel::MIN);
    pub const MAX: Self = Self::at(GaugeLevel::MAX);

    /// Only `level` satisfies this target.
    pub const fn at(level: GaugeLevel) -> Self {
        Self {
            min: level,
            max: level,
        }
    }

    /// A valid inclusive interval, or `None` when `min` is above `max`.
    pub const fn between(min: GaugeLevel, max: GaugeLevel) -> Option<Self> {
        if min.get() <= max.get() {
            Some(Self { min, max })
        } else {
            None
        }
    }

    /// Any reading at or above `min` satisfies this target.
    pub const fn at_least(min: GaugeLevel) -> Self {
        Self {
            min,
            max: GaugeLevel::MAX,
        }
    }

    /// Any reading at or below `max` satisfies this target.
    pub const fn at_most(max: GaugeLevel) -> Self {
        Self {
            min: GaugeLevel::MIN,
            max,
        }
    }

    pub const fn min(self) -> GaugeLevel {
        self.min
    }

    pub const fn max(self) -> GaugeLevel {
        self.max
    }

    pub const fn contains(self, level: GaugeLevel) -> bool {
        level.get() >= self.min.get() && level.get() <= self.max.get()
    }

    /// The direction needed to enter this target, or `None` when already in it.
    pub const fn required_change(self, current: GaugeLevel) -> Option<GaugeDirection> {
        if current.get() < self.min.get() {
            Some(GaugeDirection::Up)
        } else if current.get() > self.max.get() {
            Some(GaugeDirection::Down)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_identity_is_symbolic_and_opaque() {
        let health = GaugeId::new("health");
        assert_eq!(health.as_str(), "health");
        assert_eq!(health, GaugeId::from("health"));
    }

    #[test]
    fn level_preserves_the_whole_byte_space() {
        assert_eq!(GaugeLevel::MIN.get(), 0);
        assert_eq!(GaugeLevel::new(127).get(), 127);
        assert_eq!(GaugeLevel::MAX.get(), 255);
        assert!(GaugeLevel::MIN.is_min());
        assert!(GaugeLevel::MAX.is_max());
    }

    #[test]
    fn targets_drive_toward_their_interval() {
        let target = GaugeTarget::between(GaugeLevel::new(80), GaugeLevel::new(120)).unwrap();
        assert_eq!(
            target.required_change(GaugeLevel::new(79)),
            Some(GaugeDirection::Up)
        );
        assert!(target.contains(GaugeLevel::new(100)));
        assert_eq!(target.required_change(GaugeLevel::new(100)), None);
        assert_eq!(
            target.required_change(GaugeLevel::new(121)),
            Some(GaugeDirection::Down)
        );
    }

    #[test]
    fn one_sided_targets_saturate_at_the_opposite_endpoint() {
        let high = GaugeTarget::at_least(GaugeLevel::new(80));
        assert_eq!(
            high.required_change(GaugeLevel::new(79)),
            Some(GaugeDirection::Up)
        );
        assert!(high.contains(GaugeLevel::MAX));

        let low = GaugeTarget::at_most(GaugeLevel::new(80));
        assert_eq!(
            low.required_change(GaugeLevel::new(81)),
            Some(GaugeDirection::Down)
        );
        assert!(low.contains(GaugeLevel::MIN));
    }

    #[test]
    fn reversed_interval_is_rejected() {
        assert_eq!(
            GaugeTarget::between(GaugeLevel::new(120), GaugeLevel::new(80)),
            None
        );
    }

    #[test]
    fn endpoint_targets_are_singletons() {
        assert!(GaugeTarget::MIN.contains(GaugeLevel::MIN));
        assert_eq!(
            GaugeTarget::MIN.required_change(GaugeLevel::new(1)),
            Some(GaugeDirection::Down)
        );
        assert!(GaugeTarget::MAX.contains(GaugeLevel::MAX));
        assert_eq!(
            GaugeTarget::MAX.required_change(GaugeLevel::new(254)),
            Some(GaugeDirection::Up)
        );
    }
}
