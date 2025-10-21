//! Offline License Validation - stub implementation

pub use super::license_manager::{LicenseInfo, LicenseStatus};

#[derive(Debug, Clone)]
pub struct OfflineLicenseValidator;

#[derive(Debug, Clone)]
pub struct LicenseToken;

#[derive(Debug, Clone)]
pub struct LicenseValidation;

impl OfflineLicenseValidator {
    pub fn new(_config: &super::license_manager::LicenseConfig) -> Result<Self, anyhow::Error> {
        Ok(Self)
    }

    pub async fn validate_offline_license(
        &self,
        _license: &LicenseInfo,
    ) -> Result<LicenseStatus, anyhow::Error> {
        Ok(LicenseStatus::Valid)
    }

    pub async fn validate_license_token(&self, _token: &str) -> Result<LicenseInfo, anyhow::Error> {
        Err(anyhow::anyhow!("Not implemented"))
    }
}
