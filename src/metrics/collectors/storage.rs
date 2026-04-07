//! Storage metrics collector

use super::{MetricsCollector, MetricsSample};
use anyhow::Result;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct StorageMetricsCollector;

impl StorageMetricsCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StorageMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl MetricsCollector for StorageMetricsCollector {
    async fn collect(&self) -> Result<MetricsSample> {
        let mut values = HashMap::new();

        // Deferred: Get real metrics from metadata store / vector operations service
        // For now, use placeholder values to enable testing
        // These should be populated by:
        // 1. VectorOperationsService tracking inserts/deletes
        // 2. CollectionService tracking collection creation/deletion
        // 3. Storage engines reporting their sizes
        values.insert("total_vectors".to_string(), 100000.0);
        values.insert("total_collections".to_string(), 0.0); // Added for compatibility
        values.insert("storage_size_bytes".to_string(), 1024.0 * 1024.0 * 100.0); // 100MB
        values.insert("cache_hit_rate".to_string(), 0.85);
        values.insert("wal_size_bytes".to_string(), 1024.0 * 1024.0 * 50.0);

        Ok(MetricsSample {
            timestamp: Instant::now(),
            collector: self.name().to_string(),
            values,
        })
    }

    fn name(&self) -> &'static str {
        "storage"
    }

    fn recommended_interval(&self) -> Duration {
        Duration::from_secs(120) // Optimized: 60s -> 120s (2 minutes)
    }
}
