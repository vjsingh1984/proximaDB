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

//! # Graph Database Monitoring and Metrics
//!
//! This module provides comprehensive monitoring and metrics collection for ProximaDB's
//! graph database operations, including Prometheus metrics, performance tracking,
//! slow query logging, and operational insights.
//!
//! ## Key Features
//!
//! - **Prometheus Metrics**: Production-ready metrics for monitoring systems
//! - **Performance Tracking**: Query latency, throughput, and resource utilization
//! - **Slow Query Logging**: Identify and analyze performance bottlenecks
//! - **Memory Usage Monitoring**: Track memory consumption and garbage collection
//! - **Operation Profiling**: Detailed timing and resource usage for graph operations
//! - **Health Checks**: System health and readiness endpoints
//!
//! ## Metrics Categories
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │              Graph Metrics              │
//! ├─────────────────────────────────────────┤
//! │                                         │
//! │  ┌──────────────┬─────────────────────┐ │
//! │  │ Performance  │    Operations       │ │
//! │  │ • Latency    │    • Query Count    │ │
//! │  │ • Throughput │    • Error Count    │ │
//! │  │ • Memory     │    • Cache Hits     │ │
//! │  └──────────────┴─────────────────────┘ │
//! │                                         │
//! │  ┌──────────────┬─────────────────────┐ │
//! │  │ Resources    │    Business         │ │
//! │  │ • CPU Usage  │    • Node Count     │ │
//! │  │ • Memory     │    • Edge Count     │ │
//! │  │ • Disk I/O   │    • Traversals     │ │
//! │  └──────────────┴─────────────────────┘ │
//! └─────────────────────────────────────────┘
//! ```

use crate::core::error::ProximaDBError;
use crate::utils::Uuid;
use crate::graph::{NodeId, EdgeId, GraphMemoryPool};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use serde::{Serialize, Deserialize};
use tokio::sync::mpsc;

/// Main monitoring system for graph operations
pub struct GraphMonitor {
    /// Metrics collection and reporting
    metrics_collector: Arc<GraphMetricsCollector>,
    /// Slow query logger
    slow_query_logger: Arc<SlowQueryLogger>,
    /// Performance profiler
    profiler: Arc<GraphProfiler>,
    /// Configuration
    config: MonitoringConfig,
    /// Metrics export channel
    metrics_sender: Option<mpsc::UnboundedSender<MetricEvent>>,
}

/// Configuration for monitoring system
#[derive(Debug, Clone)]
pub struct MonitoringConfig {
    /// Enable/disable monitoring
    pub enabled: bool,
    /// Slow query threshold in milliseconds
    pub slow_query_threshold_ms: u64,
    /// Maximum number of slow queries to keep in memory
    pub max_slow_queries: usize,
    /// Metrics collection interval in seconds
    pub metrics_interval_sec: u64,
    /// Enable detailed profiling (higher overhead)
    pub detailed_profiling: bool,
    /// Enable Prometheus metrics export
    pub prometheus_enabled: bool,
    /// Prometheus metrics port
    pub prometheus_port: u16,
    /// Log file path for slow queries
    pub slow_query_log_path: Option<String>,
}

/// Metrics collector for graph operations
pub struct GraphMetricsCollector {
    /// Operation counters
    operation_counts: Arc<RwLock<HashMap<String, u64>>>,
    /// Error counters
    error_counts: Arc<RwLock<HashMap<String, u64>>>,
    /// Latency histograms
    latency_histograms: Arc<RwLock<HashMap<String, LatencyHistogram>>>,
    /// Resource usage metrics
    resource_metrics: Arc<RwLock<ResourceMetrics>>,
    /// Business metrics
    business_metrics: Arc<RwLock<BusinessMetrics>>,
    /// Cache metrics
    cache_metrics: Arc<RwLock<CacheMetrics>>,
}

/// Slow query logger
pub struct SlowQueryLogger {
    /// Recent slow queries (circular buffer)
    slow_queries: Arc<Mutex<VecDeque<SlowQueryRecord>>>,
    /// Configuration
    config: Arc<MonitoringConfig>,
}

