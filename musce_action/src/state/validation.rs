use std::collections::HashMap;

use crate::GaugeId;
use crate::schema::GaugeRegionId;

use super::{GaugeRegion, StateRegistrationError};

pub(super) fn validate_gauge_name(id: &GaugeId) -> Result<(), StateRegistrationError> {
    let name = id.as_str();
    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(StateRegistrationError::InvalidGauge {
            gauge: name.to_string(),
            reason: "ids must be nonempty and contain no whitespace or control characters".into(),
        });
    }
    Ok(())
}

pub(super) fn validate_regions(
    gauge: &GaugeId,
    regions: &[GaugeRegion],
) -> Result<HashMap<GaugeRegionId, usize>, StateRegistrationError> {
    if regions.is_empty() {
        return Err(invalid_gauge(gauge, "at least one region is required"));
    }
    if !regions[0].target().min().is_min() {
        return Err(invalid_gauge(gauge, "the first region must start at MIN"));
    }
    if !regions.last().unwrap().target().max().is_max() {
        return Err(invalid_gauge(gauge, "the last region must end at MAX"));
    }

    let mut by_id = HashMap::new();
    let mut previous_max: Option<u8> = None;
    for (ordinal, region) in regions.iter().enumerate() {
        if by_id.insert(region.id().clone(), ordinal).is_some() {
            return Err(invalid_gauge(
                gauge,
                format!("duplicate region id {}", region.id()),
            ));
        }
        if let Some(previous) = previous_max {
            let Some(expected) = previous.checked_add(1) else {
                return Err(invalid_gauge(
                    gauge,
                    format!(
                        "region {} follows a region that already ends at MAX",
                        region.id()
                    ),
                ));
            };
            if region.target().min().get() != expected {
                return Err(invalid_gauge(
                    gauge,
                    format!(
                        "region {} must start at {expected}, immediately after the prior region",
                        region.id()
                    ),
                ));
            }
        }
        previous_max = Some(region.target().max().get());
    }
    Ok(by_id)
}

fn invalid_gauge(gauge: &GaugeId, reason: impl Into<String>) -> StateRegistrationError {
    StateRegistrationError::InvalidGauge {
        gauge: gauge.as_str().to_string(),
        reason: reason.into(),
    }
}
