//! Vector Operations Service - Centralized Search Orchestration
//!
//! This module orchestrates all vector search operations across the system.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │           VectorOperationsService (Orchestrator)                 │
//! │  Unified entry point for all vector operations                   │
//! └─────────────────────────────────────────────────────────────────┘
//!                               │
//!         ┌─────────────────────┼─────────────────────┐
//!         ▼                     ▼                     ▼
//! ┌───────────────┐    ┌───────────────┐    ┌───────────────┐
//! │ Search Module │    │ Hybrid Module │    │ Validation    │
//! │ Progressive   │    │ AXIS Builder  │    │ Metadata      │
//! │ Pipeline      │    │               │    │ Enrichment    │
//! └───────────────┘    └──────────────┘    └───────────────┘
//! ```
//!
//! ## Module Organization
//!
//! - **config.rs**: Configuration types (`UnifiedSearchConfig`, `SearchPlanHints`)
//! - **hybrid/**: Hybrid query construction for vector+metadata filtering
//! - **validation/**: Metadata validation and pseudo-query generation
//! - **search/**: Search operations and progressive pipeline utilities
//! - **legacy.rs**: Main service implementation (being decomposed)
//!
//! ## Migration Notes
//!
//! This module is being decomposed from the original 4,269-line vectors.rs file.
//! The main `VectorOperationsService` is currently in legacy.rs and will be
//! further decomposed into focused submodules.

// Configuration types
pub mod config;

// Hybrid query construction
pub mod hybrid;

// Search operations and progressive pipeline
pub mod search;

// Metadata validation and pseudo-query generation
pub mod validation;

// Legacy main service implementation (being decomposed)
mod legacy;

// Public exports
pub use config::{SearchPlanHints, UnifiedSearchConfig};
pub use hybrid::build_axis_hybrid_query;
pub use legacy::{
    RichFilterCondition, RichFilterOperator, RichRecordBatchRequest, RichSearchRequest,
    VectorOperationsService,
};
pub use search::{
    executor::{SearchResult, proto_results_to_vector_records},
    pipeline::{ProgressiveSearchPipeline, StageResult, default_progressive_stages},
};
pub use validation::{
    DefaultPseudoQueryGenerator, PseudoQueryGenerator, apply_pseudo_query_metadata,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_structure() {
        // Verify module exports are accessible
        let _config = UnifiedSearchConfig::default();
        let _hints = SearchPlanHints::default();
        let _generator = DefaultPseudoQueryGenerator::default();
        let _stages = default_progressive_stages();
    }
}