/// Performance profiler for detailed analysis
pub struct GraphProfiler {
    /// Active profiles
    active_profiles: Arc<RwLock<HashMap<String, ProfileSession>>>,
    /// Completed profiles
    completed_profiles: Arc<RwLock<VecDeque<ProfileSummary>>>,
    /// Configuration
    config: Arc<MonitoringConfig>,
}

/// Latency histogram for tracking response times
#[derive(Debug, Clone, Default)]
pub struct LatencyHistogram {
    /// Histogram buckets (in milliseconds)
    pub buckets: Vec<(f64, u64)>, // (upper_bound, count)
    /// Total count
    pub count: u64,
    /// Sum of all values
    pub sum: f64,
}

/// Resource usage metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceMetrics {
    /// CPU usage percentage
    pub cpu_usage_percent: f64,
    /// Memory usage in MB
    pub memory_used_mb: f64,
    /// Memory usage percentage
    pub memory_usage_percent: f64,
    /// Heap size in MB
    pub heap_size_mb: f64,
    /// Number of active threads
    pub thread_count: u32,
    /// File descriptors used
    pub fd_count: u32,
    /// Disk I/O read bytes per second
    pub disk_read_bytes_per_sec: f64,
    /// Disk I/O write bytes per second  
    pub disk_write_bytes_per_sec: f64,
    /// Network bytes in per second
    pub network_in_bytes_per_sec: f64,
    /// Network bytes out per second
    pub network_out_bytes_per_sec: f64,
}

/// Business metrics for graph operations
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BusinessMetrics {
    /// Total number of nodes
    pub total_nodes: u64,
    /// Total number of edges
    pub total_edges: u64,
    /// Average node degree
    pub avg_node_degree: f64,
    /// Number of connected components
    pub connected_components: u32,
    /// Graph diameter (longest shortest path)
    pub graph_diameter: u32,
    /// Number of traversals per minute
    pub traversals_per_minute: f64,
    /// Number of pattern matches per minute
    pub pattern_matches_per_minute: f64,
    /// Number of hybrid queries per minute
    pub hybrid_queries_per_minute: f64,
}

/// Cache performance metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheMetrics {
    /// Query plan cache hits
    pub plan_cache_hits: u64,
    /// Query plan cache misses
    pub plan_cache_misses: u64,
    /// Node cache hits
    pub node_cache_hits: u64,
    /// Node cache misses
    pub node_cache_misses: u64,
    /// Edge cache hits
    pub edge_cache_hits: u64,
    /// Edge cache misses
    pub edge_cache_misses: u64,
    /// Cache hit ratio overall
    pub overall_hit_ratio: f64,
}

/// Record of a slow query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowQueryRecord {
    /// Query ID
    pub query_id: String,
    /// Query type (e.g., "traversal", "pattern_match", "hybrid")
    pub query_type: String,
    /// Query parameters (sanitized)
    pub parameters: HashMap<String, String>,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
    /// Timestamp when query started
    pub timestamp: SystemTime,
    /// Memory used in MB
    pub memory_used_mb: f64,
    /// Number of nodes visited
    pub nodes_visited: usize,
    /// Number of edges traversed
    pub edges_traversed: usize,
    /// Error message (if any)
    pub error: Option<String>,
    /// Stack trace for analysis
    pub stack_trace: Option<String>,
}

/// Active profiling session
#[derive(Debug)]
pub struct ProfileSession {
    /// Session ID
    pub session_id: String,
    /// Start time
    pub start_time: Instant,
    /// Operation being profiled
    pub operation: String,
    /// Detailed timing information
    pub timings: HashMap<String, Duration>,
    /// Resource usage samples
    pub resource_samples: Vec<ResourceSample>,
    /// Custom metadata
    pub metadata: HashMap<String, String>,
}

/// Resource usage sample during profiling
#[derive(Debug, Clone)]
pub struct ResourceSample {
    /// Timestamp relative to session start
    pub timestamp: Duration,
    /// CPU usage at this point
    pub cpu_percent: f64,
    /// Memory usage at this point
    pub memory_mb: f64,
    /// Thread count
    pub thread_count: u32,
}

