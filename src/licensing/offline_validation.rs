//! Offline License Validation - stub implementation

pub use super::license_manager::{LicenseInfo, LicenseStatus};

/// Validates licenses in fully offline or air-gapped deployment environments.
#[derive(Debug, Clone)]
pub struct OfflineLicenseValidator;

/// A cryptographically signed token used to represent a license in offline deployments.
#[derive(Debug, Clone)]
pub struct LicenseToken;

/// Result of an offline license validation check.
#[derive(Debug, Clone)]
pub struct LicenseValidation;

impl OfflineLicenseValidator {
    /// Create a new `OfflineLicenseValidator` from the provided license configuration.
    pub fn new(_config: &super::license_manager::LicenseConfig) -> Result<Self, anyhow::Error> {
        Ok(Self)
    }

    /// Validate a `LicenseInfo` without making any network calls.
    ///
    /// Returns the current [`LicenseStatus`] based solely on local state.
    pub async fn validate_offline_license(
        &self,
        _license: &LicenseInfo,
    ) -> Result<LicenseStatus, anyhow::Error> {
        Ok(LicenseStatus::Valid)
    }

    /// Parse and validate a raw license token string, returning the decoded [`LicenseInfo`].
    pub async fn validate_license_token(&self, _token: &str) -> Result<LicenseInfo, anyhow::Error> {
        Err(anyhow::anyhow!("Not implemented"))
    }
}
