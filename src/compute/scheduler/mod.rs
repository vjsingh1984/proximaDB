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

//! # Compute Scheduler Module
//!
//! The compute scheduler is responsible for selecting the optimal compute provider
//! for each query plan and managing execution across providers.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                         COMPUTE SCHEDULER                                    │
//! │                                                                              │
//! │  ┌──────────────────┐    ┌──────────────────┐    ┌──────────────────┐       │
//! │  │   ComputePlan    │───▶│  Provider        │───▶│  Execution       │       │
//! │  │   (input)        │    │  Selection       │    │  Result          │       │
//! │  └──────────────────┘    └──────────────────┘    └──────────────────┘       │
//! │                               │                                              │
//! │                               ▼                                              │
//! │                    ┌─────────────────────┐                                   │
//! │                    │   Scheduling Policy  │                                  │
//! │                    │  ┌───────────────┐   │                                  │
//! │                    │  │ CostBased     │   │                                  │
//! │                    │  │ RoundRobin    │   │                                  │
//! │                    │  │ CapabilityFirst│  │                                  │
//! │                    │  │ LoadBalanced  │   │                                  │
//! │                    │  └───────────────┘   │                                  │
//! │                    └─────────────────────┘                                   │
//! │                               │                                              │
//! │          ┌───────────────────┼───────────────────┐                          │
//! │          ▼                   ▼                   ▼                          │
//! │  ┌───────────────┐   ┌───────────────┐   ┌───────────────┐                  │
//! │  │    Local      │   │    Spark      │   │   DuckDB      │                  │
//! │  │   Provider    │   │   Provider    │   │   Provider    │                  │
//! │  └───────────────┘   └───────────────┘   └───────────────┘                  │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Scheduling Policies
//!
//! - **CostBased**: Select provider with lowest estimated cost
//! - **RoundRobin**: Distribute queries evenly across providers
//! - **CapabilityFirst**: Prioritize providers with exact capability match
//! - **LoadBalanced**: Consider current provider load in selection
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::compute::scheduler::{ComputeScheduler, SchedulingPolicy};
//! use proximadb::compute::provider::LocalComputeProvider;
//!
//! // Create scheduler with default provider
//! let local = Arc::new(LocalComputeProvider::new()?);
//! let scheduler = ComputeScheduler::builder()
//!     .default_provider(local.clone())
//!     .add_provider(local)
//!     .policy(SchedulingPolicy::CostBased)
//!     .build()?;
//!
//! // Schedule and execute a plan
//! let result = scheduler.schedule(plan).await?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use anyhow::{Result, bail};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use crate::compute::plan::ComputePlan;
use crate::compute::provider::traits::{
    ComputeCapabilities, ComputeProvider, CostEstimate, ExecutionContext, RecordBatchStream,
};

// ============================================================================
// Scheduling Policy
// ============================================================================

/// Policy for selecting compute providers
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub enum SchedulingPolicy {
    /// Select provider with lowest estimated cost
    #[default]
    CostBased,

    /// Distribute queries round-robin across providers
    RoundRobin,

    /// Prioritize providers with exact capability match
    CapabilityFirst,

    /// Consider current load when selecting provider
    LoadBalanced,

    /// Always use the specified provider
    Fixed { provider_index: usize },
}

/// Backwards-compat alias for [`ComputeSchedulerConfig`].
pub type SchedulerConfig = ComputeSchedulerConfig;

/// Configuration for the scheduler
#[derive(Debug, Clone)]
pub struct ComputeSchedulerConfig {
    /// Scheduling policy
    pub policy: SchedulingPolicy,

    /// Maximum concurrent executions per provider
    pub max_concurrent_per_provider: usize,

    /// Enable provider health checking
    pub health_check_enabled: bool,

    /// Health check interval (milliseconds)
    pub health_check_interval_ms: u64,

    /// Maximum retry attempts on failure
    pub max_retries: usize,

    /// Retry delay (milliseconds)
    pub retry_delay_ms: u64,

    /// Cost weight factors
    pub cost_weights: CostWeights,
}

