//! Execution Logging for RL Planner
//!
//! Persists execution results to JSONL files for analysis and replay.
//! Each line is a complete JSON object representing one query execution.
//!
//! ## A4: Enhanced Query Explanation Integration
//!
//! This module also provides integration with the enhanced explain system:
//! - `RLDecisionLogger`: Captures detailed decision context for EXPLAIN output
//! - `ExplainIntegration`: Bridges RL planner decisions to enhanced explain plans
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::query::rl_planner::logging::{RLDecisionLogger, ExplainIntegration};
//!
//! // Log decision with explanation
//! let explanation = decision_logger.log_decision_with_explanation(state, action);
//!
//! // Convert to enhanced explain format
//! let rl_explanation = ExplainIntegration::to_rl_explanation(&decision_logger);
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use super::action::ExecutionAction;
use super::state::PlannerState;

/// Complete execution log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionLog {
    /// Unique query identifier
    pub query_id: String,
    /// Collection identifier
    pub collection_id: String,
    /// Timestamp (ISO 8601)
    pub timestamp: String,

    /// State at decision time
    pub state: PlannerState,
    /// Action taken
    pub action: ExecutionAction,

    /// Execution results
    pub latency_ms: f64,
    pub recall: f32,
    pub precision: f32,
    pub throughput_qps: f32,
    pub memory_peak_mb: f32,
    pub candidates_scanned: usize,
    pub candidates_returned: usize,

    /// Stage-by-stage breakdown
    pub stages: Vec<StageLog>,

    /// Computed reward
    pub reward: f32,

    /// Additional metadata
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl ExecutionLog {
    /// Create new execution log builder
    pub fn builder(
        query_id: impl Into<String>,
        collection_id: impl Into<String>,
    ) -> ExecutionLogBuilder {
        ExecutionLogBuilder::new(query_id, collection_id)
    }

    /// Get total stage latency
    pub fn total_stage_latency_us(&self) -> u64 {
        self.stages.iter().map(|s| s.latency_us).sum()
    }

    /// Get overall pruning ratio
    pub fn overall_pruning_ratio(&self) -> f32 {
        if self.candidates_scanned == 0 {
            return 0.0;
        }
        1.0 - (self.candidates_returned as f32 / self.candidates_scanned as f32)
    }
}

/// Individual stage within execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageLog {
    /// Stage name (e.g., "Binary", "INT8", "HNSW")
    pub stage_name: String,
    /// Candidates entering this stage
    pub candidates_in: usize,
    /// Candidates after filtering
    pub candidates_out: usize,
    /// Stage latency in microseconds
    pub latency_us: u64,
    /// Pruning ratio (1 - out/in)
    pub pruning_ratio: f32,
}

impl StageLog {
    /// Create new stage log
    pub fn new(
        stage_name: impl Into<String>,
        candidates_in: usize,
        candidates_out: usize,
        latency_us: u64,
    ) -> Self {
        let pruning_ratio = if candidates_in > 0 {
            1.0 - (candidates_out as f32 / candidates_in as f32)
        } else {
            0.0
        };

        Self {
            stage_name: stage_name.into(),
            candidates_in,
            candidates_out,
            latency_us,
            pruning_ratio,
        }
    }
}

/// Builder for ExecutionLog
pub struct ExecutionLogBuilder {
    log: ExecutionLog,
}

