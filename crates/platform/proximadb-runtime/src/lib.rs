//! Platform runtime composition helpers.
//!
//! This crate owns host/runtime policy that composes lower contract crates without
//! pushing system inspection, tracing, or bootstrap behavior into foundation crates.

pub mod cluster_port;
pub mod composition;
pub mod document_port;
pub mod entity_port;
pub mod hardware;
pub mod handlers;
pub mod hybrid_port;
pub mod observability_port;
pub mod port;
pub mod proto_defaults;
pub mod resources;
pub mod security_port;
pub mod service_ports;

// Re-exports
pub use cluster_port::{ClusterHealthStatus, ClusterPort};
pub use composition::{DIContainer, ServiceComposer};
pub use document_port::DocumentPort;
pub use entity_port::EntityPort;
pub use hardware::{HardwareCapabilities, SimdLevel, best_simd_level, hardware_capabilities};
pub use handlers::{CollectionIdCache, UnifiedHandlers};
pub use hybrid_port::HybridPort;
pub use observability_port::ObservabilityPort;
pub use port::ApiHandlersPort;
pub use resources::{MemoryBudget, ResourceManager};
pub use security_port::SecurityPort;
pub use service_ports::{CollectionPort, QueryAdapterPort, VectorOpsPort};
