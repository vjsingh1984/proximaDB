//! Cache monitoring and observability dashboard

use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::collections::HashMap;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};

use crate::storage::cache::{
    CrossCacheOrchestrator, CacheType,
};
use crate::storage::cache::config::CacheConfig;
use crate::metrics::CacheMetricsSnapshot;

/// Cache monitoring dashboard for real-time insights
pub struct CacheMonitoringDashboard {
    /// Cache orchestrator reference
    orchestrator: Arc<CrossCacheOrchestrator>,
    
    /// Configuration
    config: Arc<CacheConfig>,
    
    /// Historical metrics
    history: Arc<RwLock<MetricsHistory>>,
    
    /// Alert manager
    alert_manager: Arc<AlertManager>,
    
    /// Performance profiler
    profiler: Option<Arc<PerformanceProfiler>>,
}

/// Historical metrics storage
#[derive(Debug, Clone, Default)]
struct MetricsHistory {
    /// Time series data points
    time_series: Vec<MetricsSnapshot>,
    
    /// Maximum history size
    max_size: usize,
}

/// Point-in-time metrics snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetricsSnapshot {
    timestamp: SystemTime,
    cache_metrics: CacheMetricsSnapshot,
    memory_pressure: f64,
    cpu_usage: f64,
    io_wait: f64,
}

/// Alert manager for threshold monitoring
pub struct AlertManager {
    /// Active alerts
    active_alerts: Arc<RwLock<Vec<Alert>>>,
    
    /// Alert thresholds from config
    thresholds: AlertThresholds,
    
    /// Alert handlers
    handlers: Vec<Box<dyn AlertHandler + Send + Sync>>,
}

/// Individual alert
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub severity: AlertSeverity,
    pub message: String,
    pub triggered_at: SystemTime,
    pub cache_type: Option<CacheType>,
    pub metric_value: f64,
    pub threshold: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

/// Alert thresholds configuration
#[derive(Debug, Clone)]
pub struct AlertThresholds {
    min_hit_rate: f64,
    max_memory_usage: f64,
    max_eviction_rate: f64,
    max_cascade_size: usize,
    max_prefetch_queue: usize,
}

/// Alert handler trait
trait AlertHandler: Send + Sync {
    fn handle_alert(&self, alert: &Alert);
}

/// Performance profiler for detailed analysis
pub struct PerformanceProfiler {
    /// Profiling data
    profiles: Arc<RwLock<Vec<PerformanceProfile>>>,
    
    /// Sampling configuration
    sampling_rate: f64,
    
    /// Output path for profiles
    output_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PerformanceProfile {
    timestamp: SystemTime,
    operation: String,
    duration_ms: f64,
    cache_type: CacheType,
    tier: String,
    hit: bool,
    value_size: usize,
}

impl CacheMonitoringDashboard {
    /// Create new monitoring dashboard
    pub fn new(
        orchestrator: Arc<CrossCacheOrchestrator>,
        config: Arc<CacheConfig>,
    ) -> Self {
        let alert_manager = Arc::new(AlertManager::new(&config.monitoring.alert_thresholds));
        
        let profiler = if config.monitoring.enable_profiling {
            Some(Arc::new(PerformanceProfiler::new(
                config.monitoring.trace_sampling_rate,
                config.monitoring.profile_output_path.clone().unwrap_or_else(|| "/tmp/cache_profiles".to_string()),
            )))
        } else {
            None
        };
        
        Self {
            orchestrator,
            config,
            history: Arc::new(RwLock::new(MetricsHistory {
                time_series: Vec::new(),
                max_size: 1000,
            })),
            alert_manager,
            profiler,
        }
    }
    
    /// Start monitoring loop
    pub async fn start(&self) {
        let interval = Duration::from_secs(self.config.monitoring.metrics_interval_seconds);
        let orchestrator = self.orchestrator.clone();
        let history = self.history.clone();
        let alert_manager = self.alert_manager.clone();
        let config = self.config.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval);
            
