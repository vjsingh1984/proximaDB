//! System metrics collector (CPU, memory, disk, network)

use super::{MetricsCollector, MetricsSample};
use anyhow::Result;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct SystemMetricsCollector;

impl SystemMetricsCollector {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl MetricsCollector for SystemMetricsCollector {
    async fn collect(&self) -> Result<MetricsSample> {
        let mut values = HashMap::new();
        
        // Placeholder values - would use actual system metrics
        values.insert("cpu_usage_percent".to_string(), 45.0);
        values.insert("memory_used_bytes".to_string(), 1024.0 * 1024.0 * 512.0);
        values.insert("disk_used_bytes".to_string(), 1024.0 * 1024.0 * 1024.0 * 10.0);
        
        Ok(MetricsSample {
            timestamp: Instant::now(),
            collector: self.name().to_string(),
            values,
        })
    }
    
    fn name(&self) -> &'static str {
        "system"
    }
    
    fn recommended_interval(&self) -> Duration {
        Duration::from_secs(30)
    }
}