/// Summary of a completed profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSummary {
    /// Session ID
    pub session_id: String,
    /// Operation profiled
    pub operation: String,
    /// Total execution time
    pub total_time_ms: u64,
    /// Breakdown of time by component
    pub time_breakdown: HashMap<String, u64>,
    /// Peak resource usage
    pub peak_cpu_percent: f64,
    /// Peak memory usage
    pub peak_memory_mb: f64,
    /// Average resource usage
    pub avg_cpu_percent: f64,
    /// Average memory usage
    pub avg_memory_mb: f64,
    /// Number of samples collected
    pub sample_count: usize,
    /// Completion timestamp
    pub completed_at: SystemTime,
}

/// Metric event for async processing
#[derive(Debug, Clone)]
pub enum MetricEvent {
    /// Operation completed
    OperationCompleted {
        operation: String,
        duration: Duration,
        success: bool,
        metadata: HashMap<String, String>,
    },
    /// Resource usage update
    ResourceUpdate(ResourceMetrics),
    /// Cache event
    CacheEvent {
        cache_type: String,
        hit: bool,
    },
    /// Business metric update
    BusinessUpdate(BusinessMetrics),
}

/// Health check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheck {
    /// Overall health status
    pub status: HealthStatus,
    /// Individual component checks
    pub components: HashMap<String, ComponentHealth>,
    /// System uptime in seconds
    pub uptime_seconds: u64,
    /// Timestamp of check
    pub timestamp: SystemTime,
}

/// Health status values
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum HealthStatus {
    /// All systems operational
    Healthy,
    /// Some issues but still functional
    Degraded,
    /// System is not functioning properly
    Unhealthy,
}

/// Individual component health
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    /// Component status
    pub status: HealthStatus,
    /// Human-readable message
    pub message: String,
    /// Last check time
    pub last_check: SystemTime,
    /// Response time in milliseconds
    pub response_time_ms: u64,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            slow_query_threshold_ms: 1000, // 1 second
            max_slow_queries: 1000,
            metrics_interval_sec: 30,
            detailed_profiling: false,
            prometheus_enabled: true,
            prometheus_port: 9090,
            slow_query_log_path: Some("/var/log/proximadb/slow_queries.log".to_string()),
        }
    }
}

impl GraphMonitor {
    /// Create a new graph monitor
    pub fn new(config: MonitoringConfig) -> Self {
        let metrics_collector = Arc::new(GraphMetricsCollector::new());
        let slow_query_logger = Arc::new(SlowQueryLogger::new(Arc::new(config.clone())));
        let profiler = Arc::new(GraphProfiler::new(Arc::new(config.clone())));
        
        Self {
            metrics_collector,
            slow_query_logger,
            profiler,
            config,
            metrics_sender: None,
        }
    }
    