impl ExecutionLogBuilder {
    /// Create new builder
    pub fn new(query_id: impl Into<String>, collection_id: impl Into<String>) -> Self {
        Self {
            log: ExecutionLog {
                query_id: query_id.into(),
                collection_id: collection_id.into(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                state: PlannerState::default(),
                action: ExecutionAction::default(),
                latency_ms: 0.0,
                recall: 0.0,
                precision: 0.0,
                throughput_qps: 0.0,
                memory_peak_mb: 0.0,
                candidates_scanned: 0,
                candidates_returned: 0,
                stages: Vec::new(),
                reward: 0.0,
                metadata: serde_json::Value::Null,
            },
        }
    }

    pub fn state(mut self, state: PlannerState) -> Self {
        self.log.state = state;
        self
    }

    pub fn action(mut self, action: ExecutionAction) -> Self {
        self.log.action = action;
        self
    }

    pub fn latency_ms(mut self, latency: f64) -> Self {
        self.log.latency_ms = latency;
        self
    }

    pub fn recall(mut self, recall: f32) -> Self {
        self.log.recall = recall;
        self
    }

    pub fn precision(mut self, precision: f32) -> Self {
        self.log.precision = precision;
        self
    }

    pub fn throughput_qps(mut self, qps: f32) -> Self {
        self.log.throughput_qps = qps;
        self
    }

    pub fn memory_peak_mb(mut self, memory: f32) -> Self {
        self.log.memory_peak_mb = memory;
        self
    }

    pub fn candidates(mut self, scanned: usize, returned: usize) -> Self {
        self.log.candidates_scanned = scanned;
        self.log.candidates_returned = returned;
        self
    }

    pub fn add_stage(mut self, stage: StageLog) -> Self {
        self.log.stages.push(stage);
        self
    }

    pub fn reward(mut self, reward: f32) -> Self {
        self.log.reward = reward;
        self
    }

    pub fn metadata(mut self, metadata: serde_json::Value) -> Self {
        self.log.metadata = metadata;
        self
    }

    pub fn build(self) -> ExecutionLog {
        self.log
    }
}

/// Execution logger with JSONL persistence
pub struct ExecutionLogger {
    /// Path to JSONL log file
    log_path: Option<PathBuf>,
    /// In-memory buffer for batch writes
    buffer: Vec<ExecutionLog>,
    /// Buffer flush threshold
    flush_threshold: usize,
    /// File handle (lazy initialized)
    #[allow(dead_code)]
    file: Option<tokio::fs::File>,
}

impl ExecutionLogger {
    /// Create new logger with optional file path
    pub fn new(log_path: Option<String>) -> Self {
        Self {
            log_path: log_path.map(PathBuf::from),
            buffer: Vec::new(),
            flush_threshold: 100,
            file: None,
        }
    }

    /// Log execution result
    pub async fn log(&mut self, entry: &ExecutionLog) -> anyhow::Result<()> {
        // Always buffer
        self.buffer.push(entry.clone());

        // Flush if buffer is full
        if self.buffer.len() >= self.flush_threshold {
            self.flush().await?;
        }

        Ok(())
    }

    /// Flush buffer to file
    pub async fn flush(&mut self) -> anyhow::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        let log_path = match &self.log_path {
            Some(path) => path.clone(),
            None => {
                // Default to /tmp if no path specified
                PathBuf::from("/tmp/proximadb_rl_execution.jsonl")
            }
        };

        // Ensure parent directory exists
        if let Some(parent) = log_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Open file for append
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await?;

        // Write each entry as a single line
        for entry in &self.buffer {
            let json = serde_json::to_string(entry)?;
            file.write_all(json.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        file.flush().await?;
        self.buffer.clear();

        tracing::debug!(
            "Flushed {} execution logs to {:?}",
            self.buffer.len(),
            log_path
        );
        Ok(())
    }

    /// Get buffered entries
    pub fn buffered(&self) -> &[ExecutionLog] {
        &self.buffer
    }

    /// Clear buffer without flushing
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Read logs from file
    pub async fn read_logs(path: &str) -> anyhow::Result<Vec<ExecutionLog>> {
        let content = tokio::fs::read_to_string(path).await?;
        let mut logs = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ExecutionLog>(line) {
                Ok(log) => logs.push(log),
                Err(e) => {
                    tracing::warn!("Failed to parse log line: {}", e);
                }
            }
        }

        Ok(logs)
    }