impl Default for ComputeSchedulerConfig {
    fn default() -> Self {
        Self {
            policy: SchedulingPolicy::CostBased,
            max_concurrent_per_provider: 100,
            health_check_enabled: false,
            health_check_interval_ms: 30000,
            max_retries: 2,
            retry_delay_ms: 100,
            cost_weights: CostWeights::default(),
        }
    }
}

/// Weights for cost calculation
#[derive(Debug, Clone)]
pub struct CostWeights {
    /// CPU cost weight
    pub cpu: f64,
    /// I/O cost weight
    pub io: f64,
    /// Network cost weight
    pub network: f64,
    /// Memory cost weight
    pub memory: f64,
}

impl Default for CostWeights {
    fn default() -> Self {
        Self {
            cpu: 1.0,
            io: 2.0,
            network: 5.0,
            memory: 0.5,
        }
    }
}

// ============================================================================
// Provider State
// ============================================================================

/// State tracking for a compute provider
struct ProviderState {
    /// The provider
    provider: Arc<dyn ComputeProvider>,

    /// Number of active executions
    active_executions: AtomicUsize,

    /// Total executions
    total_executions: AtomicU64,

    /// Successful executions
    successful_executions: AtomicU64,

    /// Failed executions
    failed_executions: AtomicU64,

    /// Total execution time (milliseconds)
    total_execution_time_ms: AtomicU64,

    /// Is provider healthy
    is_healthy: std::sync::atomic::AtomicBool,

    /// Last health check time
    #[allow(dead_code)]
    last_health_check: RwLock<std::time::Instant>,
}

impl ProviderState {
    /// Create a new provider state wrapping the given compute provider.
    fn new(provider: Arc<dyn ComputeProvider>) -> Self {
        Self {
            provider,
            active_executions: AtomicUsize::new(0),
            total_executions: AtomicU64::new(0),
            successful_executions: AtomicU64::new(0),
            failed_executions: AtomicU64::new(0),
            total_execution_time_ms: AtomicU64::new(0),
            is_healthy: std::sync::atomic::AtomicBool::new(true),
            last_health_check: RwLock::new(std::time::Instant::now()),
        }
    }

    /// Record the start of a new execution on this provider.
    fn start_execution(&self) {
        self.active_executions.fetch_add(1, Ordering::SeqCst);
        self.total_executions.fetch_add(1, Ordering::SeqCst);
    }

    /// Record the completion of an execution, updating success/failure and timing stats.
    fn finish_execution(&self, success: bool, duration_ms: u64) {
        self.active_executions.fetch_sub(1, Ordering::SeqCst);
        self.total_execution_time_ms
            .fetch_add(duration_ms, Ordering::SeqCst);

        if success {
            self.successful_executions.fetch_add(1, Ordering::SeqCst);
        } else {
            self.failed_executions.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Compute the current load score, combining active executions and failure penalty.
    fn load(&self) -> f64 {
        let active = self.active_executions.load(Ordering::SeqCst) as f64;
        let total = self.total_executions.load(Ordering::SeqCst) as f64;

        if total == 0.0 {
            active
        } else {
            // Consider both current load and historical success rate
            let success = self.successful_executions.load(Ordering::SeqCst) as f64;
            let failure_rate = if total > 0.0 {
                1.0 - (success / total)
            } else {
                0.0
            };
            active + (failure_rate * 10.0) // Penalize providers with failures
        }
    }

    /// Return the ratio of successful executions to total executions (1.0 when none run).
    fn success_rate(&self) -> f64 {
        let total = self.total_executions.load(Ordering::SeqCst) as f64;
        if total == 0.0 {
            1.0
        } else {
            let success = self.successful_executions.load(Ordering::SeqCst) as f64;
            success / total
        }
    }
}

// ============================================================================
// Scheduler Statistics
// ============================================================================

/// Scheduler statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchedulerStatistics {
    /// Total plans scheduled
    pub total_plans: u64,

    /// Plans by provider
    pub plans_by_provider: HashMap<String, u64>,

    /// Average selection time (microseconds)
    pub avg_selection_time_us: f64,

    /// Total execution time (milliseconds)
    pub total_execution_time_ms: u64,

    /// Failed plans
    pub failed_plans: u64,

    /// Retried plans
    pub retried_plans: u64,
}

// ============================================================================
// Compute Scheduler
// ============================================================================

/// Scheduler for compute plan execution
///
/// The scheduler manages multiple compute providers and selects the best one
/// for each query plan based on capabilities, cost, and current load.
pub struct ComputeScheduler {
    /// Registered providers with state
    providers: Vec<ProviderState>,

