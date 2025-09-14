//! ProximaDB License Management System
//!
//! Comprehensive license tier management supporting both SaaS and on-premises deployments
//! with privacy-conscious customer requirements (air-gapped, VPC-only deployments)

pub mod license_manager;
pub mod tier_enforcement;
pub mod offline_validation;
pub mod feature_gates;

pub use license_manager::{LicenseManager, LicenseInfo, LicenseStatus, LicenseTier};
pub use tier_enforcement::{TierEnforcement, FeatureLimit};
pub use offline_validation::{OfflineLicenseValidator, LicenseToken, LicenseValidation};
pub use feature_gates::{FeatureGate, FeatureAccess, FeatureValidation};