    /// Start monitoring with background tasks
    pub async fn start(&mut self) -> Result<(), ProximaDBError> {
        if !self.config.enabled {
            return Ok(());
        }
        
        // Create metrics processing channel
        let (sender, mut receiver) = mpsc::unbounded_channel();
        self.metrics_sender = Some(sender);
        
        // Start metrics processing task
        let metrics_collector = Arc::clone(&self.metrics_collector);
        tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                if let Err(e) = Self::process_metric_event(&metrics_collector, event).await {
                    eprintln!("Error processing metric event: {}", e);
                }
            }
        });
        
        // Start periodic metrics collection
        if self.config.metrics_interval_sec > 0 {
            let collector = Arc::clone(&self.metrics_collector);
            let interval = Duration::from_secs(self.config.metrics_interval_sec);
            
            tokio::spawn(async move {
                let mut interval_timer = tokio::time::interval(interval);
                loop {
                    interval_timer.tick().await;
                    if let Err(e) = Self::collect_system_metrics(&collector).await {
                        eprintln!("Error collecting system metrics: {}", e);
                    }
                }
            });
        }
        
        // Start Prometheus metrics server if enabled
        if self.config.prometheus_enabled {
            self.start_prometheus_server().await?;
        }
        
        Ok(())
    }
    
    /// Record an operation completion
    pub fn record_operation(
        &self,
        operation: &str,
        duration: Duration,
        success: bool,
        metadata: HashMap<String, String>,
    ) {
        if !self.config.enabled {
            return;
        }
        
        if let Some(ref sender) = self.metrics_sender {
            let event = MetricEvent::OperationCompleted {
                operation: operation.to_string(),
                duration,
                success,
                metadata,
            };
            
            if let Err(_) = sender.send(event) {
                eprintln!("Failed to send operation metric event");
            }
        }
        
        // Check for slow query
        let duration_ms = duration.as_millis() as u64;
        if duration_ms >= self.config.slow_query_threshold_ms {
            self.slow_query_logger.log_slow_query(SlowQueryRecord {
                query_id: Uuid::new_v4().to_string(),
                query_type: operation.to_string(),
                parameters: metadata.clone(),
                execution_time_ms: duration_ms,
                timestamp: SystemTime::now(),
                memory_used_mb: 0.0, // TODO: Get actual memory usage
                nodes_visited: 0,    // TODO: Get from metadata
                edges_traversed: 0,  // TODO: Get from metadata
                error: if success { None } else { Some("Operation failed".to_string()) },
                stack_trace: None,
            });
        }
    }
    
    /// Start a profiling session
    pub fn start_profiling(&self, operation: &str) -> String {
        if !self.config.detailed_profiling {
            return String::new();
        }
        
        self.profiler.start_session(operation)
    }
    
    /// End a profiling session
    pub fn end_profiling(&self, session_id: &str) -> Option<ProfileSummary> {
        if !self.config.detailed_profiling {
            return None;
        }
        
        self.profiler.end_session(session_id)
    }
    
    /// Record a profiling checkpoint
    pub fn record_checkpoint(&self, session_id: &str, checkpoint: &str, duration: Duration) {
        if !self.config.detailed_profiling {
            return;
        }
        
        self.profiler.record_timing(session_id, checkpoint, duration);
    }
    
    /// Get current metrics snapshot
    pub fn get_metrics_snapshot(&self) -> Result<MetricsSnapshot, ProximaDBError> {
        let operation_counts = self.metrics_collector.operation_counts.read()
            .map_err(|_| ProximaDBError::internal("Failed to read operation counts"))?
            .clone();
        
        let error_counts = self.metrics_collector.error_counts.read()
            .map_err(|_| ProximaDBError::internal("Failed to read error counts"))?
            .clone();
        
        let resource_metrics = self.metrics_collector.resource_metrics.read()
            .map_err(|_| ProximaDBError::internal("Failed to read resource metrics"))?
            .clone();
        
        let business_metrics = self.metrics_collector.business_metrics.read()
            .map_err(|_| ProximaDBError::internal("Failed to read business metrics"))?
            .clone();
        
        let cache_metrics = self.metrics_collector.cache_metrics.read()
            .map_err(|_| ProximaDBError::internal("Failed to read cache metrics"))?
            .clone();
        
        Ok(MetricsSnapshot {
            timestamp: SystemTime::now(),
            operation_counts,
            error_counts,
            resource_metrics,
            business_metrics,
            cache_metrics,
        })
    }
    
    /// Get recent slow queries
    pub fn get_slow_queries(&self, limit: Option<usize>) -> Vec<SlowQueryRecord> {
        self.slow_query_logger.get_recent_slow_queries(limit)
    }
    
    /// Get recent profile summaries
    pub fn get_recent_profiles(&self, limit: Option<usize>) -> Vec<ProfileSummary> {
        self.profiler.get_recent_summaries(limit)
    }
    
    /// Perform health check
    pub async fn health_check(&self, memory_pool: &Arc<GraphMemoryPool>) -> HealthCheck {
        let mut components = HashMap::new();
        let start_time = Instant::now();
        
        // Check graph memory pool
        let graph_health = self.check_graph_health(memory_pool).await;
        components.insert("graph".to_string(), graph_health);
        
        // Check metrics collector
        let metrics_health = self.check_metrics_health().await;
        components.insert("metrics".to_string(), metrics_health);
        
        // Check profiler
        let profiler_health = self.check_profiler_health().await;
        components.insert("profiler".to_string(), profiler_health);
        
        // Determine overall status
        let overall_status = if components.values().all(|h| matches!(h.status, HealthStatus::Healthy)) {
            HealthStatus::Healthy
        } else if components.values().any(|h| matches!(h.status, HealthStatus::Unhealthy)) {
            HealthStatus::Unhealthy
        } else {
            HealthStatus::Degraded
        };
        
        HealthCheck {
            status: overall_status,
            components,
            uptime_seconds: start_time.elapsed().as_secs(), // Placeholder
            timestamp: SystemTime::now(),
        }
    }
    
    /// Update graph statistics for monitoring
    pub fn update_graph_statistics(&self, memory_pool: &Arc<GraphMemoryPool>) {
        if !self.config.enabled {
            return;
        }
        
        let node_count = memory_pool.node_count() as u64;
        let edge_count = memory_pool.edge_count() as u64;
        let avg_node_degree = if node_count > 0 {
            edge_count as f64 / node_count as f64
        } else {
            0.0
        };
        
        let business_metrics = BusinessMetrics {
            total_nodes: node_count,
            total_edges: edge_count,
            avg_node_degree,
            connected_components: 1, // Simplified for now
            graph_diameter: 0,       // Would require computation
            traversals_per_minute: 0.0,     // Would track over time
            pattern_matches_per_minute: 0.0, // Would track over time
            hybrid_queries_per_minute: 0.0,  // Would track over time
        };
        
        if let Some(ref sender) = self.metrics_sender {
            let event = MetricEvent::BusinessUpdate(business_metrics);
            if let Err(_) = sender.send(event) {
                eprintln!("Failed to send business metrics update");
            }
        }
    }
    
    /// Process metric events
    async fn process_metric_event(
        collector: &GraphMetricsCollector,
        event: MetricEvent,
    ) -> Result<(), ProximaDBError> {
        match event {
            MetricEvent::OperationCompleted { operation, duration, success, metadata: _ } => {
                // Update operation counts
                {
                    let mut counts = collector.operation_counts.write()
                        .map_err(|_| ProximaDBError::internal("Failed to write operation counts"))?;
                    *counts.entry(operation.clone()).or_insert(0) += 1;
                }
                
                // Update error counts if failed
                if !success {
                    let mut error_counts = collector.error_counts.write()
                        .map_err(|_| ProximaDBError::internal("Failed to write error counts"))?;
                    *error_counts.entry(operation.clone()).or_insert(0) += 1;
                }
                
                // Update latency histogram
                {
                    let mut histograms = collector.latency_histograms.write()
                        .map_err(|_| ProximaDBError::internal("Failed to write latency histograms"))?;
                    
                    let histogram = histograms.entry(operation).or_insert_with(|| {
                        LatencyHistogram::new()
                    });
                    
                    histogram.record(duration.as_millis() as f64);
                }
            }
            
            MetricEvent::ResourceUpdate(metrics) => {
                let mut resource_metrics = collector.resource_metrics.write()
                    .map_err(|_| ProximaDBError::internal("Failed to write resource metrics"))?;
                *resource_metrics = metrics;
            }
            
            MetricEvent::CacheEvent { cache_type, hit } => {
                let mut cache_metrics = collector.cache_metrics.write()
                    .map_err(|_| ProximaDBError::internal("Failed to write cache metrics"))?;
                
                match cache_type.as_str() {
                    "plan" => {
                        if hit {
                            cache_metrics.plan_cache_hits += 1;
                        } else {
                            cache_metrics.plan_cache_misses += 1;
                        }
                    }
                    "node" => {
                        if hit {
                            cache_metrics.node_cache_hits += 1;
                        } else {
                            cache_metrics.node_cache_misses += 1;
                        }
                    }
                    "edge" => {
                        if hit {
                            cache_metrics.edge_cache_hits += 1;
                        } else {
                            cache_metrics.edge_cache_misses += 1;
                        }
                    }
                    _ => {}
                }
                
                // Update overall hit ratio
                let total_hits = cache_metrics.plan_cache_hits + 
                                cache_metrics.node_cache_hits + 
                                cache_metrics.edge_cache_hits;
                let total_requests = total_hits + 
                                   cache_metrics.plan_cache_misses + 
                                   cache_metrics.node_cache_misses + 
                                   cache_metrics.edge_cache_misses;
                
                cache_metrics.overall_hit_ratio = if total_requests > 0 {
                    total_hits as f64 / total_requests as f64
                } else {
                    0.0
                };
            }
            
            MetricEvent::BusinessUpdate(metrics) => {
                let mut business_metrics = collector.business_metrics.write()
                    .map_err(|_| ProximaDBError::internal("Failed to write business metrics"))?;
                *business_metrics = metrics;
            }
        }
        
        Ok(())
    }
    
    /// Collect system-level metrics
    async fn collect_system_metrics(
        collector: &GraphMetricsCollector,
    ) -> Result<(), ProximaDBError> {
        // This is a simplified implementation
        // In production, you would use proper system monitoring libraries
        
        let metrics = ResourceMetrics {
            cpu_usage_percent: 0.0,  // Would use sysinfo or similar
            memory_used_mb: 0.0,     // Would use sysinfo or similar
            memory_usage_percent: 0.0,
            heap_size_mb: 0.0,
            thread_count: 0,
            fd_count: 0,
            disk_read_bytes_per_sec: 0.0,
            disk_write_bytes_per_sec: 0.0,
            network_in_bytes_per_sec: 0.0,
            network_out_bytes_per_sec: 0.0,
        };
        
        let mut resource_metrics = collector.resource_metrics.write()
            .map_err(|_| ProximaDBError::internal("Failed to write resource metrics"))?;
        *resource_metrics = metrics;
        
        Ok(())
    }
    
    /// Start Prometheus metrics server
    async fn start_prometheus_server(&self) -> Result<(), ProximaDBError> {
        // This is a placeholder for Prometheus integration
        // In production, you would use the prometheus crate and set up HTTP endpoints
        println!("Prometheus metrics server would start on port {}", self.config.prometheus_port);
        Ok(())
    }
    
    /// Check graph component health
    async fn check_graph_health(&self, memory_pool: &Arc<GraphMemoryPool>) -> ComponentHealth {
        let start_time = Instant::now();
        
        // Basic health check - ensure we can access the memory pool
        let node_count = memory_pool.node_count();
        let edge_count = memory_pool.edge_count();
        
        let response_time = start_time.elapsed().as_millis() as u64;
        
        if node_count == 0 && edge_count == 0 {
            ComponentHealth {
                status: HealthStatus::Degraded,
                message: "Graph is empty".to_string(),
                last_check: SystemTime::now(),
                response_time_ms: response_time,
            }
        } else {
            ComponentHealth {
                status: HealthStatus::Healthy,
                message: format!("Graph has {} nodes and {} edges", node_count, edge_count),
                last_check: SystemTime::now(),
                response_time_ms: response_time,
            }
        }
    }
    
    /// Check metrics collector health
    async fn check_metrics_health(&self) -> ComponentHealth {
        let start_time = Instant::now();
        
        // Check if we can access metrics
        match self.metrics_collector.operation_counts.read() {
            Ok(_) => ComponentHealth {
                status: HealthStatus::Healthy,
                message: "Metrics collector operational".to_string(),
                last_check: SystemTime::now(),
                response_time_ms: start_time.elapsed().as_millis() as u64,
            },
            Err(_) => ComponentHealth {
                status: HealthStatus::Unhealthy,
                message: "Cannot access metrics collector".to_string(),
                last_check: SystemTime::now(),
                response_time_ms: start_time.elapsed().as_millis() as u64,
            },
        }
    }
    
    /// Check profiler health
    async fn check_profiler_health(&self) -> ComponentHealth {
        let start_time = Instant::now();
        
        // Check if profiler is accessible
        match self.profiler.active_profiles.read() {
            Ok(_) => ComponentHealth {
                status: HealthStatus::Healthy,
                message: "Profiler operational".to_string(),
                last_check: SystemTime::now(),
                response_time_ms: start_time.elapsed().as_millis() as u64,
            },
            Err(_) => ComponentHealth {
                status: HealthStatus::Unhealthy,
                message: "Cannot access profiler".to_string(),
                last_check: SystemTime::now(),
                response_time_ms: start_time.elapsed().as_millis() as u64,
            },
        }
    }
}