            loop {
                interval.tick().await;
                
                // Collect metrics - convert from CacheMetrics to CacheMetricsSnapshot
                let cache_metrics = orchestrator.metrics();
                use crate::metrics::cache::{TierMetrics, MemoryMetrics, EvictionMetrics, CoordinationMetrics};
                let metrics = CacheMetricsSnapshot {
                    overall_hit_rate: cache_metrics.hit_rate(),
                    l1_metrics: TierMetrics {
                        hits: cache_metrics.tier_hits(crate::storage::cache::backend::CacheTier::L1),
                        misses: cache_metrics.tier_misses(crate::storage::cache::backend::CacheTier::L1),
                        hit_rate: cache_metrics.tier_hit_rate(crate::storage::cache::backend::CacheTier::L1),
                        avg_latency_ms: cache_metrics.avg_latency_ms(crate::storage::cache::backend::CacheTier::L1),
                        p99_latency_ms: cache_metrics.p99_latency_ms(crate::storage::cache::backend::CacheTier::L1),
                        entries: cache_metrics.tier_entries(crate::storage::cache::backend::CacheTier::L1),
                        size_bytes: cache_metrics.tier_size_bytes(crate::storage::cache::backend::CacheTier::L1),
                    },
                    l2_metrics: TierMetrics {
                        hits: cache_metrics.tier_hits(crate::storage::cache::backend::CacheTier::L2),
                        misses: cache_metrics.tier_misses(crate::storage::cache::backend::CacheTier::L2),
                        hit_rate: cache_metrics.tier_hit_rate(crate::storage::cache::backend::CacheTier::L2),
                        avg_latency_ms: cache_metrics.avg_latency_ms(crate::storage::cache::backend::CacheTier::L2),
                        p99_latency_ms: cache_metrics.p99_latency_ms(crate::storage::cache::backend::CacheTier::L2),
                        entries: cache_metrics.tier_entries(crate::storage::cache::backend::CacheTier::L2),
                        size_bytes: cache_metrics.tier_size_bytes(crate::storage::cache::backend::CacheTier::L2),
                    },
                    l3_metrics: TierMetrics {
                        hits: cache_metrics.tier_hits(crate::storage::cache::backend::CacheTier::L3),
                        misses: cache_metrics.tier_misses(crate::storage::cache::backend::CacheTier::L3),
                        hit_rate: cache_metrics.tier_hit_rate(crate::storage::cache::backend::CacheTier::L3),
                        avg_latency_ms: cache_metrics.avg_latency_ms(crate::storage::cache::backend::CacheTier::L3),
                        p99_latency_ms: cache_metrics.p99_latency_ms(crate::storage::cache::backend::CacheTier::L3),
                        entries: cache_metrics.tier_entries(crate::storage::cache::backend::CacheTier::L3),
                        size_bytes: cache_metrics.tier_size_bytes(crate::storage::cache::backend::CacheTier::L3),
                    },
                    memory_usage: MemoryMetrics {
                        total_allocated_bytes: cache_metrics.total_allocated_bytes(),
                        used_bytes: cache_metrics.used_bytes(),
                        fragmentation_ratio: cache_metrics.fragmentation_ratio(),
                        allocation_failures: cache_metrics.allocation_failures(),
                    },
                    eviction_metrics: EvictionMetrics {
                        total_evictions: cache_metrics.total_evictions(),
                        lru_evictions: cache_metrics.lru_evictions(),
                        lfu_evictions: cache_metrics.lfu_evictions(),
                        arc_evictions: cache_metrics.arc_evictions(),
                        memory_pressure_evictions: cache_metrics.memory_pressure_evictions(),
                        ttl_evictions: cache_metrics.ttl_evictions(),
                    },
                    coordination_metrics: CoordinationMetrics {
                        cross_cache_hits: cache_metrics.cross_cache_hits(),
                        prefetch_success_rate: cache_metrics.prefetch_success_rate(),
                        invalidation_cascades: cache_metrics.invalidation_cascades(),
                        memory_rebalances: cache_metrics.memory_rebalances(),
                    },
                    last_updated: SystemTime::now(),
                };
                
                // Create snapshot
                let snapshot = MetricsSnapshot {
                    timestamp: SystemTime::now(),
                    cache_metrics: metrics.clone(),
                    memory_pressure: Self::get_memory_pressure(),
                    cpu_usage: Self::get_cpu_usage(),
                    io_wait: Self::get_io_wait(),
                };
                
                // Store in history
                {
                    let mut hist = history.write().await;
                    hist.time_series.push(snapshot.clone());
                    if hist.time_series.len() > hist.max_size {
                        hist.time_series.remove(0);
                    }
                }
                
                // Check alerts
                alert_manager.check_alerts(&metrics, &config.monitoring.alert_thresholds).await;
            }
        });
    }
    
    /// Get current dashboard state
    pub async fn get_dashboard_state(&self) -> DashboardState {
        let history = self.history.read().await;
        let alerts = self.alert_manager.active_alerts.read().await;
        // Convert from CacheMetrics to CacheMetricsSnapshot
        let cache_metrics = self.orchestrator.metrics();
        use crate::metrics::cache::{TierMetrics, MemoryMetrics, EvictionMetrics, CoordinationMetrics};
        let metrics = CacheMetricsSnapshot {
            overall_hit_rate: cache_metrics.hit_rate_percent(),
            l1_metrics: TierMetrics {
                hits: cache_metrics.tier_hits(crate::storage::cache::backend::CacheTier::L1),
                misses: cache_metrics.tier_misses(crate::storage::cache::backend::CacheTier::L1),
                hit_rate: cache_metrics.tier_hit_rate(crate::storage::cache::backend::CacheTier::L1),
                avg_latency_ms: cache_metrics.avg_latency_ms(crate::storage::cache::backend::CacheTier::L1),
                p99_latency_ms: cache_metrics.p99_latency_ms(crate::storage::cache::backend::CacheTier::L1),
                entries: cache_metrics.tier_entries(crate::storage::cache::backend::CacheTier::L1),
                size_bytes: cache_metrics.tier_size_bytes(crate::storage::cache::backend::CacheTier::L1),
            },
            l2_metrics: TierMetrics {
                hits: cache_metrics.tier_hits(crate::storage::cache::backend::CacheTier::L2),
                misses: cache_metrics.tier_misses(crate::storage::cache::backend::CacheTier::L2),
                hit_rate: cache_metrics.tier_hit_rate(crate::storage::cache::backend::CacheTier::L2),
                avg_latency_ms: cache_metrics.avg_latency_ms(crate::storage::cache::backend::CacheTier::L2),
                p99_latency_ms: cache_metrics.p99_latency_ms(crate::storage::cache::backend::CacheTier::L2),
                entries: cache_metrics.tier_entries(crate::storage::cache::backend::CacheTier::L2),
                size_bytes: cache_metrics.tier_size_bytes(crate::storage::cache::backend::CacheTier::L2),
            },
            l3_metrics: TierMetrics {
                hits: cache_metrics.tier_hits(crate::storage::cache::backend::CacheTier::L3),
                misses: cache_metrics.tier_misses(crate::storage::cache::backend::CacheTier::L3),
                hit_rate: cache_metrics.tier_hit_rate(crate::storage::cache::backend::CacheTier::L3),
                avg_latency_ms: cache_metrics.avg_latency_ms(crate::storage::cache::backend::CacheTier::L3),
                p99_latency_ms: cache_metrics.p99_latency_ms(crate::storage::cache::backend::CacheTier::L3),
                entries: cache_metrics.tier_entries(crate::storage::cache::backend::CacheTier::L3),
                size_bytes: cache_metrics.tier_size_bytes(crate::storage::cache::backend::CacheTier::L3),
            },
            memory_usage: MemoryMetrics {
                total_allocated_bytes: cache_metrics.total_allocated_bytes(),
                used_bytes: cache_metrics.used_bytes(),
                fragmentation_ratio: cache_metrics.fragmentation_ratio(),
                allocation_failures: cache_metrics.allocation_failures(),
            },
            eviction_metrics: EvictionMetrics {
                total_evictions: cache_metrics.total_evictions(),
                lru_evictions: cache_metrics.lru_evictions(),
                lfu_evictions: cache_metrics.lfu_evictions(),
                arc_evictions: cache_metrics.arc_evictions(),
                memory_pressure_evictions: cache_metrics.memory_pressure_evictions(),
                ttl_evictions: cache_metrics.ttl_evictions(),
            },
            coordination_metrics: CoordinationMetrics {
                cross_cache_hits: cache_metrics.cross_cache_hits(),
                prefetch_success_rate: cache_metrics.prefetch_success_rate(),
                invalidation_cascades: cache_metrics.invalidation_cascades(),
                memory_rebalances: cache_metrics.memory_rebalances(),
            },
            last_updated: SystemTime::now(),
        };
        
        DashboardState {
            current_metrics: metrics.clone(),
            active_alerts: alerts.clone(),
            recent_history: history.time_series.iter()
                .rev()
                .take(100)
                .cloned()
                .collect(),
            cache_status: self.get_cache_status().await,
            optimization_suggestions: self.get_optimization_suggestions(&metrics).await,
        }
    }
    
    /// Get cache status for each cache type
    async fn get_cache_status(&self) -> HashMap<String, CacheStatus> {
        let mut status = HashMap::new();
        
        status.insert("vector_data".to_string(), CacheStatus {
            enabled: self.config.vector_data.enabled,
            memory_allocated_mb: self.config.global.total_memory_mb * self.config.vector_data.memory_percentage as usize / 100,
            entries: 0, // Would get from actual cache
            hit_rate: 0.0,
        });
        
        status.insert("query_result".to_string(), CacheStatus {
            enabled: self.config.query_result.enabled,
            memory_allocated_mb: self.config.global.total_memory_mb * self.config.query_result.memory_percentage as usize / 100,
            entries: 0,
            hit_rate: 0.0,
        });
        
        status
    }
    
    /// Get optimization suggestions based on metrics
    async fn get_optimization_suggestions(&self, metrics: &CacheMetricsSnapshot) -> Vec<String> {
        let mut suggestions = Vec::new();
        
        if metrics.overall_hit_rate < 0.5 {
            suggestions.push("Consider increasing cache memory allocation".to_string());
        }
        
        if metrics.l1_metrics.hit_rate < 0.7 && self.config.global.enable_tiered_storage {
            suggestions.push("L1 hit rate is low, consider adjusting tier thresholds".to_string());
        }
        
        if metrics.eviction_metrics.memory_pressure_evictions > metrics.eviction_metrics.total_evictions / 2 {
            suggestions.push("High memory pressure evictions, increase total memory budget".to_string());
        }
        
        if metrics.coordination_metrics.prefetch_success_rate < 0.3 {
            suggestions.push("Low prefetch success rate, consider tuning pattern analysis".to_string());
        }
        
        suggestions
    }
    
    // System metrics collection (simplified)
    fn get_memory_pressure() -> f64 {
        // Would use actual system metrics
        0.5
    }
    
    fn get_cpu_usage() -> f64 {
        // Would use actual system metrics
        0.3
    }
    
    fn get_io_wait() -> f64 {
        // Would use actual system metrics
        0.1
    }
}

