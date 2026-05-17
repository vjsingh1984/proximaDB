//! Compatibility shim for query capability contracts.
//!
//! The canonical capability registry lives in the `proximadb-query-capability`
//! workspace crate. This module preserves the historical root import surface.

pub use proximadb_query_capability::{
    Capability, CapabilityCheckError, CapabilityRegistry, CapabilitySet,
};

/// Backward-compatible nested module for callers that imported
/// `crate::query::capability::registry::*`.
pub mod registry {
    pub use proximadb_query_capability::{
        Capability, CapabilityCheckError, CapabilityRegistry, CapabilitySet,
    };
}