    /// Default provider index
    default_provider_index: usize,

    /// Scheduler configuration
    config: ComputeSchedulerConfig,

    /// Round-robin counter (for RoundRobin policy)
    round_robin_counter: AtomicUsize,

    /// Scheduler statistics
    statistics: RwLock<SchedulerStatistics>,
}

impl std::fmt::Debug for ComputeScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputeScheduler")
            .field("provider_count", &self.providers.len())
            .field("default_provider_index", &self.default_provider_index)
            .field("config", &self.config)
            .finish()
    }
}

impl ComputeScheduler {
    /// Create a new builder
    pub fn builder() -> ComputeSchedulerBuilder {
        ComputeSchedulerBuilder::new()
    }

    /// Create a scheduler with a single local provider
    pub fn with_local_provider(provider: Arc<dyn ComputeProvider>) -> Self {
        Self {
            providers: vec![ProviderState::new(provider)],
            default_provider_index: 0,
            config: ComputeSchedulerConfig::default(),
            round_robin_counter: AtomicUsize::new(0),
            statistics: RwLock::new(SchedulerStatistics::default()),
        }
    }

    /// Schedule and execute a compute plan
    ///
    /// This method selects the best provider and executes the plan.
    #[instrument(skip(self, plan), fields(plan_id = %plan.id))]
    pub async fn schedule(&self, plan: ComputePlan) -> Result<RecordBatchStream> {
        self.schedule_with_context(plan, ExecutionContext::default())
            .await
    }

    /// Schedule and execute with custom context
    #[instrument(skip(self, plan, ctx), fields(plan_id = %plan.id, execution_id = %ctx.execution_id))]
    pub async fn schedule_with_context(
        &self,
        plan: ComputePlan,
        ctx: ExecutionContext,
    ) -> Result<RecordBatchStream> {
        let start = std::time::Instant::now();

        // Select provider
        let provider_idx = self.select_provider(&plan)?;
        let provider_state = &self.providers[provider_idx];

        let selection_time = start.elapsed().as_micros();
        debug!(
            provider = provider_state.provider.provider_name(),
            selection_time_us = selection_time,
            "Selected provider"
        );

        // Execute with retries
        let mut last_error = None;
        let mut attempts = 0;

        while attempts <= self.config.max_retries {
            if attempts > 0 {
                // Update retry statistics
                {
                    let mut stats = self.statistics.write();
                    stats.retried_plans += 1;
                }

                // Wait before retry
                tokio::time::sleep(std::time::Duration::from_millis(
                    self.config.retry_delay_ms * (1 << attempts), // Exponential backoff
                ))
                .await;

                // Try selecting a different provider if available
                if let Ok(alt_idx) = self.select_alternative_provider(&plan, provider_idx) {
                    let alt_state = &self.providers[alt_idx];
                    info!(
                        "Retrying with alternative provider: {}",
                        alt_state.provider.provider_name()
                    );

                    let exec_start = std::time::Instant::now();
                    alt_state.start_execution();

                    match alt_state.provider.execute(&plan, &ctx).await {
                        Ok(result) => {
                            alt_state
                                .finish_execution(true, exec_start.elapsed().as_millis() as u64);
                            self.update_statistics(&plan, alt_state.provider.provider_name(), true);
                            return Ok(result.data);
                        }
                        Err(e) => {
                            alt_state
                                .finish_execution(false, exec_start.elapsed().as_millis() as u64);
                            last_error = Some(e);
                        }
                    }

                    attempts += 1;
                    continue;
                }
            }

            let exec_start = std::time::Instant::now();
            provider_state.start_execution();

            match provider_state.provider.execute(&plan, &ctx).await {
                Ok(result) => {
                    let duration = exec_start.elapsed().as_millis() as u64;
                    provider_state.finish_execution(true, duration);
                    self.update_statistics(&plan, provider_state.provider.provider_name(), true);

                    info!(
                        provider = provider_state.provider.provider_name(),
                        duration_ms = duration,
                        "Plan executed successfully"
                    );

                    return Ok(result.data);
                }
                Err(e) => {
                    let duration = exec_start.elapsed().as_millis() as u64;
                    provider_state.finish_execution(false, duration);

                    warn!(
                        provider = provider_state.provider.provider_name(),
                        error = %e,
                        attempt = attempts,
                        "Plan execution failed"
                    );

                    last_error = Some(e);
                }
            }

            attempts += 1;
        }

        // All retries exhausted
        self.update_statistics(&plan, provider_state.provider.provider_name(), false);
        bail!(
            "Plan execution failed after {} attempts: {}",
            attempts,
            last_error.map_or_else(|| "unknown error".to_string(), |e| e.to_string())
        )
    }

