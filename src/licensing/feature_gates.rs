//! Feature Gates - stub implementation

pub use super::license_manager::UsageContext;

/// Gate that controls access to a specific licensed feature.
#[derive(Debug, Clone)]
pub struct FeatureGate;

/// Result of a feature access check, indicating whether the caller may use a feature.
#[derive(Debug, Clone)]
pub struct FeatureAccess;

/// Validation outcome for a feature access request.
#[derive(Debug, Clone)]
pub struct FeatureValidation;
