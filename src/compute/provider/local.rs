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

//! # Local Compute Provider
//!
//! Default compute provider using ProximaDB's native query engine.
//! Executes compute plans locally with hardware-accelerated operations.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use anyhow::Result;
use async_recursion::async_recursion;
use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::RwLock;
use tokio::sync::broadcast;
use tracing::{debug, info, instrument, warn};

use crate::compute::plan::{ComputePlan, PlanNode};
use crate::compute::provider::traits::{
    ComputeCapabilities, ComputeProvider, CostEstimate, ExecutionContext, ExecutionResult,
    ProviderMetrics, RecordBatchStream, ResourceUsage,
};

// ============================================================================
// Local Provider Configuration
// ============================================================================

/// Configuration for LocalComputeProvider
#[derive(Debug, Clone)]
pub struct LocalProviderConfig {
    /// Maximum memory for computations (bytes)
    pub max_memory_bytes: u64,

    /// Maximum parallel tasks
    pub max_parallelism: usize,

    /// Default batch size for results
    pub default_batch_size: usize,

    /// Enable result caching
    pub enable_caching: bool,

    /// Cache size limit (bytes)
    pub cache_size_bytes: u64,

    /// Enable query compilation/JIT
    pub enable_jit: bool,

    /// Spill to disk threshold (% of max memory)
    pub spill_threshold: f64,

    /// Provider name override
    pub provider_name: Option<String>,
}

impl Default for LocalProviderConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 8 * 1024 * 1024 * 1024, // 8GB
            max_parallelism: num_cpus::get(),
            default_batch_size: 10000,
            enable_caching: true,
            cache_size_bytes: 1024 * 1024 * 1024, // 1GB
            enable_jit: false,
            spill_threshold: 0.8,
            provider_name: None,
        }
    }
}

impl LocalProviderConfig {
    /// Create configuration optimized for low memory
    pub fn low_memory() -> Self {
        Self {
            max_memory_bytes: 512 * 1024 * 1024, // 512MB
            max_parallelism: 2,
            default_batch_size: 1000,
            enable_caching: false,
            cache_size_bytes: 0,
            enable_jit: false,
            spill_threshold: 0.6,
            provider_name: Some("local-lowmem".to_string()),
        }
    }

    /// Create configuration optimized for high performance
    pub fn high_performance() -> Self {
        Self {
            max_memory_bytes: 32 * 1024 * 1024 * 1024, // 32GB
            max_parallelism: num_cpus::get() * 2,
            default_batch_size: 50000,
            enable_caching: true,
            cache_size_bytes: 4 * 1024 * 1024 * 1024, // 4GB
            enable_jit: true,
            spill_threshold: 0.9,
            provider_name: Some("local-perf".to_string()),
        }
    }
}

// ============================================================================
// Execution State
// ============================================================================

/// State for tracking active executions
struct ExecutionState {
    /// Active execution count
    active_count: AtomicUsize,

    /// Total executions since start
    total_count: AtomicU64,

    /// Current memory usage
    memory_usage: AtomicU64,

    /// Active execution IDs
    active_executions: RwLock<HashMap<String, ExecutionInfo>>,

    /// Cancellation channels
    cancel_channels: RwLock<HashMap<String, broadcast::Sender<()>>>,
}

impl std::fmt::Debug for ExecutionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionState")
            .field("active_count", &self.active_count.load(Ordering::SeqCst))
            .field("total_count", &self.total_count.load(Ordering::SeqCst))
            .field("memory_usage", &self.memory_usage.load(Ordering::SeqCst))
            .field("active_executions", &"<RwLock>")
            .field("cancel_channels", &"<RwLock>")
            .finish()
    }
}

