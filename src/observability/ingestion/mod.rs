// Ingestion pipeline for observability data
//
// High-throughput ingestion with:
// - Lock-free ring buffer for back-pressure management
// - Multiple input adapters (OTLP, Syslog, Fluent, CEF/LEEF, OCSF)
// - Parallel parsing workers
// - Alert rule evaluation during ingestion

pub mod adapters;
pub mod buffer;
pub mod parser;

use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, warn};

use crate::proto::proximadb_v1::{IngestionFormat, LogEntry, MetricSample};

use super::storage::ObservabilityStorage;
use super::IngestResult;
use self::buffer::RingBuffer;
use self::parser::LogParser;

/// Observability ingester with high-throughput processing
pub struct ObservabilityIngester {
    /// Ring buffer for buffering events
    buffer: Arc<RingBuffer>,
    /// Log parser for different formats
    parser: Arc<LogParser>,
    /// Storage layer
    storage: Arc<ObservabilityStorage>,
    /// Number of worker threads
    num_workers: usize,
}

impl ObservabilityIngester {
    /// Create a new ingester
    pub async fn new(storage: Arc<ObservabilityStorage>) -> Result<Self> {
        let num_workers = num_cpus::get().max(2);
        let buffer_size = 10_000_000; // 10M entries

        info!(
            "Creating observability ingester with {} workers and {}M buffer",
            num_workers,
            buffer_size / 1_000_000
        );

        Ok(Self {
            buffer: Arc::new(RingBuffer::new(buffer_size)),
            parser: Arc::new(LogParser::new()),
            storage,
            num_workers,
        })
    }

    /// Ingest a batch of logs
    pub async fn ingest_logs(
        &self,
        namespace: &str,
        logs: Vec<LogEntry>,
        format: Option<IngestionFormat>,
    ) -> Result<IngestResult> {
        let start = std::time::Instant::now();
        let mut result = IngestResult::default();

        // Parse logs if raw format provided
        let parsed_logs = match format {
            Some(fmt) => self.parser.parse_batch(&logs, fmt)?,
            None => logs,
        };

        // Write to storage
        for log in parsed_logs {
            match self.storage.write_log(namespace, &log).await {
                Ok(_) => result.ingested += 1,
                Err(e) => {
                    result.failed += 1;
                    if result.errors.len() < 10 {
                        result.errors.push(e.to_string());
                    }
                }
            }
        }

        result.processing_time_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }

    /// Ingest a batch of metrics
    pub async fn ingest_metrics(
        &self,
        namespace: &str,
        metrics: Vec<MetricSample>,
    ) -> Result<IngestResult> {
        let start = std::time::Instant::now();
        let mut result = IngestResult::default();

        for metric in metrics {
            match self.storage.write_metric(namespace, &metric).await {
                Ok(_) => result.ingested += 1,
                Err(e) => {
                    result.failed += 1;
                    if result.errors.len() < 10 {
                        result.errors.push(e.to_string());
                    }
                }
            }
        }

        result.processing_time_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }

    /// Get ingestion statistics
    pub fn stats(&self) -> IngestionStats {
        IngestionStats {
            buffer_size: self.buffer.capacity(),
            buffer_used: self.buffer.len(),
            buffer_utilization: self.buffer.utilization(),
            num_workers: self.num_workers,
        }
    }
}

/// Ingestion statistics
#[derive(Debug, Clone)]
pub struct IngestionStats {
    /// Total buffer capacity
    pub buffer_size: usize,
    /// Current buffer usage
    pub buffer_used: usize,
    /// Buffer utilization percentage
    pub buffer_utilization: f32,
    /// Number of processing workers
    pub num_workers: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ingestion_stats() {
        let stats = IngestionStats {
            buffer_size: 1_000_000,
            buffer_used: 500_000,
            buffer_utilization: 0.5,
            num_workers: 4,
        };
        assert_eq!(stats.buffer_utilization, 0.5);
    }
}