/// Snapshot of all metrics at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub timestamp: SystemTime,
    pub operation_counts: HashMap<String, u64>,
    pub error_counts: HashMap<String, u64>,
    pub resource_metrics: ResourceMetrics,
    pub business_metrics: BusinessMetrics,
    pub cache_metrics: CacheMetrics,
}

impl GraphMetricsCollector {
    pub fn new() -> Self {
        Self {
            operation_counts: Arc::new(RwLock::new(HashMap::new())),
            error_counts: Arc::new(RwLock::new(HashMap::new())),
            latency_histograms: Arc::new(RwLock::new(HashMap::new())),
            resource_metrics: Arc::new(RwLock::new(ResourceMetrics::default())),
            business_metrics: Arc::new(RwLock::new(BusinessMetrics::default())),
            cache_metrics: Arc::new(RwLock::new(CacheMetrics::default())),
        }
    }
}

impl LatencyHistogram {
    pub fn new() -> Self {
        Self {
            buckets: vec![
                (1.0, 0),     // < 1ms
                (5.0, 0),     // < 5ms
                (10.0, 0),    // < 10ms
                (25.0, 0),    // < 25ms
                (50.0, 0),    // < 50ms
                (100.0, 0),   // < 100ms
                (250.0, 0),   // < 250ms
                (500.0, 0),   // < 500ms
                (1000.0, 0),  // < 1s
                (2500.0, 0),  // < 2.5s
                (5000.0, 0),  // < 5s
                (f64::INFINITY, 0), // >= 5s
            ],
            count: 0,
            sum: 0.0,
        }
    }
    
