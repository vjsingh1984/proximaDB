/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Compute Provider Module
//!
//! This module defines the pluggable compute engine interface for Hadoop-style storage-compute
//! separation. Compute providers handle query execution, transformations, and analytics while
//! storage formats handle data serialization/deserialization.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                         COMPUTE PROVIDER LAYER                               │
//! │  ┌───────────────────────────────────────────────────────────────────────┐  │
//! │  │                      ComputeProvider Trait                             │  │
//! │  │   execute() | can_execute() | estimate_cost() | capabilities()        │  │
//! │  └───────────────────────────────────────────────────────────────────────┘  │
//! │                                    │                                         │
//! │          ┌─────────────────────────┼─────────────────────────┐              │
//! │          ▼                         ▼                         ▼              │
//! │  ┌───────────────┐        ┌───────────────┐        ┌───────────────┐       │
//! │  │    Local      │        │   Spark       │        │   DuckDB      │       │
//! │  │   Provider    │        │  Provider     │        │   Provider    │       │
//! │  │ (Default SQL) │        │ (Distributed) │        │ (Analytics)   │       │
//! │  └───────────────┘        └───────────────┘        └───────────────┘       │
//! │          │                         │                         │              │
//! │          └─────────────────────────┼─────────────────────────┘              │
//! │                                    ▼                                         │
//! │                         ┌─────────────────────┐                              │
//! │                         │   ComputeScheduler   │                             │
//! │                         │  Provider Selection  │                             │
//! │                         └─────────────────────┘                              │
//! │                                    │                                         │
//! │                                    ▼                                         │
//! │                         ┌─────────────────────┐                              │
//! │                         │  Arrow RecordBatch  │                              │
//! │                         │   (Data Exchange)   │                              │
//! │                         └─────────────────────┘                              │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Design Principles
//!
//! 1. **Pluggable Engines**: Any compute engine implementing `ComputeProvider` can be used.
//!
//! 2. **Cost-Based Selection**: The scheduler uses cost estimates to select the best provider.
//!
//! 3. **Capability Matching**: Providers advertise capabilities (vector search, graph traversal, etc.)
//!    and the scheduler routes plans to providers that support the required operations.
//!
//! 4. **Arrow as Exchange**: All data flows through Arrow RecordBatch for zero-copy compatibility.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::compute::provider::{ComputeProvider, LocalComputeProvider};
//! use proximadb::compute::plan::ComputePlan;
//!
//! // Create local compute provider
//! let provider = LocalComputeProvider::new()?;
//!
//! // Check if plan can be executed
//! if provider.can_execute(&plan) {
//!     // Get cost estimate
//!     let cost = provider.estimate_cost(&plan)?;
//!
//!     // Execute the plan
//!     let results = provider.execute(&plan).await?;
//! }
//! ```

// Core traits
pub mod traits;

// Local compute provider (default)
pub mod local;

// Re-exports for convenience
pub use traits::{
    ComputeCapabilities, ComputeProvider, CostEstimate, ExecutionContext, ProviderMetrics,
};

pub use local::LocalComputeProvider;

// ============================================================================
// Module-Level Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_capabilities_default() {
        let caps = ComputeCapabilities::default();
        // Default capabilities should be conservative
        assert!(!caps.supports_aggregate_pushdown);
        assert!(!caps.supports_graph_traversal);
        assert_eq!(caps.max_parallelism, 1);
    }

    #[test]
    fn test_cost_estimate_total() {
        let cost = CostEstimate {
            cpu_cost: 100.0,
            io_cost: 50.0,
            network_cost: 25.0,
            memory_bytes: 1024,
            estimated_rows: 10000,
        };
        // Total cost should be sum of all costs
        let total = cost.cpu_cost + cost.io_cost + cost.network_cost;
        assert_eq!(total, 175.0);
    }
}
