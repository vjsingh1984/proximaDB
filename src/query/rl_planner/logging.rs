//! Execution Logging for RL Planner
//!
//! Persists execution results to JSONL files for analysis and replay.
//! Each line is a complete JSON object representing one query execution.

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
    pub fn builder(query_id: impl Into<String>, collection_id: impl Into<String>) -> ExecutionLogBuilder {
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

        tracing::debug!("Flushed {} execution logs to {:?}", self.buffer.len(), log_path);
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

        action_performance.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

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
}
