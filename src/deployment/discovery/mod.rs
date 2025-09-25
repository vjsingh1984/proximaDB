//! Environment Discovery Module

pub mod environment_detector;

pub use environment_detector::{
    EnvironmentDetector, DetectedEnvironment, PlatformType,
    ResourceAvailability, NetworkConfig, SecurityConstraints,
    PerformanceProfile, DeploymentRecommendation, MonitoringConfig, BackupStrategy,
    ComplianceFramework, EncryptionRequirements, OptimalConfig, DeploymentStrategy, ScalingConfig, CapacityEstimate
};