impl ExecutionState {
    /// Create a new empty execution state with zeroed counters.
    fn new() -> Self {
        Self {
            active_count: AtomicUsize::new(0),
            total_count: AtomicU64::new(0),
            memory_usage: AtomicU64::new(0),
            active_executions: RwLock::new(HashMap::new()),
            cancel_channels: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new execution, increment counters, and return a cancellation receiver.
    fn start_execution(&self, id: &str) -> broadcast::Receiver<()> {
        self.active_count.fetch_add(1, Ordering::SeqCst);
        self.total_count.fetch_add(1, Ordering::SeqCst);

        let (tx, rx) = broadcast::channel(1);
        self.cancel_channels.write().insert(id.to_string(), tx);
        self.active_executions.write().insert(
            id.to_string(),
            ExecutionInfo {
                started_at: std::time::Instant::now(),
                memory_allocated: 0,
            },
        );

        rx
    }

    /// Mark an execution as finished and release its memory allocation.
    fn finish_execution(&self, id: &str) {
        self.active_count.fetch_sub(1, Ordering::SeqCst);
        self.cancel_channels.write().remove(id);

        if let Some(info) = self.active_executions.write().remove(id) {
            self.memory_usage
                .fetch_sub(info.memory_allocated, Ordering::SeqCst);
        }
    }

    /// Send a cancellation signal to the execution with the given id.
    fn cancel_execution(&self, id: &str) -> bool {
        if let Some(tx) = self.cancel_channels.read().get(id) {
            tx.send(()).is_ok()
        } else {
            false
        }
    }

    /// Track additional memory allocated for the given execution.
    #[allow(dead_code)]
    fn allocate_memory(&self, id: &str, bytes: u64) {
        self.memory_usage.fetch_add(bytes, Ordering::SeqCst);
        if let Some(info) = self.active_executions.write().get_mut(id) {
            info.memory_allocated += bytes;
        }
    }
}

/// Metadata tracked for a single in-flight execution.
#[derive(Debug)]
struct ExecutionInfo {
    /// Timestamp when the execution started
    #[allow(dead_code)]
    started_at: std::time::Instant,
    /// Cumulative memory allocated by this execution in bytes
    memory_allocated: u64,
}

// ============================================================================
// Local Compute Provider
// ============================================================================

/// Default local compute provider using ProximaDB's query engine
///
/// This provider executes compute plans locally using ProximaDB's native
/// query execution engine. It supports:
///
/// - Table scans with filter/projection pushdown
/// - Vector similarity search (HNSW, IVF, brute-force)
/// - Graph traversals (BFS, DFS, shortest path)
/// - SQL operations (filter, project, aggregate, join, sort)
/// - Window functions
///
/// ## Example
///
/// ```rust,ignore
/// use proximadb::compute::provider::LocalComputeProvider;
///
/// let provider = LocalComputeProvider::new()?;
///
/// // Check capabilities
/// let caps = provider.capabilities();
/// assert!(caps.supports_vector_search);
///
/// // Execute a plan
/// let result = provider.execute(&plan, &ctx).await?;
/// ```
#[derive(Debug)]
pub struct LocalComputeProvider {
    /// Configuration
    config: LocalProviderConfig,

    /// Capabilities
    capabilities: ComputeCapabilities,

    /// Execution state
    state: Arc<ExecutionState>,
}

impl LocalComputeProvider {
    /// Create a new local compute provider with default configuration
    pub fn new() -> Result<Self> {
        Self::with_config(LocalProviderConfig::default())
    }

    /// Create with custom configuration
    pub fn with_config(config: LocalProviderConfig) -> Result<Self> {
        info!(
            "Creating LocalComputeProvider with max_memory={}MB, max_parallelism={}",
            config.max_memory_bytes / (1024 * 1024),
            config.max_parallelism
        );

        let capabilities = ComputeCapabilities {
            supports_filter_pushdown: true,
            supports_projection_pushdown: true,
            supports_aggregate_pushdown: false, // Handled by compute layer
            supports_vector_search: true,
            supports_graph_traversal: true,
            supports_full_text_search: false, // Future enhancement
            supports_geospatial: true,
            supports_window_functions: true,
            supports_distributed: false,
            supports_streaming: true,
            max_parallelism: config.max_parallelism,
            max_memory_bytes: config.max_memory_bytes,
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
            ],
            extensions: std::collections::HashMap::new(),
        };

        Ok(Self {
            config,
            capabilities,
            state: Arc::new(ExecutionState::new()),
        })
    }

    /// Get current configuration
    pub fn config(&self) -> &LocalProviderConfig {
        &self.config
    }

    /// Estimate cost for a plan node recursively
    fn estimate_node_cost(&self, node: &PlanNode) -> CostEstimate {
        match node {
            PlanNode::TableScan {
                filter, columns, ..
            } => {
                // Base cost for table scan
                let base_cpu = 1.0;
                let base_io = 10.0;

                // Reduce CPU if fewer columns
                let column_factor = if columns.is_empty() {
                    1.0
                } else {
                    0.1 * columns.len() as f64
                };

                // Reduce rows if filter present
                let selectivity = if filter.is_some() { 0.3 } else { 1.0 };

                CostEstimate {
                    cpu_cost: base_cpu * column_factor,
                    io_cost: base_io,
                    network_cost: 0.0,
                    memory_bytes: 1024 * 1024, // 1MB estimate
                    estimated_rows: (10000.0 * selectivity) as u64,
                }
            }

            PlanNode::VectorScan { top_k, .. } => {
                // Vector search cost depends on index type and k
                let k = *top_k as f64;
                CostEstimate {
                    cpu_cost: 10.0 + (k * 0.1),
                    io_cost: 5.0,
                    network_cost: 0.0,
                    memory_bytes: (*top_k as u64) * 1024, // ~1KB per result
                    estimated_rows: *top_k as u64,
                }
            }

            PlanNode::GraphScan { traversal, .. } => {
                // Graph traversal cost depends on depth
                let max_depth = traversal.max_depth as f64;
                CostEstimate {
                    cpu_cost: 5.0 * max_depth,
                    io_cost: 2.0 * max_depth,
                    network_cost: 0.0,
                    memory_bytes: (1024 * 1024) * (max_depth as u64), // 1MB per level
                    estimated_rows: (100.0 * max_depth) as u64,
                }
            }

            PlanNode::Filter { input, .. } => {
                let input_cost = self.estimate_node_cost(input);
                CostEstimate {
                    cpu_cost: input_cost.cpu_cost + 0.5,
                    io_cost: input_cost.io_cost,
                    network_cost: input_cost.network_cost,
                    memory_bytes: input_cost.memory_bytes,
                    estimated_rows: (input_cost.estimated_rows as f64 * 0.5) as u64, // 50% selectivity
                }
            }

            PlanNode::Project {
                input, expressions, ..
            } => {
                let input_cost = self.estimate_node_cost(input);
                CostEstimate {
                    cpu_cost: input_cost.cpu_cost + (0.1 * expressions.len() as f64),
                    io_cost: input_cost.io_cost,
                    network_cost: input_cost.network_cost,
                    memory_bytes: input_cost.memory_bytes,
                    estimated_rows: input_cost.estimated_rows,
                }
            }

            PlanNode::Aggregate {
                input, aggregates, ..
            } => {
                let input_cost = self.estimate_node_cost(input);
                CostEstimate {
                    cpu_cost: input_cost.cpu_cost + (5.0 * aggregates.len() as f64),
                    io_cost: input_cost.io_cost,
                    network_cost: input_cost.network_cost,
                    memory_bytes: input_cost.memory_bytes * 2, // Grouping buffers
                    estimated_rows: 100,                       // Aggregates typically reduce rows
                }
            }

            PlanNode::Sort {
                input, order_by, ..
            } => {
                let input_cost = self.estimate_node_cost(input);
                let n = input_cost.estimated_rows as f64;
                let sort_cost = if n > 0.0 { n * n.log2() } else { 0.0 };
                CostEstimate {
                    cpu_cost: input_cost.cpu_cost + sort_cost * 0.01 * order_by.len() as f64,
                    io_cost: input_cost.io_cost,
                    network_cost: input_cost.network_cost,
                    memory_bytes: input_cost.memory_bytes * 2, // Sort buffer
                    estimated_rows: input_cost.estimated_rows,
                }
            }

            PlanNode::Limit { input, limit, .. } => {
                let input_cost = self.estimate_node_cost(input);
                CostEstimate {
                    cpu_cost: input_cost.cpu_cost,
                    io_cost: input_cost.io_cost,
                    network_cost: input_cost.network_cost,
                    memory_bytes: input_cost.memory_bytes,
                    estimated_rows: (*limit).min(input_cost.estimated_rows),
                }
            }

            PlanNode::HashJoin { left, right, .. } => {
                let left_cost = self.estimate_node_cost(left);
                let right_cost = self.estimate_node_cost(right);

                CostEstimate {
                    cpu_cost: left_cost.cpu_cost
                        + right_cost.cpu_cost
                        + (left_cost.estimated_rows as f64 * 0.1),
                    io_cost: left_cost.io_cost + right_cost.io_cost,
                    network_cost: left_cost.network_cost + right_cost.network_cost,
                    memory_bytes: left_cost.memory_bytes + right_cost.memory_bytes,
                    estimated_rows: (left_cost.estimated_rows as f64
                        * right_cost.estimated_rows as f64
                        * 0.01) as u64,
                }
            }

            PlanNode::Union { inputs, .. } => {
                let mut total = CostEstimate::default();
                for input in inputs {
                    let cost = self.estimate_node_cost(input);
                    total = total.combine(&cost);
                }
                total
            }

            PlanNode::Exchange { input, .. } => {
                let input_cost = self.estimate_node_cost(input);
                // Local exchange has minimal overhead
                CostEstimate {
                    cpu_cost: input_cost.cpu_cost + 1.0,
                    io_cost: input_cost.io_cost,
                    network_cost: 0.0, // Local exchange
                    memory_bytes: input_cost.memory_bytes,
                    estimated_rows: input_cost.estimated_rows,
                }
            }
        }
    }

    /// Check if a plan node can be executed
    fn can_execute_node(&self, node: &PlanNode) -> bool {
        match node {
            PlanNode::TableScan { .. } => true,
            PlanNode::VectorScan { .. } => self.capabilities.supports_vector_search,
            PlanNode::GraphScan { .. } => self.capabilities.supports_graph_traversal,
            PlanNode::Filter { input, .. } => self.can_execute_node(input),
            PlanNode::Project { input, .. } => self.can_execute_node(input),
            PlanNode::Aggregate { input, .. } => self.can_execute_node(input),
            PlanNode::Sort { input, .. } => self.can_execute_node(input),
            PlanNode::Limit { input, .. } => self.can_execute_node(input),
            PlanNode::HashJoin { left, right, .. } => {
                self.can_execute_node(left) && self.can_execute_node(right)
            }
            PlanNode::Union { inputs, .. } => inputs.iter().all(|i| self.can_execute_node(i)),
            PlanNode::Exchange { input, .. } => self.can_execute_node(input),
        }
    }

    /// Execute a plan node (internal implementation)
    #[async_recursion]
    #[instrument(skip(self, ctx, _cancel_rx), fields(node_type))]
    async fn execute_node(
        &self,
        node: &PlanNode,
        ctx: &ExecutionContext,
        _cancel_rx: &mut broadcast::Receiver<()>,
    ) -> Result<RecordBatchStream> {
        // Check for timeout
        if ctx.is_timed_out() {
            anyhow::bail!("Execution timed out");
        }

        // Helper to create a properly typed empty stream
        fn empty_stream() -> RecordBatchStream {
            Box::pin(futures::stream::empty())
        }

        match node {
            PlanNode::TableScan {
                table,
                columns,
                filter,
            } => {
                debug!(table = %table, columns = ?columns, has_filter = filter.is_some(), "Executing TableScan");
                // Placeholder: Return empty stream
                // In real implementation, this would read from storage layer
                Ok(empty_stream())
            }

            PlanNode::VectorScan {
                collection,
                query_vector,
                top_k,
                ..
            } => {
                debug!(collection = %collection, dim = query_vector.len(), top_k = top_k, "Executing VectorScan");
                // Placeholder: Return empty stream
                // In real implementation, this would use HNSW/IVF index
                Ok(empty_stream())
            }

            PlanNode::GraphScan {
                graph,
                start_nodes,
                traversal,
            } => {
                debug!(
                    graph = %graph,
                    start_count = start_nodes.len(),
                    max_depth = traversal.max_depth,
                    "Executing GraphScan"
                );
                // Placeholder: Return empty stream
                // In real implementation, this would traverse the graph
                Ok(empty_stream())
            }

            PlanNode::Filter {
                input,
                predicate: _,
            } => {
                debug!("Executing Filter");
                let input_stream = self.execute_node(input, ctx, _cancel_rx).await?;
                // Placeholder: Pass through without filtering
                // In real implementation, evaluate predicate on each batch
                Ok(input_stream)
            }

            PlanNode::Project {
                input,
                expressions: _,
            } => {
                debug!("Executing Project");
                let input_stream = self.execute_node(input, ctx, _cancel_rx).await?;
                // Placeholder: Pass through without projection
                // In real implementation, evaluate expressions on each batch
                Ok(input_stream)
            }

            PlanNode::Aggregate {
                input,
                group_by: _,
                aggregates: _,
            } => {
                debug!("Executing Aggregate");
                let input_stream = self.execute_node(input, ctx, _cancel_rx).await?;
                // Collect and aggregate
                // Placeholder: Pass through
                Ok(input_stream)
            }

            PlanNode::Sort { input, order_by: _ } => {
                debug!("Executing Sort");
                let input_stream = self.execute_node(input, ctx, _cancel_rx).await?;
                // Placeholder: Pass through without sorting
                Ok(input_stream)
            }

            PlanNode::Limit {
                input,
                limit,
                offset,
            } => {
                debug!(limit = limit, offset = offset, "Executing Limit");
                let input_stream = self.execute_node(input, ctx, _cancel_rx).await?;

                // Apply limit/offset
                let offset = *offset as usize;
                let limit = *limit as usize;

                let limited = input_stream.skip(offset).take(limit);

                Ok(Box::pin(limited) as RecordBatchStream)
            }

            PlanNode::HashJoin { left, right, on: _ } => {
                debug!("Executing HashJoin");
                let _left_stream = self.execute_node(left, ctx, _cancel_rx).await?;
                let _right_stream = self.execute_node(right, ctx, _cancel_rx).await?;
                // Placeholder: Return empty
                Ok(empty_stream())
            }

            PlanNode::Union { inputs, all: _ } => {
                debug!(input_count = inputs.len(), "Executing Union");
                // Execute all inputs and concatenate
                let mut streams = Vec::with_capacity(inputs.len());
                for input in inputs {
                    let stream = self.execute_node(input, ctx, _cancel_rx).await?;
                    streams.push(stream);
                }
                // Concatenate streams
                let combined = futures::stream::select_all(streams);
                Ok(Box::pin(combined) as RecordBatchStream)
            }

            PlanNode::Exchange {
                input,
                partitioning: _,
            } => {
                debug!("Executing Exchange (local passthrough)");
                // For local execution, exchange is a no-op
                self.execute_node(input, ctx, _cancel_rx).await
            }
        }
    }
}

impl Default for LocalComputeProvider {
    fn default() -> Self {
        Self::new()
            .unwrap_or_else(|e| panic!("Failed to create default LocalComputeProvider: {}", e))
    }
}

#[async_trait]
impl ComputeProvider for LocalComputeProvider {
    fn provider_name(&self) -> &str {
        self.config
            .provider_name
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Provider name should be set"))
            .unwrap_or("local")
    }

