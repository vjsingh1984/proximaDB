//! Platform runtime composition helpers.
//!
//! This crate owns host/runtime policy that composes lower contract crates without
//! pushing system inspection, tracing, or bootstrap behavior into foundation crates.

pub mod composition;
pub mod hardware;
pub mod proto_defaults;
pub mod resources;

// Re-exports
pub use composition::{DIContainer, ServiceComposer};
pub use hardware::{HardwareCapabilities, SimdLevel, best_simd_level, hardware_capabilities};
pub use resources::{MemoryBudget, ResourceManager};
