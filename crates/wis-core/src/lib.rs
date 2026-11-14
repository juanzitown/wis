use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Default for Region {
    fn default() -> Self {
        Self {
            x: 240,
            y: 180,
            width: 640,
            height: 240,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WisSettings {
    pub region: Region,
    pub scan_interval_ms: u64,
}

impl Default for WisSettings {
    fn default() -> Self {
        Self {
            region: Region::default(),
            scan_interval_ms: 1_500,
        }
    }
}

impl WisSettings {
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.region.width == 0 || self.region.height == 0 {
            return Err(SettingsError::EmptyRegion);
        }
        if self.region.width > 8_192 || self.region.height > 8_192 {
            return Err(SettingsError::RegionTooLarge);
        }
        if self.region.x.checked_add(self.region.width).is_none()
            || self.region.y.checked_add(self.region.height).is_none()
        {
            return Err(SettingsError::CoordinateOverflow);
        }
        if !(250..=60_000).contains(&self.scan_interval_ms) {
            return Err(SettingsError::InvalidInterval);
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SettingsError {
    #[error("the text area cannot be empty")]
    EmptyRegion,
    #[error("the text area is larger than 8192×8192")]
    RegionTooLarge,
    #[error("the text area coordinates overflow")]
    CoordinateOverflow,
    #[error("the scan interval must be between 250 and 60000 milliseconds")]
    InvalidInterval,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert_eq!(WisSettings::default().validate(), Ok(()));
    }

    #[test]
    fn rejects_an_interval_that_would_scan_too_fast() {
        let settings = WisSettings {
            scan_interval_ms: 100,
            ..WisSettings::default()
        };
        assert_eq!(settings.validate(), Err(SettingsError::InvalidInterval));
    }
}
