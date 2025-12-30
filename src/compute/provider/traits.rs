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

//! # Compute Provider Traits
//!
//! Core traits and types for the pluggable compute engine interface.
//! These traits enable Hadoop-style storage-compute separation where
//! multiple compute engines can operate on the same data.

use std::fmt::Debug;
use std::pin::Pin;
use std::time::Duration;

use anyhow::Result;
use arrow_array::RecordBatch;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::compute::plan::ComputePlan;

// ============================================================================
// Core Types
// ============================================================================

/// Stream of Arrow RecordBatches for efficient data transfer
pub type RecordBatchStream = Pin<Box<dyn Stream<Item = Result<RecordBatch>> + Send>>;

/// Compute capabilities advertised by a provider
///
/// Providers declare which operations they support, allowing the scheduler
/// to route plans to appropriate providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeCapabilities {
    /// Supports filter pushdown to storage layer
    pub supports_filter_pushdown: bool,

    /// Supports projection pushdown to storage layer
    pub supports_projection_pushdown: bool,

    /// Supports aggregate pushdown (e.g., COUNT, SUM at storage)
    pub supports_aggregate_pushdown: bool,

    /// Supports vector similarity search operations
    pub supports_vector_search: bool,

    /// Supports graph traversal operations
    pub supports_graph_traversal: bool,

    /// Supports full-text search operations
    pub supports_full_text_search: bool,

    /// Supports geospatial operations
    pub supports_geospatial: bool,

    /// Supports window functions
    pub supports_window_functions: bool,

    /// Supports distributed execution
    pub supports_distributed: bool,

    /// Supports streaming execution
    pub supports_streaming: bool,

    /// Maximum parallelism (number of concurrent tasks)
    pub max_parallelism: usize,

    /// Maximum memory available (bytes)
    pub max_memory_bytes: u64,

    /// Supported distance metrics for vector search
    pub supported_distance_metrics: Vec<String>,

    /// Supported aggregation functions
    pub supported_aggregations: Vec<String>,

    /// Additional provider-specific capabilities
    pub extensions: std::collections::HashMap<String, serde_json::Value>,
}

impl Default for ComputeCapabilities {
    fn default() -> Self {
        Self {
            supports_filter_pushdown: false,
            supports_projection_pushdown: false,
            supports_aggregate_pushdown: false,
            supports_vector_search: false,
            supports_graph_traversal: false,
            supports_full_text_search: false,
            supports_geospatial: false,
            supports_window_functions: false,
            supports_distributed: false,
            supports_streaming: false,
            max_parallelism: 1,
            max_memory_bytes: 0,
            supported_distance_metrics: Vec::new(),
            supported_aggregations: Vec::new(),
            extensions: std::collections::HashMap::new(),
        }
    }
}

impl ComputeCapabilities {
    /// Create capabilities for a local single-node provider
    pub fn local_provider() -> Self {
        Self {
            supports_filter_pushdown: true,
            supports_projection_pushdown: true,
            supports_aggregate_pushdown: false,
            supports_vector_search: true,
            supports_graph_traversal: true,
            supports_full_text_search: false,
            supports_geospatial: true,
            supports_window_functions: true,
            supports_distributed: false,
            supports_streaming: true,
            max_parallelism: num_cpus::get(),
            max_memory_bytes: 8 * 1024 * 1024 * 1024, // 8GB default
            supported_distance_metrics: vec![
                "euclidean".to_string(),
                "cosine".to_string(),
                "dot_product".to_string(),
                "manhattan".to_string(),
            ],
            supported_aggregations: vec![
                "count".to_string(),
                "sum".to_string(),
                "avg".to_string(),
                "min".to_string(),
                "max".to_string(),
            ],
            extensions: std::collections::HashMap::new(),
        }
    }

