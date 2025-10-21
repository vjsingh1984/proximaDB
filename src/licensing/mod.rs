//! ProximaDB License Management System
//!
//! Comprehensive license tier management supporting both SaaS and on-premises deployments
//! with privacy-conscious customer requirements (air-gapped, VPC-only deployments)

pub mod feature_gates;
pub mod license_manager;
pub mod offline_validation;
pub mod tier_enforcement;

pub use feature_gates::{FeatureAccess, FeatureGate, FeatureValidation};
pub use license_manager::{LicenseInfo, LicenseManager, LicenseStatus, LicenseTier};
pub use offline_validation::{LicenseToken, LicenseValidation, OfflineLicenseValidator};
pub use tier_enforcement::{FeatureLimit, TierEnforcement};