    /// Analyze logs for insights
    pub fn analyze(logs: &[ExecutionLog]) -> LogAnalysis {
        if logs.is_empty() {
            return LogAnalysis::default();
        }

        // Group by action
        let mut action_stats: std::collections::HashMap<String, Vec<f32>> =
            std::collections::HashMap::new();

        for log in logs {
            let action_desc = log.action.describe();
            action_stats
                .entry(action_desc)
                .or_default()
                .push(log.reward);
        }

        // Compute per-action statistics
        let mut action_performance: Vec<(String, f32, usize)> = action_stats
            .into_iter()
            .map(|(action, rewards)| {
                let avg = rewards.iter().sum::<f32>() / rewards.len() as f32;
                (action, avg, rewards.len())
            })
            .collect();

        action_performance
            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Overall statistics
        let total_queries = logs.len();
        let avg_latency = logs.iter().map(|l| l.latency_ms).sum::<f64>() / total_queries as f64;
        let avg_recall = logs.iter().map(|l| l.recall).sum::<f32>() / total_queries as f32;
        let avg_reward = logs.iter().map(|l| l.reward).sum::<f32>() / total_queries as f32;

        LogAnalysis {
            total_queries,
            avg_latency_ms: avg_latency,
            avg_recall,
            avg_reward,
            action_performance,
        }
    }
}

impl Default for ExecutionLogger {
    fn default() -> Self {
        Self::new(None)
    }
}

/// Analysis results from execution logs
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogAnalysis {
    pub total_queries: usize,
    pub avg_latency_ms: f64,
    pub avg_recall: f32,
    pub avg_reward: f32,
    /// Per-action performance: (action_desc, avg_reward, count)
    pub action_performance: Vec<(String, f32, usize)>,
}

impl LogAnalysis {
    /// Get best performing action
    pub fn best_action(&self) -> Option<&str> {
        self.action_performance.first().map(|(a, _, _)| a.as_str())
    }

    /// Get worst performing action
    pub fn worst_action(&self) -> Option<&str> {
        self.action_performance.last().map(|(a, _, _)| a.as_str())
    }

    /// Print summary
    pub fn print_summary(&self) {
        println!("=== Execution Log Analysis ===");
        println!("Total queries: {}", self.total_queries);
        println!("Avg latency: {:.2}ms", self.avg_latency_ms);
        println!("Avg recall: {:.2}%", self.avg_recall * 100.0);
        println!("Avg reward: {:.3}", self.avg_reward);
        println!();
        println!("Top 5 Actions:");
        for (i, (action, reward, count)) in self.action_performance.iter().take(5).enumerate() {
            println!(
                "  {}. {} (reward: {:.3}, count: {})",
                i + 1,
                action,
                reward,
                count
            );
        }
    }
}

// ============================================================================
// A4: Enhanced Query Explanation - RL Planner Integration
// ============================================================================

/// Decision context captured by the RL planner for explain output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RLDecisionContext {
    /// The state at decision time
    pub state: PlannerState,
    /// The selected action
    pub selected_action: ExecutionAction,
    /// Confidence score (0.0 to 1.0)
    pub confidence: f32,
    /// Whether this was exploration or exploitation
    pub is_exploration: bool,
    /// Alternative actions considered with their expected rewards
    pub alternatives: Vec<(ExecutionAction, f32)>,
    /// Key features that influenced the decision
    pub influential_features: Vec<(String, f64, f64)>, // (name, value, weight)
    /// Historical performance of the selected action
    pub action_history: Option<ActionHistory>,
    /// Timestamp of the decision
    pub decision_timestamp: String,
}