    /// Create capabilities for a distributed provider
    pub fn distributed_provider(nodes: usize) -> Self {
        Self {
            supports_filter_pushdown: true,
            supports_projection_pushdown: true,
            supports_aggregate_pushdown: true,
            supports_vector_search: true,
            supports_graph_traversal: true,
            supports_full_text_search: true,
            supports_geospatial: true,
            supports_window_functions: true,
            supports_distributed: true,
            supports_streaming: true,
            max_parallelism: nodes * num_cpus::get(),
            max_memory_bytes: nodes as u64 * 16 * 1024 * 1024 * 1024, // 16GB per node
            supported_distance_metrics: vec![
                "euclidean".to_string(),
                "cosine".to_string(),
                "dot_product".to_string(),
                "manhattan".to_string(),
                "hamming".to_string(),
            ],
            supported_aggregations: vec![
                "count".to_string(),
                "sum".to_string(),
                "avg".to_string(),
                "min".to_string(),
                "max".to_string(),
                "stddev".to_string(),
                "variance".to_string(),
                "approx_distinct".to_string(),
            ],
            extensions: std::collections::HashMap::new(),
        }
    }

    /// Check if provider supports all required capabilities for a plan
    pub fn supports_plan(&self, required: &ComputeCapabilities) -> bool {
        // Check boolean capabilities
        if required.supports_filter_pushdown && !self.supports_filter_pushdown {
            return false;
        }
        if required.supports_projection_pushdown && !self.supports_projection_pushdown {
            return false;
        }
        if required.supports_aggregate_pushdown && !self.supports_aggregate_pushdown {
            return false;
        }
        if required.supports_vector_search && !self.supports_vector_search {
            return false;
        }
        if required.supports_graph_traversal && !self.supports_graph_traversal {
            return false;
        }
        if required.supports_distributed && !self.supports_distributed {
            return false;
        }
        true
    }
}

/// Cost estimate for executing a compute plan
///
/// Used by the scheduler to select the most efficient provider
/// for a given query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Estimated CPU cost (relative units)
    pub cpu_cost: f64,

    /// Estimated I/O cost (relative units)
    pub io_cost: f64,

    /// Estimated network cost (relative units, 0 for local)
    pub network_cost: f64,

    /// Estimated memory usage in bytes
    pub memory_bytes: u64,

    /// Estimated number of output rows
    pub estimated_rows: u64,
}

impl CostEstimate {
    /// Create a new cost estimate
    pub fn new(cpu: f64, io: f64, network: f64, memory: u64, rows: u64) -> Self {
        Self {
            cpu_cost: cpu,
            io_cost: io,
            network_cost: network,
            memory_bytes: memory,
            estimated_rows: rows,
        }
    }

    /// Get total cost (simple weighted sum)
    pub fn total_cost(&self) -> f64 {
        // Default weights: CPU=1.0, IO=2.0, Network=5.0
        self.cpu_cost + (2.0 * self.io_cost) + (5.0 * self.network_cost)
    }

    /// Get total cost with custom weights
    pub fn total_cost_weighted(&self, cpu_weight: f64, io_weight: f64, network_weight: f64) -> f64 {
        (cpu_weight * self.cpu_cost) + (io_weight * self.io_cost) + (network_weight * self.network_cost)
    }

    /// Create an unknown/infinite cost estimate
    pub fn unknown() -> Self {
        Self {
            cpu_cost: f64::MAX,
            io_cost: f64::MAX,
            network_cost: f64::MAX,
            memory_bytes: u64::MAX,
            estimated_rows: 0,
        }
    }

    /// Check if this is an unknown cost
    pub fn is_unknown(&self) -> bool {
        self.cpu_cost == f64::MAX || self.io_cost == f64::MAX
    }

    /// Combine two cost estimates (for composed operations)
    pub fn combine(&self, other: &CostEstimate) -> CostEstimate {
        CostEstimate {
            cpu_cost: self.cpu_cost + other.cpu_cost,
            io_cost: self.io_cost + other.io_cost,
            network_cost: self.network_cost + other.network_cost,
            memory_bytes: self.memory_bytes.saturating_add(other.memory_bytes),
            estimated_rows: other.estimated_rows, // Use the final row estimate
        }
    }
}

impl Default for CostEstimate {
    fn default() -> Self {
        Self {
            cpu_cost: 0.0,
            io_cost: 0.0,
            network_cost: 0.0,
            memory_bytes: 0,
            estimated_rows: 0,
        }
    }
}