/// Dashboard state for API responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardState {
    pub current_metrics: CacheMetricsSnapshot,
    pub active_alerts: Vec<Alert>,
    pub recent_history: Vec<MetricsSnapshot>,
    pub cache_status: HashMap<String, CacheStatus>,
    pub optimization_suggestions: Vec<String>,
}

/// Individual cache status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStatus {
    pub enabled: bool,
    pub memory_allocated_mb: usize,
    pub entries: usize,
    pub hit_rate: f64,
}

impl AlertManager {
    pub fn new(thresholds: &crate::storage::cache::config::AlertThresholds) -> Self {
        Self {
            active_alerts: Arc::new(RwLock::new(Vec::new())),
            thresholds: AlertThresholds {
                min_hit_rate: thresholds.min_hit_rate,
                max_memory_usage: thresholds.max_memory_usage,
                max_eviction_rate: thresholds.max_eviction_rate,
                max_cascade_size: thresholds.max_cascade_size,
                max_prefetch_queue: thresholds.max_prefetch_queue,
            },
            handlers: Vec::new(),
        }
    }
    
    pub async fn get_active_alerts(&self) -> Vec<Alert> {
        self.active_alerts.read().await.clone()
    }
    
    pub async fn check_alerts(&self, metrics: &CacheMetricsSnapshot, thresholds: &crate::storage::cache::config::AlertThresholds) {
        let mut alerts = self.active_alerts.write().await;
        alerts.clear();
        
        // Check hit rate
        if metrics.overall_hit_rate < thresholds.min_hit_rate {
            alerts.push(Alert {
                id: "low_hit_rate".to_string(),
                severity: AlertSeverity::Warning,
                message: format!("Cache hit rate {} below threshold {}", 
                    metrics.overall_hit_rate, thresholds.min_hit_rate),
                triggered_at: SystemTime::now(),
                cache_type: None,
                metric_value: metrics.overall_hit_rate,
                threshold: thresholds.min_hit_rate,
            });
        }
        
        // Check memory usage
        let memory_usage = metrics.memory_usage.used_bytes as f64 / 
                          metrics.memory_usage.total_allocated_bytes as f64;
        if memory_usage > thresholds.max_memory_usage {
            alerts.push(Alert {
                id: "high_memory_usage".to_string(),
                severity: AlertSeverity::Critical,
                message: format!("Memory usage {} exceeds threshold {}", 
                    memory_usage, thresholds.max_memory_usage),
                triggered_at: SystemTime::now(),
                cache_type: None,
                metric_value: memory_usage,
                threshold: thresholds.max_memory_usage,
            });
        }
    }
}

impl PerformanceProfiler {
    pub fn new(sampling_rate: f64, output_path: String) -> Self {
        Self {
            profiles: Arc::new(RwLock::new(Vec::new())),
            sampling_rate,
            output_path,
        }
    }
    
    pub async fn record_operation(
        &self,
        operation: String,
        cache_type: CacheType,
        tier: String,
        duration_ms: f64,
        hit: bool,
        value_size: usize,
    ) {
        // Sample based on rate
        if rand::random::<f64>() > self.sampling_rate {
            return;
        }
        
        let profile = PerformanceProfile {
            timestamp: SystemTime::now(),
            operation,
            duration_ms,
            cache_type,
            tier,
            hit,
            value_size,
        };
        
        let mut profiles = self.profiles.write().await;
        profiles.push(profile);
        
        // Periodically write to disk
        if profiles.len() >= 1000 {
            self.flush_to_disk(&profiles).await;
            profiles.clear();
        }
    }
    
    async fn flush_to_disk(&self, profiles: &[PerformanceProfile]) {
        // Would write to actual file
        let _json = serde_json::to_string_pretty(profiles).unwrap();
        // std::fs::write(&self.output_path, json).ok();
    }
}

use rand;