    pub fn record(&mut self, value: f64) {
        self.count += 1;
        self.sum += value;
        
        for (upper_bound, count) in &mut self.buckets {
            if value <= *upper_bound {
                *count += 1;
                break;
            }
        }
    }
    
    pub fn percentile(&self, p: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        
        let target_count = (self.count as f64 * p / 100.0) as u64;
        let mut cumulative = 0;
        
        for (upper_bound, count) in &self.buckets {
            cumulative += count;
            if cumulative >= target_count {
                return *upper_bound;
            }
        }
        
        0.0
    }
}

impl SlowQueryLogger {
    pub fn new(config: Arc<MonitoringConfig>) -> Self {
        Self {
            slow_queries: Arc::new(Mutex::new(VecDeque::new())),
            config,
        }
    }
    
    pub fn log_slow_query(&self, record: SlowQueryRecord) {
        if let Ok(mut queries) = self.slow_queries.lock() {
            // Maintain circular buffer
            if queries.len() >= self.config.max_slow_queries {
                queries.pop_front();
            }
            queries.push_back(record);
        }
        
        // TODO: Also write to log file if configured
    }
    
    pub fn get_recent_slow_queries(&self, limit: Option<usize>) -> Vec<SlowQueryRecord> {
        if let Ok(queries) = self.slow_queries.lock() {
            let limit = limit.unwrap_or(queries.len());
            queries.iter()
                .rev()
                .take(limit)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }
}

impl GraphProfiler {
    pub fn new(config: Arc<MonitoringConfig>) -> Self {
        Self {
            active_profiles: Arc::new(RwLock::new(HashMap::new())),
            completed_profiles: Arc::new(RwLock::new(VecDeque::new())),
            config,
        }
    }
    
