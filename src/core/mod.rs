pub mod config;
pub mod error;
pub mod global_coordination;
pub mod routing;
pub mod serverless;
pub mod storage_layout;
pub mod avro_unified;

// Legacy modules - kept temporarily for compatibility during migration
mod types;
mod unified_types;

pub use config::*;
pub use error::*;
pub use global_coordination::{
    DeploymentTopology, GlobalCoordinationConfig, GlobalMetadataCoordinator,
};
pub use routing::{
    AccountTier, CustomerSegment, RoutingContext, RoutingDecision, SmartRouter, WorkloadType,
};
pub use serverless::*;
// Use avro_unified types as the single source of truth
pub use avro_unified::*;
