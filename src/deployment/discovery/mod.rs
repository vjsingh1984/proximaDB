//! Environment Discovery Module

pub mod environment_detector;

pub use environment_detector::{
    BackupStrategy, CapacityEstimate, ComplianceFramework, DeploymentRecommendation,
    DeploymentStrategy, DetectedEnvironment, EncryptionRequirements, EnvironmentDetector,
    MonitoringConfig, NetworkConfig, OptimalConfig, PerformanceProfile, PlatformType,
    ResourceAvailability, ScalingConfig, SecurityConstraints,
};