    pub fn start_session(&self, operation: &str) -> String {
        let session_id = Uuid::new_v4().to_string();
        
        let session = ProfileSession {
            session_id: session_id.clone(),
            start_time: Instant::now(),
            operation: operation.to_string(),
            timings: HashMap::new(),
            resource_samples: Vec::new(),
            metadata: HashMap::new(),
        };
        
        if let Ok(mut profiles) = self.active_profiles.write() {
            profiles.insert(session_id.clone(), session);
        }
        
        session_id
    }
    
    pub fn end_session(&self, session_id: &str) -> Option<ProfileSummary> {
        let session = if let Ok(mut profiles) = self.active_profiles.write() {
            profiles.remove(session_id)
        } else {
            return None;
        };
        
        if let Some(session) = session {
            let total_time = session.start_time.elapsed();
            let summary = ProfileSummary {
                session_id: session.session_id,
                operation: session.operation,
                total_time_ms: total_time.as_millis() as u64,
                time_breakdown: session.timings.iter()
                    .map(|(k, v)| (k.clone(), v.as_millis() as u64))
                    .collect(),
                peak_cpu_percent: session.resource_samples.iter()
                    .map(|s| s.cpu_percent)
                    .fold(0.0, f64::max),
                peak_memory_mb: session.resource_samples.iter()
                    .map(|s| s.memory_mb)
                    .fold(0.0, f64::max),
                avg_cpu_percent: if !session.resource_samples.is_empty() {
                    session.resource_samples.iter().map(|s| s.cpu_percent).sum::<f64>() / 
                    session.resource_samples.len() as f64
                } else { 0.0 },
                avg_memory_mb: if !session.resource_samples.is_empty() {
                    session.resource_samples.iter().map(|s| s.memory_mb).sum::<f64>() / 
                    session.resource_samples.len() as f64
                } else { 0.0 },
                sample_count: session.resource_samples.len(),
                completed_at: SystemTime::now(),
            };
            
            // Add to completed profiles
            if let Ok(mut completed) = self.completed_profiles.write() {
                if completed.len() >= 1000 { // Keep last 1000 profiles
                    completed.pop_front();
                }
                completed.push_back(summary.clone());
            }
            
            Some(summary)
        } else {
            None
        }
    }
    