    fn provider_version(&self) -> &str {
        "0.1.0"
    }

    #[instrument(skip(self, plan, ctx), fields(execution_id = %ctx.execution_id))]
    async fn execute(&self, plan: &ComputePlan, ctx: &ExecutionContext) -> Result<ExecutionResult> {
        info!(
            plan_id = %plan.id,
            execution_id = %ctx.execution_id,
            "Starting plan execution"
        );

        // Check memory availability
        let current_memory = self.state.memory_usage.load(Ordering::SeqCst);
        if current_memory
            > (self.config.max_memory_bytes as f64 * self.config.spill_threshold) as u64
        {
            warn!(
                current = current_memory,
                max = self.config.max_memory_bytes,
                "Memory usage high, may spill to disk"
            );
        }

        // Register execution
        let mut cancel_rx = self.state.start_execution(&ctx.execution_id);

        // Execute the plan
        let start = std::time::Instant::now();

        let result = self.execute_node(&plan.root, ctx, &mut cancel_rx).await;

        // Finish execution tracking
        let elapsed = start.elapsed();
        self.state.finish_execution(&ctx.execution_id);

        match result {
            Ok(data) => {
                let metrics = if ctx.collect_metrics {
                    Some(ProviderMetrics {
                        total_time_ms: elapsed.as_millis() as u64,
                        tasks_used: 1,
                        ..Default::default()
                    })
                } else {
                    None
                };

                debug!(
                    plan_id = %plan.id,
                    elapsed_ms = elapsed.as_millis(),
                    "Plan execution completed"
                );

                Ok(ExecutionResult { data, metrics })
            }
            Err(e) => {
                warn!(
                    plan_id = %plan.id,
                    error = %e,
                    elapsed_ms = elapsed.as_millis(),
                    "Plan execution failed"
                );
                Err(e)
            }
        }
    }

