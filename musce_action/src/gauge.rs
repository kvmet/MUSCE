//! The value vocabulary for gauges: bounded, derived measurements whose exact
//! backing representation stays app-owned. This module deliberately contains no
//! evaluator registry, predicates, effects, or planner integration; those are
//! consumers of these types, not part of the bounded quantity itself.

/// The stable symbolic name of a gauge in app vocabulary.
///
/// It is a string today for the same reason relation kinds and component tags are
/// strings in the affordance vocabulary: app symbols eventually cross type-erased
/// seams. Interning remains a transparent future optimization.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GaugeId(pub String);

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
        Self(name)
    }
}

/// A normalized, ordinal gauge reading.
///
/// Every `u8` is valid. `0` and `255` are the saturated endpoints; interior
/// values carry only order, not a universal magnitude, unit, or label. An app
/// keeps the exact backing value in its component and maps it monotonically into
/// this space when a gauge is read.
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

/// An orientation on a gauge's ordered space.
///
/// This says only which way a change moves. It carries no magnitude and no
/// valence: whether `Up` is desirable is policy belonging to the goal that asks
/// for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GaugeDirection {
    Down,
    Up,
}

/// An inclusive target interval on a gauge.
///
/// A target is satisfied anywhere from `min` through `max`. Below the interval
/// it requires [`GaugeDirection::Up`]; above it, [`GaugeDirection::Down`]. This
/// makes a threshold, a tolerance band, and an exact endpoint the same small
/// algebra without assigning universal names to interior levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GaugeTarget {
    min: GaugeLevel,
    max: GaugeLevel,
}

impl GaugeTarget {
    /// The singleton target at lower saturation.
    pub const MIN: Self = Self::at(GaugeLevel::MIN);

    /// The singleton target at upper saturation.
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
    fn level_preserves_the_whole_byte_space() {
        assert_eq!(GaugeLevel::MIN.get(), 0);
        assert_eq!(GaugeLevel::new(127).get(), 127);
        assert_eq!(GaugeLevel::MAX.get(), 255);
        assert!(GaugeLevel::MIN.is_min());
        assert!(GaugeLevel::MAX.is_max());
    }

    #[test]
    fn at_least_drives_up_until_satisfied() {
        let target = GaugeTarget::at_least(GaugeLevel::new(80));
        assert_eq!(
            target.required_change(GaugeLevel::new(79)),
            Some(GaugeDirection::Up)
        );
        assert_eq!(target.required_change(GaugeLevel::new(80)), None);
        assert_eq!(target.required_change(GaugeLevel::new(200)), None);
    }

    #[test]
    fn at_most_drives_down_until_satisfied() {
        let target = GaugeTarget::at_most(GaugeLevel::new(80));
        assert_eq!(target.required_change(GaugeLevel::new(20)), None);
        assert_eq!(target.required_change(GaugeLevel::new(80)), None);
        assert_eq!(
            target.required_change(GaugeLevel::new(81)),
            Some(GaugeDirection::Down)
        );
    }

    #[test]
    fn interval_drives_toward_its_nearest_edge() {
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

    #[test]
    fn exact_level_is_a_singleton_target() {
        let target = GaugeTarget::at(GaugeLevel::new(80));
        assert_eq!(
            target.required_change(GaugeLevel::new(79)),
            Some(GaugeDirection::Up)
        );
        assert_eq!(target.required_change(GaugeLevel::new(80)), None);
        assert_eq!(
            target.required_change(GaugeLevel::new(81)),
            Some(GaugeDirection::Down)
        );
    }
}