    /// Select the best provider for a plan
    fn select_provider(&self, plan: &ComputePlan) -> Result<usize> {
        // Check hints first
        if let Some(preferred) = &plan.hints.preferred_provider {
            for (i, state) in self.providers.iter().enumerate() {
                if state.provider.provider_name() == preferred {
                    if state.provider.can_execute(plan) {
                        return Ok(i);
                    } else {
                        warn!(
                            provider = preferred,
                            "Preferred provider cannot execute plan, falling back to selection"
                        );
                        break;
                    }
                }
            }
        }

        match self.config.policy {
            SchedulingPolicy::CostBased => self.select_by_cost(plan),
            SchedulingPolicy::RoundRobin => self.select_round_robin(plan),
            SchedulingPolicy::CapabilityFirst => self.select_by_capability(plan),
            SchedulingPolicy::LoadBalanced => self.select_by_load(plan),
            SchedulingPolicy::Fixed { provider_index } => {
                if provider_index < self.providers.len() {
                    Ok(provider_index)
                } else {
                    Ok(self.default_provider_index)
                }
            }
        }
    }

    /// Select provider with lowest cost
    fn select_by_cost(&self, plan: &ComputePlan) -> Result<usize> {
        let mut best_idx = None;
        let mut best_cost = f64::MAX;

        for (i, state) in self.providers.iter().enumerate() {
            if !state.provider.can_execute(plan) || !state.is_healthy.load(Ordering::SeqCst) {
                continue;
            }

            if let Ok(estimate) = state.provider.estimate_cost(plan) {
                let weighted_cost = self.calculate_weighted_cost(&estimate);

                // Adjust for current load
                let load_factor = 1.0 + (state.load() * 0.1);
                let adjusted_cost = weighted_cost * load_factor;

                if adjusted_cost < best_cost {
                    best_cost = adjusted_cost;
                    best_idx = Some(i);
                }
            }
        }

        best_idx.ok_or_else(|| anyhow::anyhow!("No provider can execute this plan"))
    }

    /// Select provider round-robin
    fn select_round_robin(&self, plan: &ComputePlan) -> Result<usize> {
        let start = self.round_robin_counter.fetch_add(1, Ordering::SeqCst);
        let count = self.providers.len();

        for offset in 0..count {
            let idx = (start + offset) % count;
            let state = &self.providers[idx];

            if state.provider.can_execute(plan) && state.is_healthy.load(Ordering::SeqCst) {
                return Ok(idx);
            }
        }

        // Fall back to default
        if self.providers[self.default_provider_index]
            .provider
            .can_execute(plan)
        {
            Ok(self.default_provider_index)
        } else {
            bail!("No provider can execute this plan")
        }
    }

    /// Select provider with best capability match
    fn select_by_capability(&self, plan: &ComputePlan) -> Result<usize> {
        let mut best_idx = None;
        let mut best_score = -1i32;

        for (i, state) in self.providers.iter().enumerate() {
            if !state.provider.can_execute(plan) || !state.is_healthy.load(Ordering::SeqCst) {
                continue;
            }

            let caps = state.provider.capabilities();
            let score = self.capability_score(&caps, plan);

            if score > best_score {
                best_score = score;
                best_idx = Some(i);
            }
        }

        best_idx.ok_or_else(|| anyhow::anyhow!("No provider can execute this plan"))
    }