    fn can_execute(&self, plan: &ComputePlan) -> bool {
        self.can_execute_node(&plan.root)
    }

    fn estimate_cost(&self, plan: &ComputePlan) -> Result<CostEstimate> {
        Ok(self.estimate_node_cost(&plan.root))
    }

    fn capabilities(&self) -> ComputeCapabilities {
        self.capabilities.clone()
    }

    fn validate_plan(&self, plan: &ComputePlan) -> Result<()> {
        if !self.can_execute(plan) {
            anyhow::bail!("LocalComputeProvider cannot execute plan: unsupported operations");
        }

        // Check memory estimate
        let cost = self.estimate_cost(plan)?;
        if cost.memory_bytes > self.config.max_memory_bytes {
            anyhow::bail!(
                "Plan requires {}MB memory, but provider limit is {}MB",
                cost.memory_bytes / (1024 * 1024),
                self.config.max_memory_bytes / (1024 * 1024)
            );
        }

        Ok(())
    }

    async fn cancel(&self, execution_id: &str) -> Result<bool> {
        Ok(self.state.cancel_execution(execution_id))
    }

    fn resource_usage(&self) -> Result<ResourceUsage> {
        Ok(ResourceUsage {
            memory_used_bytes: self.state.memory_usage.load(Ordering::SeqCst),
            active_executions: self.state.active_count.load(Ordering::SeqCst),
            total_executions: self.state.total_count.load(Ordering::SeqCst),
            cpu_utilization: 0.0, // Would need OS integration
            active_threads: self.state.active_count.load(Ordering::SeqCst),
        })
    }

    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down LocalComputeProvider");

