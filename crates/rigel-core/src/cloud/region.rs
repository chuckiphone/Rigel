//! Project data residency pinning.
//!
//! A pin records the Firebase region selected for a project's Firestore database. Firebase
//! does not support moving an existing database in place, so changing an existing pin is a
//! deliberate, confirmed metadata change; it does not copy or migrate cloud documents.

use serde::{Deserialize, Serialize};

use super::{CloudConfig, CloudError};

/// A project region persisted in `cloud.json` and shown by the data-location panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RegionPin(String);

impl RegionPin {
    pub fn parse(value: &str) -> Result<Self, CloudError> {
        let value = value.trim().to_ascii_lowercase();
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.split('-').all(|part| {
                !part.is_empty()
                    && part.chars().all(|character| {
                        character.is_ascii_lowercase() || character.is_ascii_digit()
                    })
            });
        if !valid {
            return Err(CloudError::Input(
                "region must be a Firebase region identifier such as us-central1 or europe-west1"
                    .to_string(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Exact phrase required before changing a stored region pin.
pub fn move_confirmation(project_id: &str, current: &str, target: &str) -> String {
    format!("MOVE {project_id} FROM {current} TO {target}")
}

/// Validate and apply a region pin, refusing any change to an existing pin without an exact
/// confirmation. The caller persists the config only after this succeeds.
pub fn set_pin(
    config: &mut CloudConfig,
    target: &str,
    confirmation: Option<&str>,
) -> Result<(), CloudError> {
    let target = RegionPin::parse(target)?;
    if let Some(current) = &config.region
        && current != &target
    {
        let expected = move_confirmation(&config.project_id, current.as_str(), target.as_str());
        if confirmation != Some(expected.as_str()) {
            return Err(CloudError::Input(format!(
                "changing the data region requires explicit confirmation; type `{expected}`"
            )));
        }
    }
    config.region = Some(target);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> CloudConfig {
        CloudConfig::new("sample-project", "public-key").expect("valid fixture")
    }

    #[test]
    fn region_pin_is_normalized_and_persisted() {
        let mut config = config();
        set_pin(&mut config, " Europe-West1 ", None).expect("first pin needs no move confirm");
        assert_eq!(
            config.region.as_ref().map(RegionPin::as_str),
            Some("europe-west1")
        );
    }

    #[test]
    fn cross_region_change_requires_exact_confirmation() {
        let mut config = config();
        set_pin(&mut config, "us-central1", None).expect("initial region pin");
        assert!(set_pin(&mut config, "europe-west1", Some("yes")).is_err());
        assert_eq!(
            config.region.as_ref().map(RegionPin::as_str),
            Some("us-central1")
        );
        let confirmation = move_confirmation("sample-project", "us-central1", "europe-west1");
        set_pin(&mut config, "europe-west1", Some(&confirmation)).expect("confirmed move");
        assert_eq!(
            config.region.as_ref().map(RegionPin::as_str),
            Some("europe-west1")
        );
    }

    #[test]
    fn invalid_region_is_rejected() {
        assert!(RegionPin::parse("../elsewhere").is_err());
    }

    #[test]
    fn pin_survives_cloud_config_round_trip() {
        let directory = tempfile::tempdir().expect("temporary config directory");
        let mut config = config();
        set_pin(&mut config, "asia-northeast1", None).expect("pin region");
        CloudConfig::save(directory.path(), &config).expect("save cloud config");
        let loaded = CloudConfig::load(directory.path())
            .expect("load cloud config")
            .expect("config exists");
        assert_eq!(loaded.region, config.region);
    }
}