    /// Select provider with lowest load
    fn select_by_load(&self, plan: &ComputePlan) -> Result<usize> {
        let mut best_idx = None;
        let mut best_load = f64::MAX;

        for (i, state) in self.providers.iter().enumerate() {
            if !state.provider.can_execute(plan) || !state.is_healthy.load(Ordering::SeqCst) {
                continue;
            }

            let load = state.load();

            // Also consider success rate
            let success_factor = 1.0 / (state.success_rate() + 0.1);
            let adjusted_load = load * success_factor;

            if adjusted_load < best_load {
                best_load = adjusted_load;
                best_idx = Some(i);
            }
        }

        best_idx.ok_or_else(|| anyhow::anyhow!("No provider can execute this plan"))
    }

    /// Select an alternative provider (for retries)
    fn select_alternative_provider(&self, plan: &ComputePlan, exclude: usize) -> Result<usize> {
        let mut best_idx = None;
        let mut best_score = f64::MAX;

        for (i, state) in self.providers.iter().enumerate() {
            if i == exclude {
                continue;
            }

            if !state.provider.can_execute(plan) || !state.is_healthy.load(Ordering::SeqCst) {
                continue;
            }

            // Use load as the primary factor for retry selection
            let score = state.load();

            if score < best_score {
                best_score = score;
                best_idx = Some(i);
            }
        }

        best_idx.ok_or_else(|| anyhow::anyhow!("No alternative provider available"))
    }

    /// Calculate weighted cost
    fn calculate_weighted_cost(&self, estimate: &CostEstimate) -> f64 {
        let weights = &self.config.cost_weights;
        (estimate.cpu_cost * weights.cpu)
            + (estimate.io_cost * weights.io)
            + (estimate.network_cost * weights.network)
            + (estimate.memory_bytes as f64 / (1024.0 * 1024.0) * weights.memory)
    }

    /// Calculate capability match score
    fn capability_score(&self, caps: &ComputeCapabilities, plan: &ComputePlan) -> i32 {
        let mut score = 0;

        // Bonus for specific capabilities matching plan requirements
        if plan.has_vector_operations() && caps.supports_vector_search {
            score += 10;
        }

        if plan.has_graph_operations() && caps.supports_graph_traversal {
            score += 10;
        }

        if caps.supports_filter_pushdown {
            score += 2;
        }

        if caps.supports_projection_pushdown {
            score += 2;
        }

        // Bonus for parallelism
        score += caps.max_parallelism.min(16) as i32;

        score
    }

    /// Update scheduler statistics
    fn update_statistics(&self, _plan: &ComputePlan, provider_name: &str, success: bool) {
        let mut stats = self.statistics.write();
        stats.total_plans += 1;

        *stats
            .plans_by_provider
            .entry(provider_name.to_string())
            .or_insert(0) += 1;

        if !success {
            stats.failed_plans += 1;
        }
    }

    /// Get scheduler statistics
    pub fn statistics(&self) -> SchedulerStatistics {
        self.statistics.read().clone()
    }

    /// Get provider statistics
    pub fn provider_statistics(&self) -> Vec<ProviderStatistics> {
        self.providers
            .iter()
            .map(|state| ProviderStatistics {
                name: state.provider.provider_name().to_string(),
                active_executions: state.active_executions.load(Ordering::SeqCst),
                total_executions: state.total_executions.load(Ordering::SeqCst),
                successful_executions: state.successful_executions.load(Ordering::SeqCst),
                failed_executions: state.failed_executions.load(Ordering::SeqCst),
                total_execution_time_ms: state.total_execution_time_ms.load(Ordering::SeqCst),
                is_healthy: state.is_healthy.load(Ordering::SeqCst),
                success_rate: state.success_rate(),
            })
            .collect()
    }

    /// Get number of registered providers
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Get provider by index
    pub fn get_provider(&self, index: usize) -> Option<&Arc<dyn ComputeProvider>> {
        self.providers.get(index).map(|s| &s.provider)
    }

    /// Shutdown all providers
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down compute scheduler");

        for state in &self.providers {
            if let Err(e) = state.provider.shutdown().await {
                warn!(
                    provider = state.provider.provider_name(),
                    error = %e,
                    "Error shutting down provider"
                );
            }
        }