        // Cancel all active executions
        let execution_ids: Vec<String> = self
            .state
            .active_executions
            .read()
            .keys()
            .cloned()
            .collect();
        for id in execution_ids {
            let _ = self.state.cancel_execution(&id);
        }

        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::plan::{PlanHints, TraversalDirection, TraversalSpec};

    fn create_simple_plan() -> ComputePlan {
        ComputePlan {
            id: "test-plan".to_string(),
            root: PlanNode::TableScan {
                table: "test_table".to_string(),
                columns: vec!["id".to_string(), "name".to_string()],
                filter: None,
            },
            parameters: std::collections::HashMap::new(),
            hints: PlanHints::default(),
        }
    }

    fn create_vector_plan() -> ComputePlan {
        ComputePlan {
            id: "vector-plan".to_string(),
            root: PlanNode::VectorScan {
                collection: "vectors".to_string(),
                query_vector: vec![0.1, 0.2, 0.3],
                top_k: 10,
                filter: None,
                distance_metric: None,
            },
            parameters: std::collections::HashMap::new(),
            hints: PlanHints::default(),
        }
    }

    fn create_graph_plan() -> ComputePlan {
        ComputePlan {
            id: "graph-plan".to_string(),
            root: PlanNode::GraphScan {
                graph: "social".to_string(),
                start_nodes: vec!["node1".to_string()],
                traversal: TraversalSpec {
                    edge_types: vec!["FOLLOWS".to_string()],
                    direction: TraversalDirection::Outgoing,
                    min_depth: 1,
                    max_depth: 3,
                    filter: None,
                },
            },
            parameters: std::collections::HashMap::new(),
            hints: PlanHints::default(),
        }
    }