    pub fn record_timing(&self, session_id: &str, checkpoint: &str, duration: Duration) {
        if let Ok(mut profiles) = self.active_profiles.write() {
            if let Some(session) = profiles.get_mut(session_id) {
                session.timings.insert(checkpoint.to_string(), duration);
            }
        }
    }
    
    pub fn get_recent_summaries(&self, limit: Option<usize>) -> Vec<ProfileSummary> {
        if let Ok(completed) = self.completed_profiles.read() {
            let limit = limit.unwrap_or(completed.len());
            completed.iter()
                .rev()
                .take(limit)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_monitoring_config_default() {
        let config = MonitoringConfig::default();
        assert!(config.enabled);
        assert_eq!(config.slow_query_threshold_ms, 1000);
        assert_eq!(config.max_slow_queries, 1000);
    }
    
    #[test]
    fn test_latency_histogram() {
        let mut histogram = LatencyHistogram::new();
        
        // Record some values
        histogram.record(5.0);
        histogram.record(15.0);
        histogram.record(150.0);
        
        assert_eq!(histogram.count, 3);
        assert_eq!(histogram.sum, 170.0);
        
        // Check percentiles (approximate)
        let p50 = histogram.percentile(50.0);
        assert!(p50 > 0.0);
    }
    
    #[test]
    fn test_graph_monitor_creation() {
        let config = MonitoringConfig::default();
        let monitor = GraphMonitor::new(config);
        
        assert!(monitor.config.enabled);
    }
    
    #[tokio::test]
    async fn test_health_check() {
        let config = MonitoringConfig::default();
        let monitor = GraphMonitor::new(config);
        let memory_pool = Arc::new(GraphMemoryPool::new());
        
        let health = monitor.health_check(&memory_pool).await;
        assert!(matches!(health.status, HealthStatus::Healthy | HealthStatus::Degraded));
    }
}