        Ok(())
    }
}

/// Statistics for a single provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatistics {
    pub name: String,
    pub active_executions: usize,
    pub total_executions: u64,
    pub successful_executions: u64,
    pub failed_executions: u64,
    pub total_execution_time_ms: u64,
    pub is_healthy: bool,
    pub success_rate: f64,
}

// ============================================================================
// Builder
// ============================================================================

/// Builder for ComputeScheduler
pub struct ComputeSchedulerBuilder {
    /// Registered compute providers available for scheduling
    providers: Vec<Arc<dyn ComputeProvider>>,
    /// Provider used when no specific provider is requested
    default_provider: Option<Arc<dyn ComputeProvider>>,
    /// Scheduler configuration (retry policy, timeouts, etc.)
    config: ComputeSchedulerConfig,
}

impl ComputeSchedulerBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            default_provider: None,
            config: ComputeSchedulerConfig::default(),
        }
    }

    /// Set the default provider
    pub fn default_provider(mut self, provider: Arc<dyn ComputeProvider>) -> Self {
        self.default_provider = Some(provider);
        self
    }

    /// Add a provider
    pub fn add_provider(mut self, provider: Arc<dyn ComputeProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    /// Set scheduling policy
    pub fn policy(mut self, policy: SchedulingPolicy) -> Self {
        self.config.policy = policy;
        self
    }

    /// Set configuration
    pub fn config(mut self, config: ComputeSchedulerConfig) -> Self {
        self.config = config;
        self
    }

    /// Set max concurrent executions per provider
    pub fn max_concurrent(mut self, max: usize) -> Self {
        self.config.max_concurrent_per_provider = max;
        self
    }

    /// Enable health checking
    pub fn enable_health_check(mut self, interval_ms: u64) -> Self {
        self.config.health_check_enabled = true;
        self.config.health_check_interval_ms = interval_ms;
        self
    }

    /// Set retry configuration
    pub fn retries(mut self, max_retries: usize, delay_ms: u64) -> Self {
        self.config.max_retries = max_retries;
        self.config.retry_delay_ms = delay_ms;
        self
    }

    /// Set cost weights
    pub fn cost_weights(mut self, weights: CostWeights) -> Self {
        self.config.cost_weights = weights;
        self
    }

    /// Build the scheduler
    pub fn build(self) -> Result<ComputeScheduler> {
        // Determine providers list
        let mut all_providers = self.providers;

        // Add default provider if not already in list
        let default_idx = if let Some(default) = self.default_provider {
            let existing_idx = all_providers
                .iter()
                .position(|p| p.provider_name() == default.provider_name());

            if let Some(idx) = existing_idx {
                idx
            } else {
                all_providers.insert(0, default);
                0
            }
        } else if all_providers.is_empty() {
            bail!("At least one provider must be specified")
        } else {
            0
        };

        if all_providers.is_empty() {
            bail!("At least one provider must be specified")
        }

        let provider_states: Vec<_> = all_providers.into_iter().map(ProviderState::new).collect();

        info!(
            provider_count = provider_states.len(),
            default = provider_states[default_idx].provider.provider_name(),
            policy = ?self.config.policy,
            "Built compute scheduler"
        );

        Ok(ComputeScheduler {
            providers: provider_states,
            default_provider_index: default_idx,
            config: self.config,
            round_robin_counter: AtomicUsize::new(0),
            statistics: RwLock::new(SchedulerStatistics::default()),
        })
    }
}

