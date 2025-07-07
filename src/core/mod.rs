pub mod avro_serialization;
pub mod avro_unified;
pub mod config;
pub mod error;
pub mod global_coordination;
pub mod grpc_metadata_parser;
pub mod index;
pub mod indexing;
pub mod metadata_query;
pub mod routing;
pub mod search;
pub mod serverless;
pub mod storage_layout;

// Legacy modules removed - using avro_unified as single source of truth

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
pub use metadata_query::*;
pub use grpc_metadata_parser::*;