/// Execution context passed to compute providers
///
/// Contains runtime configuration and resources for query execution.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Query/execution ID for tracking
    pub execution_id: String,

    /// Maximum memory to use (bytes)
    pub memory_limit_bytes: Option<u64>,

    /// Maximum execution time
    pub timeout: Option<Duration>,

    /// Target batch size for results
    pub batch_size: usize,

    /// Enable parallel execution
    pub parallel: bool,

    /// Maximum parallelism
    pub max_parallelism: usize,

    /// Enable caching of intermediate results
    pub enable_caching: bool,

    /// Enable metrics collection
    pub collect_metrics: bool,

    /// Session/tenant ID for multi-tenancy
    pub session_id: Option<String>,

    /// Additional context variables
    pub variables: std::collections::HashMap<String, serde_json::Value>,

    /// Start time for tracking
    pub start_time: DateTime<Utc>,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            execution_id: uuid::Uuid::new_v4().to_string(),
            memory_limit_bytes: None,
            timeout: Some(Duration::from_secs(300)), // 5 minute default
            batch_size: 10000,
            parallel: true,
            max_parallelism: num_cpus::get(),
            enable_caching: true,
            collect_metrics: false,
            session_id: None,
            variables: std::collections::HashMap::new(),
            start_time: Utc::now(),
        }
    }
}

impl ExecutionContext {
    /// Create a new execution context with a specific ID
    pub fn with_id(execution_id: impl Into<String>) -> Self {
        Self {
            execution_id: execution_id.into(),
            ..Default::default()
        }
    }

    /// Set memory limit
    pub fn with_memory_limit(mut self, bytes: u64) -> Self {
        self.memory_limit_bytes = Some(bytes);
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set batch size
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Enable/disable parallelism
    pub fn with_parallel(mut self, parallel: bool) -> Self {
        self.parallel = parallel;
        self
    }

    /// Set maximum parallelism
    pub fn with_max_parallelism(mut self, max: usize) -> Self {
        self.max_parallelism = max;
        self
    }

    /// Enable metrics collection
    pub fn with_metrics(mut self, enabled: bool) -> Self {
        self.collect_metrics = enabled;
        self
    }

    /// Set session ID
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Add a context variable
    pub fn with_variable(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.variables.insert(key.into(), value);
        self
    }

    /// Check if execution has timed out
    pub fn is_timed_out(&self) -> bool {
        if let Some(timeout) = self.timeout {
            let elapsed = Utc::now().signed_duration_since(self.start_time);
            elapsed.to_std().map(|d| d > timeout).unwrap_or(false)
        } else {
            false
        }
    }

    /// Get elapsed time since start
    pub fn elapsed(&self) -> Duration {
        let elapsed = Utc::now().signed_duration_since(self.start_time);
        elapsed.to_std().unwrap_or(Duration::ZERO)
    }
}

/// Metrics collected during execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderMetrics {
    /// Total execution time
    pub total_time_ms: u64,

    /// Time spent in CPU operations
    pub cpu_time_ms: u64,

    /// Time spent in I/O operations
    pub io_time_ms: u64,

    /// Time spent in network operations
    pub network_time_ms: u64,

    /// Peak memory usage in bytes
    pub peak_memory_bytes: u64,

    /// Number of rows processed
    pub rows_processed: u64,

    /// Number of rows output
    pub rows_output: u64,

    /// Number of batches produced
    pub batches_produced: usize,

    /// Number of cache hits
    pub cache_hits: u64,

    /// Number of cache misses
    pub cache_misses: u64,

    /// Number of files scanned
    pub files_scanned: u64,

    /// Bytes scanned
    pub bytes_scanned: u64,

    /// Bytes output
    pub bytes_output: u64,

    /// Number of tasks/partitions used
    pub tasks_used: usize,

    /// Provider-specific metrics
    pub extensions: std::collections::HashMap<String, serde_json::Value>,
}

impl ProviderMetrics {
    /// Create new empty metrics
    pub fn new() -> Self {
        Self::default()
    }

    /// Add extension metric
    pub fn with_extension(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.extensions.insert(key.into(), value);
        self
    }