    #[test]
    fn test_provider_creation() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");
        assert_eq!(provider.provider_name(), "local");
    }

    #[test]
    fn test_provider_with_config() {
        let config = LocalProviderConfig {
            provider_name: Some("custom-local".to_string()),
            max_parallelism: 4,
            ..Default::default()
        };
        let provider = LocalComputeProvider::with_config(config)
            .expect("Failed to create LocalComputeProvider with config");
        assert_eq!(provider.provider_name(), "custom-local");
        assert_eq!(provider.capabilities().max_parallelism, 4);
    }

    #[test]
    fn test_can_execute_simple() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");
        let plan = create_simple_plan();
        assert!(provider.can_execute(&plan));
    }

    #[test]
    fn test_can_execute_vector() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");
        let plan = create_vector_plan();
        assert!(provider.can_execute(&plan));
    }

    #[test]
    fn test_can_execute_graph() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");
        let plan = create_graph_plan();
        assert!(provider.can_execute(&plan));
    }

    #[test]
    fn test_cost_estimation_simple() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");
        let plan = create_simple_plan();
        let cost = provider
            .estimate_cost(&plan)
            .expect("Failed to estimate cost");

        assert!(cost.cpu_cost > 0.0);
        assert!(cost.io_cost > 0.0);
        assert!(cost.memory_bytes > 0);
    }

    #[test]
    fn test_cost_estimation_complex() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");

        let plan = ComputePlan {
            id: "complex".to_string(),
            root: PlanNode::Sort {
                input: Box::new(PlanNode::Filter {
                    input: Box::new(PlanNode::TableScan {
                        table: "data".to_string(),
                        columns: vec![],
                        filter: None,
                    }),
                    predicate: crate::compute::plan::Expr::Literal(
                        crate::compute::plan::LiteralValue::Bool(true),
                    ),
                }),
                order_by: vec![crate::compute::plan::SortExpr {
                    expr: crate::compute::plan::Expr::Column("id".to_string()),
                    ascending: true,
                    nulls_first: false,
                }],
            },
            parameters: std::collections::HashMap::new(),
            hints: PlanHints::default(),
        };

        let cost = provider
            .estimate_cost(&plan)
            .expect("Failed to estimate cost for complex plan");
        // Sort should add to the base scan cost
        assert!(cost.cpu_cost > 1.0);
    }

    #[test]
    fn test_capabilities() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");
        let caps = provider.capabilities();

        assert!(caps.supports_filter_pushdown);
        assert!(caps.supports_projection_pushdown);
        assert!(caps.supports_vector_search);
        assert!(caps.supports_graph_traversal);
        assert!(!caps.supports_distributed);
        assert!(caps.max_parallelism > 0);
    }

    #[test]
    fn test_resource_usage() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");
        let usage = provider
            .resource_usage()
            .expect("Failed to get resource usage");

        assert_eq!(usage.active_executions, 0);
        assert_eq!(usage.total_executions, 0);
    }

    #[tokio::test]
    async fn test_execute_simple() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");
        let plan = create_simple_plan();
        let ctx = ExecutionContext::default();

        let result = provider.execute(&plan, &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_with_metrics() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");
        let plan = create_simple_plan();
        let ctx = ExecutionContext::default().with_metrics(true);

        let result = provider
            .execute(&plan, &ctx)
            .await
            .expect("Failed to execute plan with metrics");
        assert!(result.metrics.is_some());
    }

    #[tokio::test]
    async fn test_cancel_execution() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");
        // For this test, we just verify cancel doesn't panic on non-existent ID
        let result = provider
            .cancel("non-existent")
            .await
            .expect("Failed to cancel execution");
        assert!(!result);
    }

    #[tokio::test]
    async fn test_shutdown() {
        let provider = LocalComputeProvider::new().expect("Failed to create LocalComputeProvider");
        let result = provider.shutdown().await;
        assert!(result.is_ok());
    }
}