impl Default for ComputeSchedulerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::plan::PlanNode;
    use crate::compute::provider::LocalComputeProvider;

    fn create_test_provider() -> Arc<dyn ComputeProvider> {
        Arc::new(LocalComputeProvider::new().unwrap())
    }

    fn create_test_plan() -> ComputePlan {
        ComputePlan::new("test", PlanNode::table_scan("users"))
    }

    #[test]
    fn test_scheduler_builder() {
        let provider = create_test_provider();

        let scheduler = ComputeScheduler::builder()
            .default_provider(provider.clone())
            .add_provider(provider)
            .policy(SchedulingPolicy::CostBased)
            .max_concurrent(50)
            .retries(3, 100)
            .build()
            .unwrap();

        assert_eq!(scheduler.provider_count(), 1);
    }

    #[test]
    fn test_scheduler_with_local_provider() {
        let provider = create_test_provider();
        let scheduler = ComputeScheduler::with_local_provider(provider);

        assert_eq!(scheduler.provider_count(), 1);
    }

    #[test]
    fn test_scheduler_empty_fails() {
        let result = ComputeScheduler::builder().build();
        assert!(result.is_err());
    }

    #[test]
    fn test_scheduling_policies() {
        let policy = SchedulingPolicy::CostBased;
        assert!(matches!(policy, SchedulingPolicy::CostBased));

        let policy = SchedulingPolicy::RoundRobin;
        assert!(matches!(policy, SchedulingPolicy::RoundRobin));

        let policy = SchedulingPolicy::Fixed { provider_index: 0 };
        assert!(matches!(
            policy,
            SchedulingPolicy::Fixed { provider_index: 0 }
        ));
    }

    #[test]
    fn test_provider_selection() {
        let provider = create_test_provider();
        let scheduler = ComputeScheduler::builder()
            .default_provider(provider.clone())
            .add_provider(provider)
            .policy(SchedulingPolicy::CostBased)
            .build()
            .unwrap();

        let plan = create_test_plan();
        let idx = scheduler.select_provider(&plan).unwrap();
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_round_robin_selection() {
        let provider = create_test_provider();
        let scheduler = ComputeScheduler::builder()
            .default_provider(provider.clone())
            .add_provider(provider)
            .policy(SchedulingPolicy::RoundRobin)
            .build()
            .unwrap();

        let plan = create_test_plan();

        // Multiple selections should rotate
        let idx1 = scheduler.select_provider(&plan).unwrap();
        let idx2 = scheduler.select_provider(&plan).unwrap();

        // With single provider, both should be 0
        assert_eq!(idx1, 0);
        assert_eq!(idx2, 0);
    }

    #[test]
    fn test_statistics() {
        let provider = create_test_provider();
        let scheduler = ComputeScheduler::with_local_provider(provider);

        let stats = scheduler.statistics();
        assert_eq!(stats.total_plans, 0);
        assert_eq!(stats.failed_plans, 0);
    }

    #[test]
    fn test_provider_statistics() {
        let provider = create_test_provider();
        let scheduler = ComputeScheduler::with_local_provider(provider);

        let provider_stats = scheduler.provider_statistics();
        assert_eq!(provider_stats.len(), 1);
        assert_eq!(provider_stats[0].name, "local");
        assert_eq!(provider_stats[0].total_executions, 0);
        assert!(provider_stats[0].is_healthy);
    }

    #[test]
    fn test_cost_weights() {
        let weights = CostWeights {
            cpu: 2.0,
            io: 3.0,
            network: 10.0,
            memory: 1.0,
        };

        let estimate = CostEstimate {
            cpu_cost: 100.0,
            io_cost: 50.0,
            network_cost: 10.0,
            memory_bytes: 1024 * 1024, // 1MB
            estimated_rows: 1000,
        };

        let provider = create_test_provider();
        let scheduler = ComputeScheduler::builder()
            .default_provider(provider)
            .cost_weights(weights)
            .build()
            .unwrap();

        let weighted = scheduler.calculate_weighted_cost(&estimate);
        // 100*2 + 50*3 + 10*10 + 1*1 = 200 + 150 + 100 + 1 = 451
        assert!((weighted - 451.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_schedule_execution() {
        let provider = create_test_provider();
        let scheduler = ComputeScheduler::with_local_provider(provider);

        let plan = create_test_plan();
        let result = scheduler.schedule(plan).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_schedule_with_context() {
        let provider = create_test_provider();
        let scheduler = ComputeScheduler::with_local_provider(provider);

        let plan = create_test_plan();
        let ctx = ExecutionContext::with_id("test-exec-001")
            .with_batch_size(1000)
            .with_metrics(true);

        let result = scheduler.schedule_with_context(plan, ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_shutdown() {
        let provider = create_test_provider();
        let scheduler = ComputeScheduler::with_local_provider(provider);

        let result = scheduler.shutdown().await;
        assert!(result.is_ok());
    }
}