    /// Merge metrics from another instance
    pub fn merge(&mut self, other: &ProviderMetrics) {
        self.total_time_ms += other.total_time_ms;
        self.cpu_time_ms += other.cpu_time_ms;
        self.io_time_ms += other.io_time_ms;
        self.network_time_ms += other.network_time_ms;
        self.peak_memory_bytes = self.peak_memory_bytes.max(other.peak_memory_bytes);
        self.rows_processed += other.rows_processed;
        self.rows_output += other.rows_output;
        self.batches_produced += other.batches_produced;
        self.cache_hits += other.cache_hits;
        self.cache_misses += other.cache_misses;
        self.files_scanned += other.files_scanned;
        self.bytes_scanned += other.bytes_scanned;
        self.bytes_output += other.bytes_output;
        self.tasks_used += other.tasks_used;
    }
}

/// Result of execution including data and optional metrics
pub struct ExecutionResult {
    /// Stream of result batches
    pub data: RecordBatchStream,

    /// Execution metrics (if requested)
    pub metrics: Option<ProviderMetrics>,
}

impl std::fmt::Debug for ExecutionResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionResult")
            .field("data", &"<RecordBatchStream>")
            .field("metrics", &self.metrics)
            .finish()
    }
}

// ============================================================================
// Core Trait
// ============================================================================

/// Pluggable compute engine interface
///
/// This trait defines the contract for compute providers in the storage-compute
/// separation architecture. Providers execute compute plans and return results
/// as Arrow RecordBatch streams.
///
/// ## Implementation Guidelines
///
/// 1. **Thread Safety**: Implementations must be `Send + Sync` for concurrent access.
///
/// 2. **Cost Estimation**: `estimate_cost` should return accurate estimates for
///    the scheduler to make optimal provider selections.
///
/// 3. **Capability Declaration**: `capabilities()` must accurately reflect what
///    operations the provider supports.
///
/// 4. **Error Handling**: Use `anyhow::Result` for error propagation.
///
/// ## Example
///
/// ```rust,ignore
/// use async_trait::async_trait;
/// use proximadb::compute::provider::{ComputeProvider, ComputeCapabilities};
///
/// struct MyProvider;
///
/// #[async_trait]
/// impl ComputeProvider for MyProvider {
///     fn provider_name(&self) -> &str {
///         "my_provider"
///     }
///
///     async fn execute(
///         &self,
///         plan: &ComputePlan,
///         ctx: &ExecutionContext
///     ) -> Result<ExecutionResult> {
///         // Execute the plan...
///     }
///
///     fn can_execute(&self, plan: &ComputePlan) -> bool {
///         // Check if all plan operations are supported...
///         true
///     }
///
///     fn estimate_cost(&self, plan: &ComputePlan) -> Result<CostEstimate> {
///         // Estimate execution cost...
///         Ok(CostEstimate::default())
///     }
///
///     fn capabilities(&self) -> ComputeCapabilities {
///         ComputeCapabilities::local_provider()
///     }
/// }
/// ```
#[async_trait]
pub trait ComputeProvider: Send + Sync + Debug {
    /// Get the provider name (e.g., "local", "spark", "duckdb")
    fn provider_name(&self) -> &str;

    /// Get the provider version
    fn provider_version(&self) -> &str {
        "1.0.0"
    }

    /// Execute a compute plan and return results
    ///
    /// # Arguments
    /// * `plan` - The compute plan to execute
    /// * `ctx` - Execution context with runtime configuration
    ///
    /// # Returns
    /// Stream of Arrow RecordBatches containing the results
    async fn execute(
        &self,
        plan: &ComputePlan,
        ctx: &ExecutionContext,
    ) -> Result<ExecutionResult>;

    /// Check if this provider can execute the given plan
    ///
    /// Returns `true` if the provider supports all operations in the plan.
    /// This is a fast check used by the scheduler before cost estimation.
    fn can_execute(&self, plan: &ComputePlan) -> bool;

    /// Estimate the cost of executing a plan
    ///
    /// # Arguments
    /// * `plan` - The compute plan to estimate
    ///
    /// # Returns
    /// Cost estimate used by the scheduler for provider selection
    fn estimate_cost(&self, plan: &ComputePlan) -> Result<CostEstimate>;

    /// Get the capabilities of this provider
    fn capabilities(&self) -> ComputeCapabilities;

