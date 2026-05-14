//! Platform runtime composition helpers.
//!
//! This crate owns host/runtime policy that composes lower contract crates without
//! pushing system inspection, tracing, or bootstrap behavior into foundation crates.

pub mod composition;
pub mod hardware;
pub mod handlers;
pub mod port;
pub mod proto_defaults;
pub mod resources;
pub mod service_ports;

// Re-exports
pub use composition::{DIContainer, ServiceComposer};
pub use hardware::{HardwareCapabilities, SimdLevel, best_simd_level, hardware_capabilities};
pub use handlers::{CollectionIdCache, UnifiedHandlers};
pub use port::ApiHandlersPort;
pub use resources::{MemoryBudget, ResourceManager};
pub use service_ports::{CollectionPort, QueryAdapterPort, VectorOpsPort};