impl RLDecisionContext {
    /// Create new decision context
    pub fn new(state: PlannerState, action: ExecutionAction, confidence: f32) -> Self {
        Self {
            state,
            selected_action: action,
            confidence,
            is_exploration: false,
            alternatives: Vec::new(),
            influential_features: Vec::new(),
            action_history: None,
            decision_timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Builder method to set exploration flag
    pub fn with_exploration(mut self, is_exploration: bool) -> Self {
        self.is_exploration = is_exploration;
        self
    }

    /// Builder method to add alternative
    pub fn with_alternative(mut self, action: ExecutionAction, expected_reward: f32) -> Self {
        self.alternatives.push((action, expected_reward));
        self
    }

    /// Builder method to add influential feature
    pub fn with_feature(mut self, name: &str, value: f64, weight: f64) -> Self {
        self.influential_features
            .push((name.to_string(), value, weight));
        self
    }

    /// Builder method to add action history
    pub fn with_history(mut self, history: ActionHistory) -> Self {
        self.action_history = Some(history);
        self
    }
}

/// Historical performance data for an action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionHistory {
    /// Number of times this action was executed
    pub execution_count: u64,
    /// Average reward achieved
    pub average_reward: f32,
    /// Average latency in milliseconds
    pub average_latency_ms: f64,
    /// Average recall achieved
    pub average_recall: f32,
    /// Success rate
    pub success_rate: f32,
    /// Last update timestamp
    pub last_updated: String,
}

impl ActionHistory {
    /// Create from execution statistics
    pub fn from_stats(count: u64, avg_reward: f32, avg_latency: f64, avg_recall: f32) -> Self {
        Self {
            execution_count: count,
            average_reward: avg_reward,
            average_latency_ms: avg_latency,
            average_recall: avg_recall,
            success_rate: if avg_recall > 0.9 { 1.0 } else { avg_recall },
            last_updated: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// Logger for RL planner decisions with enhanced explain integration
#[derive(Debug)]
pub struct RLDecisionLogger {
    /// Recent decisions (kept in memory for quick access)
    recent_decisions: Vec<RLDecisionContext>,
    /// Maximum decisions to keep in memory
    max_recent: usize,
    /// Action statistics (action_desc -> (total_reward, count, total_latency, total_recall))
    action_stats: std::collections::HashMap<String, (f64, u64, f64, f64)>,
}

impl Default for RLDecisionLogger {
    fn default() -> Self {
        Self::new(100)
    }
}

impl RLDecisionLogger {
    /// Create new decision logger
    pub fn new(max_recent: usize) -> Self {
        Self {
            recent_decisions: Vec::new(),
            max_recent,
            action_stats: std::collections::HashMap::new(),
        }
    }

    /// Log a decision context
    pub fn log_decision(&mut self, context: RLDecisionContext) {
        // Update action stats
        let action_desc = context.selected_action.describe();
        let entry = self
            .action_stats
            .entry(action_desc)
            .or_insert((0.0, 0, 0.0, 0.0));
        entry.0 += context.confidence as f64;
        entry.1 += 1;

        // Add to recent decisions
        if self.recent_decisions.len() >= self.max_recent {
            self.recent_decisions.remove(0);
        }
        self.recent_decisions.push(context);
    }

    /// Update stats after execution
    pub fn update_execution_result(
        &mut self,
        action: &ExecutionAction,
        latency_ms: f64,
        recall: f32,
    ) {
        let action_desc = action.describe();
        if let Some(entry) = self.action_stats.get_mut(&action_desc) {
            entry.2 += latency_ms;
            entry.3 += recall as f64;
        }
    }

    /// Get action history for a specific action
    pub fn get_action_history(&self, action: &ExecutionAction) -> Option<ActionHistory> {
        let action_desc = action.describe();
        self.action_stats.get(&action_desc).map(
            |(total_reward, count, total_latency, total_recall)| {
                ActionHistory::from_stats(
                    *count,
                    (*total_reward / *count as f64) as f32,
                    *total_latency / *count as f64,
                    (*total_recall / *count as f64) as f32,
                )
            },
        )
    }

    /// Get most recent decision
    pub fn last_decision(&self) -> Option<&RLDecisionContext> {
        self.recent_decisions.last()
    }

    /// Get all recent decisions
    pub fn recent_decisions(&self) -> &[RLDecisionContext] {
        &self.recent_decisions
    }

    /// Get top performing actions
    pub fn top_actions(&self, n: usize) -> Vec<(String, f32, u64)> {
        let mut actions: Vec<_> = self
            .action_stats
            .iter()
            .map(|(action, (total_reward, count, _, _))| {
                (
                    action.clone(),
                    (*total_reward / *count as f64) as f32,
                    *count,
                )
            })
            .collect();

        actions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        actions.truncate(n);
        actions
    }
}

/// Integration utilities for connecting RL planner to enhanced explain system
pub struct ExplainIntegration;

impl ExplainIntegration {
    /// Convert RLDecisionContext to RLPlannerExplanation for enhanced explain output
    pub fn to_rl_explanation(
        context: &RLDecisionContext,
    ) -> crate::query::explain::RLPlannerExplanation {
        use crate::query::explain::{
            AlternativeAction, ExplorationMode, HistoricalPerformance, InfluentialFeature,
            RLPlannerExplanation,
        };

        let mut explanation =
            RLPlannerExplanation::new(context.selected_action.describe(), context.confidence);

        // Set exploration mode
        explanation = explanation.with_exploration_mode(if context.is_exploration {
            ExplorationMode::Exploration
        } else {
            ExplorationMode::Exploitation
        });

        // Add alternatives
        for (alt_action, expected_reward) in &context.alternatives {
            let alt = AlternativeAction::new(alt_action.describe(), *expected_reward);
            explanation = explanation.with_alternative(alt);
        }

        // Add influential features
        for (name, value, weight) in &context.influential_features {
            let feature = InfluentialFeature::new(name.clone(), *value, *weight)
                .with_interpretation(Self::interpret_feature(name, *value, *weight));
            explanation = explanation.with_feature(feature);
        }

        // Add historical performance
        if let Some(history) = &context.action_history {
            let hist_perf =
                HistoricalPerformance::new(history.execution_count, history.average_reward)
                    .with_latency(history.average_latency_ms)
                    .with_recall(history.average_recall)
                    .with_success_rate(history.success_rate);
            explanation = explanation.with_history(hist_perf);
        }

        // Generate reason based on context
        explanation = explanation.with_reason(Self::generate_reason(context));

        explanation
    }

    /// Interpret a feature value for human-readable output
    fn interpret_feature(name: &str, value: f64, weight: f64) -> String {
        let influence = if weight > 0.0 { "favors" } else { "disfavors" };
        let strength = if weight.abs() > 0.5 {
            "strongly"
        } else if weight.abs() > 0.2 {
            "moderately"
        } else {
            "slightly"
        };

        match name {
            "collection_size" => {
                format!(
                    "Collection size ({:.0}) {} {} the selected action",
                    value * 1e9, // Denormalize from log scale
                    strength,
                    influence
                )
            }
            "top_k" => {
                format!(
                    "Top-K value ({:.0}) {} {} the selected action",
                    value * 1000.0,
                    strength,
                    influence
                )
            }
            "memory_pressure" => {
                format!(
                    "Memory pressure ({:.1}%) {} {} the selected action",
                    value * 100.0,
                    strength,
                    influence
                )
            }
            "cache_hit_rate" => {
                format!(
                    "Cache hit rate ({:.1}%) {} {} the selected action",
                    value * 100.0,
                    strength,
                    influence
                )
            }
            "has_filter" => {
                if value > 0.5 {
                    format!(
                        "Presence of filters {} {} the selected action",
                        strength, influence
                    )
                } else {
                    format!(
                        "Absence of filters {} {} the selected action",
                        strength, influence
                    )
                }
            }
            _ => {
                format!(
                    "{} (value: {:.3}) {} {} the selected action",
                    name, value, strength, influence
                )
            }
        }
    }

    /// Generate a human-readable reason for the action selection
    fn generate_reason(context: &RLDecisionContext) -> String {
        let action_desc = context.selected_action.describe();

        if context.is_exploration {
            return format!(
                "Exploring action '{}' to gather more performance data",
                action_desc
            );
        }

        let mut reasons = Vec::new();

        // Check confidence level
        if context.confidence > 0.8 {
            reasons.push(format!(
                "High confidence ({:.0}%) based on historical performance",
                context.confidence * 100.0
            ));
        }

        // Check state features
        let state = &context.state;
        if state.collection_size > 100_000 {
            reasons.push("Large collection size favors approximate search".to_string());
        }
        if state.has_filter {
            reasons.push("Query filter present, considering pre-filtering strategies".to_string());
        }
        if state.memory_pressure > 0.7 {
            reasons.push("High memory pressure favors memory-efficient strategies".to_string());
        }

        // Check action history
        if let Some(history) = &context.action_history {
            if history.execution_count > 10 && history.average_recall > 0.95 {
                reasons.push(format!(
                    "Action achieved {:.1}% recall over {} executions",
                    history.average_recall * 100.0,
                    history.execution_count
                ));
            }
        }

        if reasons.is_empty() {
            format!(
                "Selected '{}' based on current workload characteristics",
                action_desc
            )
        } else {
            format!("Selected '{}': {}", action_desc, reasons.join("; "))
        }
    }

    /// Extract influential features from planner state
    pub fn extract_features(state: &PlannerState) -> Vec<(String, f64, f64)> {
        let features = state.as_feature_vector();
        let mut influential = Vec::new();

        // Map feature indices to names (based on PlannerState::as_feature_vector)
        let feature_names = [
            "query_dimension",
            "top_k",
            "has_filter",
            "filter_selectivity",
            "filter_complexity",
            "requested_exact",
            "collection_size",
            "storage_engine",
            "num_storage_segments",
            "avg_vectors_per_segment",
            "hnsw_available",
            "ivf_available",
            "lsh_available",
            "annoy_available",
            "pq_available",
            "flat_available",
            "quant_none",
            "quant_binary",
            "quant_int8",
            "quant_pq4",
            "quant_pq8",
            "quant_fp16",
            "memory_pressure",
            "cpu_utilization",
            "pending_queries",
            "cache_hit_rate",
            "available_parallelism",
            "avg_latency",
            "avg_recall",
            "avg_throughput",
            "latency_variance",
        ];

        // Calculate simple feature importance (deviation from neutral)
        for (i, (name, &value)) in feature_names.iter().zip(features.iter()).enumerate() {
            if i >= features.len() {
                break;
            }
            let neutral = 0.5; // Assume 0.5 is neutral for normalized features
            let importance = (value - neutral).abs();
            if importance > 0.2 {
                // Only include features with significant deviation
                let weight = if value > neutral {
                    importance
                } else {
                    -importance
                };
                influential.push((name.to_string(), value as f64, weight as f64));
            }
        }

        // Sort by absolute weight
        influential.sort_by(|a, b| {
            b.2.abs()
                .partial_cmp(&a.2.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Keep top 5 most influential
        influential.truncate(5);
        influential
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_log() {
        let stage = StageLog::new("Binary", 10000, 2000, 500);
        assert_eq!(stage.stage_name, "Binary");
        assert!((stage.pruning_ratio - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_execution_log_builder() {
        let log = ExecutionLog::builder("query_123", "test_collection")
            .state(PlannerState::default())
            .action(ExecutionAction::with_hnsw(100))
            .latency_ms(5.5)
            .recall(0.98)
            .candidates(10000, 10)
            .add_stage(StageLog::new("Binary", 10000, 2000, 500))
            .add_stage(StageLog::new("INT8", 2000, 500, 1200))
            .add_stage(StageLog::new("FP32", 500, 10, 3800))
            .reward(0.85)
            .build();

        assert_eq!(log.query_id, "query_123");
        assert_eq!(log.collection_id, "test_collection");
        assert!((log.latency_ms - 5.5).abs() < 0.01);
        assert_eq!(log.stages.len(), 3);
        assert_eq!(log.total_stage_latency_us(), 5500);
        assert!((log.overall_pruning_ratio() - 0.999).abs() < 0.001);
    }

    #[test]
    fn test_log_serialization() {
        let log = ExecutionLog::builder("q1", "c1")
            .latency_ms(10.0)
            .recall(0.95)
            .reward(0.8)
            .build();

        let json = serde_json::to_string(&log).unwrap();
        let parsed: ExecutionLog = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.query_id, log.query_id);
        assert!((parsed.latency_ms - log.latency_ms).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_logger_flush() {
        let path = "/tmp/test_rl_logger.jsonl";
        let mut logger = ExecutionLogger::new(Some(path.to_string()));

        // Add some logs
        for i in 0..5 {
            let log = ExecutionLog::builder(format!("q{}", i), "test")
                .latency_ms(i as f64 * 10.0)
                .reward(i as f32 / 5.0)
                .build();
            logger.log(&log).await.unwrap();
        }

        assert_eq!(logger.buffered().len(), 5);

        // Force flush
        logger.flush().await.unwrap();
        assert!(logger.buffered().is_empty());

        // Read back
        let logs = ExecutionLogger::read_logs(path).await.unwrap();
        assert_eq!(logs.len(), 5);

        // Cleanup
        let _ = tokio::fs::remove_file(path).await;
    }

    #[test]
    fn test_log_analysis() {
        let logs: Vec<ExecutionLog> = (0..100)
            .map(|i| {
                let action = if i % 2 == 0 {
                    ExecutionAction::with_hnsw(100)
                } else {
                    ExecutionAction::default()
                };
                let reward = if i % 2 == 0 { 0.8 } else { 0.4 };

                ExecutionLog::builder(format!("q{}", i), "test")
                    .action(action)
                    .latency_ms(10.0)
                    .recall(0.95)
                    .reward(reward)
                    .build()
            })
            .collect();

        let analysis = ExecutionLogger::analyze(&logs);

        assert_eq!(analysis.total_queries, 100);
        assert!((analysis.avg_reward - 0.6).abs() < 0.01);

        // HNSW should be best performing
        let best = analysis.best_action().unwrap();
        assert!(best.contains("HNSW"));
    }

    // ===== A4: Enhanced Explain Integration Tests =====

    #[test]
    fn test_rl_decision_context() {
        let state = PlannerState::default();
        let action = ExecutionAction::with_hnsw(100);

        let context = RLDecisionContext::new(state, action, 0.85)
            .with_exploration(false)
            .with_feature("collection_size", 0.8, 0.6)
            .with_alternative(ExecutionAction::with_ivf(16), 0.7);

        assert_eq!(context.confidence, 0.85);
        assert!(!context.is_exploration);
        assert_eq!(context.alternatives.len(), 1);
        assert_eq!(context.influential_features.len(), 1);
    }

    #[test]
    fn test_action_history() {
        let history = ActionHistory::from_stats(100, 0.85, 10.5, 0.98);

        assert_eq!(history.execution_count, 100);
        assert_eq!(history.average_reward, 0.85);
        assert_eq!(history.average_latency_ms, 10.5);
        assert_eq!(history.average_recall, 0.98);
    }

    #[test]
    fn test_rl_decision_logger() {
        let mut logger = RLDecisionLogger::new(10);

        // Log some decisions
        for i in 0..5 {
            let context = RLDecisionContext::new(
                PlannerState::default(),
                ExecutionAction::with_hnsw(100),
                0.8 + i as f32 * 0.02,
            );
            logger.log_decision(context);
        }

        assert_eq!(logger.recent_decisions().len(), 5);

        // Update execution result
        let action = ExecutionAction::with_hnsw(100);
        logger.update_execution_result(&action, 10.0, 0.95);

        // Get action history
        let history = logger.get_action_history(&action);
        assert!(history.is_some());
    }

    #[test]
    fn test_explain_integration_to_rl_explanation() {
        let state = PlannerState::default();
        let action = ExecutionAction::with_hnsw(100);
        let history = ActionHistory::from_stats(50, 0.9, 8.5, 0.97);

        let context = RLDecisionContext::new(state, action, 0.9)
            .with_exploration(false)
            .with_feature("collection_size", 0.8, 0.6)
            .with_feature("memory_pressure", 0.3, -0.2)
            .with_alternative(ExecutionAction::with_ivf(16), 0.75)
            .with_history(history);

        let explanation = ExplainIntegration::to_rl_explanation(&context);

        assert!(explanation.selected_action.contains("HNSW"));
        assert_eq!(explanation.confidence, 0.9);
        assert_eq!(explanation.alternatives_considered.len(), 1);
        assert_eq!(explanation.influential_features.len(), 2);
        assert!(explanation.historical_performance.is_some());
        assert!(explanation.selection_reason.is_some());
    }

    #[test]
    fn test_explain_integration_extract_features() {
        let state = PlannerState::builder()
            .collection_size(1_000_000)
            .memory_pressure(0.8)
            .top_k(100)
            .build();

        let features = ExplainIntegration::extract_features(&state);

        // Should extract features with significant deviation from neutral
        assert!(!features.is_empty());
        assert!(features.len() <= 5); // Should be limited to top 5
    }

    #[test]
    fn test_decision_logger_max_recent() {
        let mut logger = RLDecisionLogger::new(3);

        // Log more than max_recent decisions
        for i in 0..5 {
            let context = RLDecisionContext::new(
                PlannerState::default(),
                ExecutionAction::default(),
                i as f32 * 0.1,
            );
            logger.log_decision(context);
        }

        // Should only keep last 3
        assert_eq!(logger.recent_decisions().len(), 3);

        // Check that we have the last 3 decisions (confidence 0.2, 0.3, 0.4)
        let last = logger.last_decision().unwrap();
        assert!((last.confidence - 0.4).abs() < 0.01);
    }

    #[test]
    fn test_exploration_mode_in_explanation() {
        let context_explore =
            RLDecisionContext::new(PlannerState::default(), ExecutionAction::default(), 0.5)
                .with_exploration(true);

        let context_exploit =
            RLDecisionContext::new(PlannerState::default(), ExecutionAction::default(), 0.9)
                .with_exploration(false);

        let explain_explore = ExplainIntegration::to_rl_explanation(&context_explore);
        let explain_exploit = ExplainIntegration::to_rl_explanation(&context_exploit);

        assert_eq!(
            explain_explore.exploration_mode,
            crate::query::explain::ExplorationMode::Exploration
        );
        assert_eq!(
            explain_exploit.exploration_mode,
            crate::query::explain::ExplorationMode::Exploitation
        );

        // Exploration should mention exploring in the reason
        assert!(
            explain_explore
                .selection_reason
                .unwrap()
                .contains("Exploring")
        );
    }
}