    /// Validate a plan before execution
    ///
    /// Performs deeper validation than `can_execute`, checking for:
    /// - Schema compatibility
    /// - Resource availability
    /// - Plan correctness
    fn validate_plan(&self, plan: &ComputePlan) -> Result<()> {
        if !self.can_execute(plan) {
            anyhow::bail!("Provider '{}' cannot execute this plan", self.provider_name());
        }
        Ok(())
    }

    /// Cancel an ongoing execution
    ///
    /// # Arguments
    /// * `execution_id` - The ID of the execution to cancel
    ///
    /// # Returns
    /// `true` if cancellation was successful
    async fn cancel(&self, execution_id: &str) -> Result<bool> {
        // Default implementation does nothing
        let _ = execution_id;
        Ok(false)
    }

    /// Get current resource usage
    fn resource_usage(&self) -> Result<ResourceUsage> {
        Ok(ResourceUsage::default())
    }

    /// Shutdown the provider and release resources
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

/// Resource usage information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceUsage {
    /// Currently used memory in bytes
    pub memory_used_bytes: u64,

    /// Currently active executions
    pub active_executions: usize,

    /// Total executions since start
    pub total_executions: u64,

    /// CPU utilization (0.0 to 1.0)
    pub cpu_utilization: f64,

    /// Active threads/tasks
    pub active_threads: usize,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_capabilities_local() {
        let caps = ComputeCapabilities::local_provider();
        assert!(caps.supports_filter_pushdown);
        assert!(caps.supports_vector_search);
        assert!(!caps.supports_distributed);
        assert!(caps.max_parallelism > 0);
    }

    #[test]
    fn test_compute_capabilities_distributed() {
        let caps = ComputeCapabilities::distributed_provider(4);
        assert!(caps.supports_distributed);
        assert!(caps.supports_aggregate_pushdown);
        assert_eq!(caps.max_parallelism, 4 * num_cpus::get());
    }

    #[test]
    fn test_cost_estimate_total() {
        let cost = CostEstimate::new(100.0, 50.0, 10.0, 1024, 1000);
        // Total = 100 + (2*50) + (5*10) = 100 + 100 + 50 = 250
        assert_eq!(cost.total_cost(), 250.0);
    }

    #[test]
    fn test_cost_estimate_combine() {
        let cost1 = CostEstimate::new(100.0, 50.0, 0.0, 1024, 1000);
        let cost2 = CostEstimate::new(50.0, 25.0, 10.0, 512, 500);

        let combined = cost1.combine(&cost2);
        assert_eq!(combined.cpu_cost, 150.0);
        assert_eq!(combined.io_cost, 75.0);
        assert_eq!(combined.network_cost, 10.0);
        assert_eq!(combined.memory_bytes, 1536);
        assert_eq!(combined.estimated_rows, 500);
    }

    #[test]
    fn test_execution_context_timeout() {
        let ctx = ExecutionContext::default()
            .with_timeout(Duration::from_millis(1));

        // Sleep a bit
        std::thread::sleep(Duration::from_millis(10));

        assert!(ctx.is_timed_out());
    }

    #[test]
    fn test_execution_context_builder() {
        let ctx = ExecutionContext::with_id("test-123")
            .with_memory_limit(1024)
            .with_batch_size(100)
            .with_parallel(false)
            .with_session("session-456");

        assert_eq!(ctx.execution_id, "test-123");
        assert_eq!(ctx.memory_limit_bytes, Some(1024));
        assert_eq!(ctx.batch_size, 100);
        assert!(!ctx.parallel);
        assert_eq!(ctx.session_id, Some("session-456".to_string()));
    }

    #[test]
    fn test_provider_metrics_merge() {
        let mut m1 = ProviderMetrics {
            rows_processed: 100,
            peak_memory_bytes: 1000,
            ..Default::default()
        };

        let m2 = ProviderMetrics {
            rows_processed: 200,
            peak_memory_bytes: 1500,
            ..Default::default()
        };

        m1.merge(&m2);
        assert_eq!(m1.rows_processed, 300);
        assert_eq!(m1.peak_memory_bytes, 1500); // Max, not sum
    }
}
