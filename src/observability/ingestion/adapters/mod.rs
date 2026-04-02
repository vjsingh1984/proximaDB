// Input adapters for observability ingestion
//
// Supports multiple input formats and protocols:
// - OTLP (OpenTelemetry Protocol) - gRPC and HTTP
// - Syslog (RFC 3164/5424) - TCP and UDP
// - Fluent (Fluent Bit/Fluentd forward protocol)
// - CEF (Common Event Format) - ArcSight
// - LEEF (Log Event Extended Format) - IBM QRadar
// - OCSF (Open Cybersecurity Schema Framework)
// - HTTP (JSON over HTTP)

pub mod cef_leef;
pub mod fluent;
pub mod http;
pub mod ocsf;
pub mod otlp;
pub mod otlp_grpc;
pub mod syslog;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::proto::proximadb_v1::LogEntry;

/// Trait for input adapters
#[async_trait]
pub trait InputAdapter: Send + Sync {
    /// Get the adapter name
    fn name(&self) -> &str;

    /// Start the adapter
    async fn start(&self) -> Result<()>;

    /// Stop the adapter
    async fn stop(&self) -> Result<()>;

    /// Check if the adapter is running
    fn is_running(&self) -> bool;

    /// Get the number of events received
    fn events_received(&self) -> u64;
}

/// Configuration for adapters
#[derive(Debug, Clone)]
pub struct AdapterConfig {
    /// Bind address for network adapters
    pub bind_address: SocketAddr,
    /// Maximum batch size for processing
    pub batch_size: usize,
    /// Buffer size for incoming events
    pub buffer_size: usize,
    /// Channel for sending parsed events
    pub sender: mpsc::Sender<Vec<LogEntry>>,
}

impl AdapterConfig {
    /// Create a new adapter configuration
    pub fn new(bind_address: SocketAddr, sender: mpsc::Sender<Vec<LogEntry>>) -> Self {
        Self {
            bind_address,
            batch_size: 1000,
            buffer_size: 10_000,
            sender,
        }
    }

    /// Set batch size
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Set buffer size
    pub fn with_buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }
}

/// Adapter manager for multiple input adapters
pub struct AdapterManager {
    /// Registered adapters
    adapters: Vec<Arc<dyn InputAdapter>>,
}

impl AdapterManager {
    /// Create a new adapter manager
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
        }
    }

    /// Register an adapter
    pub fn register(&mut self, adapter: Arc<dyn InputAdapter>) {
        self.adapters.push(adapter);
    }

    /// Start all adapters
    pub async fn start_all(&self) -> Result<()> {
        for adapter in &self.adapters {
            adapter.start().await?;
        }
        Ok(())
    }

    /// Stop all adapters
    pub async fn stop_all(&self) -> Result<()> {
        for adapter in &self.adapters {
            adapter.stop().await?;
        }
        Ok(())
    }

    /// Get adapter by name
    pub fn get(&self, name: &str) -> Option<&Arc<dyn InputAdapter>> {
        self.adapters.iter().find(|a| a.name() == name)
    }

    /// Get statistics for all adapters
    pub fn stats(&self) -> Vec<AdapterStats> {
        self.adapters
            .iter()
            .map(|a| AdapterStats {
                name: a.name().to_string(),
                running: a.is_running(),
                events_received: a.events_received(),
            })
            .collect()
    }
}

impl Default for AdapterManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics for an adapter
#[derive(Debug, Clone)]
pub struct AdapterStats {
    /// Adapter name
    pub name: String,
    /// Whether the adapter is running
    pub running: bool,
    /// Number of events received
    pub events_received: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adapter_manager_new() {
        let manager = AdapterManager::new();
        assert!(manager.adapters.is_empty());
    }
}
