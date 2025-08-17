//! Cache metrics integration with ProximaDB's metrics framework

use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use anyhow::Result;

use crate::metrics::{
    InternalMetricsUpdater, AggregationWindow, MetricsAggregationEngine,
};
use crate::storage::cache::metrics::CacheMetrics as BaseCacheMetrics;
use crate::storage::cache::backend::CacheTier;

/// Cache metrics snapshot that work with the broader metrics framework
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetricsSnapshot {
    /// Cache hit rate across all tiers
    pub overall_hit_rate: f64,
    
    /// Per-tier metrics
    pub l1_metrics: TierMetrics,
    pub l2_metrics: TierMetrics,
    pub l3_metrics: TierMetrics,
    
    /// Memory usage metrics
    pub memory_usage: MemoryMetrics,
    
    /// Eviction metrics
    pub eviction_metrics: EvictionMetrics,
    
    /// Cache coordination metrics
    pub coordination_metrics: CoordinationMetrics,
    
    /// Timestamp of last update
    pub last_updated: SystemTime,
}

/// Per-tier cache metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierMetrics {
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub avg_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub entries: usize,
    pub size_bytes: usize,
}

/// Memory usage metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetrics {
    pub total_allocated_bytes: usize,
    pub used_bytes: usize,
    pub fragmentation_ratio: f64,
    pub allocation_failures: u64,
}

/// Eviction metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvictionMetrics {
    pub total_evictions: u64,
    pub lru_evictions: u64,
    pub lfu_evictions: u64,
    pub arc_evictions: u64,
    pub memory_pressure_evictions: u64,
    pub ttl_evictions: u64,
}

/// Cache coordination metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinationMetrics {
    pub cross_cache_hits: u64,
    pub prefetch_success_rate: f64,
    pub invalidation_cascades: u64,
    pub memory_rebalances: u64,
}

/// Cache metrics collector that integrates with the main metrics system
pub struct CacheMetricsCollector {
    /// Reference to the internal metrics updater
    updater: Arc<dyn InternalMetricsUpdater>,
    
    /// Cache-specific metrics aggregator
    aggregator: Arc<MetricsAggregationEngine>,
    
    /// Current metrics snapshot
    current_metrics: Arc<RwLock<CacheMetricsSnapshot>>,
    
    /// Base cache metrics from the cache subsystem
    base_metrics: Arc<BaseCacheMetrics>,
}

impl CacheMetricsCollector {
    /// Create a new cache metrics aggregator
    pub fn new(
        updater: Arc<dyn InternalMetricsUpdater>,
        aggregator: Arc<MetricsAggregationEngine>,
        base_metrics: Arc<BaseCacheMetrics>,
    ) -> Self {
        Self {
            updater,
            aggregator,
            current_metrics: Arc::new(RwLock::new(CacheMetricsSnapshot::default())),
            base_metrics,
        }
    }
    
    /// Start the periodic metrics collection and reporting
    pub async fn start(&self, interval: Duration) {
        let updater = self.updater.clone();
        let aggregator = self.aggregator.clone();
        let current_metrics = self.current_metrics.clone();
        let base_metrics = self.base_metrics.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval);
            
            loop {
                interval.tick().await;
                
                // Collect metrics from base cache metrics
                let metrics = Self::collect_metrics(&base_metrics).await;
                
                // Update current snapshot
                {
                    let mut current = current_metrics.write().await;
                    *current = metrics.clone();
                }
                
                // Send to main metrics system
                if let Err(e) = Self::report_to_metrics_system(
                    updater.as_ref(),
                    &aggregator,
                    &metrics
                ).await {
                    tracing::warn!("Failed to report cache metrics: {}", e);
                }
            }
        });
    }
    
    /// Collect metrics from the base cache metrics
    async fn collect_metrics(base_metrics: &BaseCacheMetrics) -> CacheMetricsSnapshot {
        let l1_hits = base_metrics.tier_hits(CacheTier::L1);
        let l1_misses = base_metrics.tier_misses(CacheTier::L1);
        let l1_total = l1_hits + l1_misses;
        
        let l2_hits = base_metrics.tier_hits(CacheTier::L2);
        let l2_misses = base_metrics.tier_misses(CacheTier::L2);
        let l2_total = l2_hits + l2_misses;
        
        let l3_hits = base_metrics.tier_hits(CacheTier::L3);
        let l3_misses = base_metrics.tier_misses(CacheTier::L3);
        let l3_total = l3_hits + l3_misses;
        
        CacheMetricsSnapshot {
            overall_hit_rate: base_metrics.hit_rate_percent(),
            
            l1_metrics: TierMetrics {
                hits: l1_hits,
                misses: l1_misses,
                hit_rate: if l1_total > 0 { l1_hits as f64 / l1_total as f64 } else { 0.0 },
                avg_latency_ms: base_metrics.avg_latency_ms(CacheTier::L1),
                p99_latency_ms: base_metrics.p99_latency_ms(CacheTier::L1),
                entries: base_metrics.tier_entries(CacheTier::L1),
                size_bytes: base_metrics.tier_size_bytes(CacheTier::L1),
            },
            
            l2_metrics: TierMetrics {
                hits: l2_hits,
                misses: l2_misses,
                hit_rate: if l2_total > 0 { l2_hits as f64 / l2_total as f64 } else { 0.0 },
                avg_latency_ms: base_metrics.avg_latency_ms(CacheTier::L2),
                p99_latency_ms: base_metrics.p99_latency_ms(CacheTier::L2),
                entries: base_metrics.tier_entries(CacheTier::L2),
                size_bytes: base_metrics.tier_size_bytes(CacheTier::L2),
            },
            
            l3_metrics: TierMetrics {
                hits: l3_hits,
                misses: l3_misses,
                hit_rate: if l3_total > 0 { l3_hits as f64 / l3_total as f64 } else { 0.0 },
                avg_latency_ms: base_metrics.avg_latency_ms(CacheTier::L3),
                p99_latency_ms: base_metrics.p99_latency_ms(CacheTier::L3),
                entries: base_metrics.tier_entries(CacheTier::L3),
                size_bytes: base_metrics.tier_size_bytes(CacheTier::L3),
            },
            
            memory_usage: MemoryMetrics {
                total_allocated_bytes: base_metrics.total_allocated_bytes(),
                used_bytes: base_metrics.used_bytes(),
                fragmentation_ratio: base_metrics.fragmentation_ratio(),
                allocation_failures: base_metrics.allocation_failures(),
            },
            
            eviction_metrics: EvictionMetrics {
                total_evictions: base_metrics.total_evictions(),
                lru_evictions: base_metrics.evictions_by_strategy("lru"),
                lfu_evictions: base_metrics.evictions_by_strategy("lfu"),
                arc_evictions: base_metrics.evictions_by_strategy("arc"),
                memory_pressure_evictions: base_metrics.memory_pressure_evictions(),
                ttl_evictions: base_metrics.ttl_evictions(),
            },
            
            coordination_metrics: CoordinationMetrics {
                cross_cache_hits: base_metrics.cross_cache_hits(),
                prefetch_success_rate: base_metrics.prefetch_success_rate(),
                invalidation_cascades: base_metrics.invalidation_cascades(),
                memory_rebalances: base_metrics.memory_rebalances(),
            },
            
            last_updated: SystemTime::now(),
        }
    }
    
    /// Report metrics to the main metrics system
    async fn report_to_metrics_system(
        _updater: &dyn InternalMetricsUpdater,
        aggregator: &MetricsAggregationEngine,
        _metrics: &CacheMetricsSnapshot,
    ) -> Result<()> {
        // Cache metrics are global, not per-collection
        // We'll aggregate them into the aggregator for different windows
        
        // Aggregate for different windows
        // Note: MetricsAggregationEngine.aggregate expects (collection_id, window, start_time, end_time)
        // For now, we'll use a placeholder implementation since cache metrics are global
        let now = chrono::Utc::now().timestamp_millis();
        let _ = aggregator.aggregate(
            "global_cache_info",
            AggregationWindow::Minute,
            now - 60000,
            now,
        )?;
        
        let _ = aggregator.aggregate(
            "global_cache_info",
            AggregationWindow::FiveMinutes,
            now - 300000,
            now,
        )?;
        
        let _ = aggregator.aggregate(
            "global_cache_info",
            AggregationWindow::Hour,
            now - 3600000,
            now,
        )?;
        
        Ok(())
    }
    
    /// Get current cache metrics snapshot
    pub async fn get_current_metrics(&self) -> CacheMetricsSnapshot {
        self.current_metrics.read().await.clone()
    }
    
    /// Get cache optimization hints based on metrics
    pub async fn get_optimization_hints(&self) -> CacheOptimizationHints {
        let metrics = self.current_metrics.read().await;
        
        CacheOptimizationHints {
            should_increase_l1_size: metrics.l1_metrics.hit_rate_percent < 0.7 && 
                                    metrics.memory_usage.used_bytes < metrics.memory_usage.total_allocated_bytes * 8 / 10,
            should_enable_l2: metrics.l1_metrics.hit_rate_percent < 0.5 && metrics.l2_metrics.entries == 0,
            should_enable_l3: metrics.l2_metrics.hit_rate_percent < 0.3 && metrics.l3_metrics.entries == 0,
            should_adjust_eviction: metrics.eviction_metrics.memory_pressure_evictions > 
                                   metrics.eviction_metrics.total_evictions / 2,
            should_enable_prefetching: metrics.coordination_metrics.prefetch_success_rate < 0.5,
            recommended_memory_mb: Self::calculate_recommended_memory(&metrics),
        }
    }
    
    fn calculate_recommended_memory(metrics: &CacheMetricsSnapshot) -> usize {
        // Simple heuristic: if hit rate is low and we're using most memory, recommend more
        let current_mb = metrics.memory_usage.used_bytes / (1024 * 1024);
        
        // If no memory is allocated/used yet, recommend a default amount
        if current_mb == 0 {
            // Default recommendation based on hit rate
            if metrics.overall_hit_rate < 0.5 {
                256 // Start with 256MB if hit rate is low
            } else {
                128 // Start with 128MB for reasonable hit rate
            }
        } else if metrics.overall_hit_rate < 0.6 && 
           metrics.memory_usage.used_bytes > metrics.memory_usage.total_allocated_bytes * 9 / 10 {
            // Recommend 50% more memory
            current_mb * 3 / 2
        } else if metrics.overall_hit_rate > 0.9 && 
                  metrics.memory_usage.used_bytes < metrics.memory_usage.total_allocated_bytes / 2 {
            // Can reduce memory by 25%
            current_mb * 3 / 4
        } else {
            current_mb.max(1) // At least 1MB
        }
    }
}

/// Cache optimization hints based on metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheOptimizationHints {
    pub should_increase_l1_size: bool,
    pub should_enable_l2: bool,
    pub should_enable_l3: bool,
    pub should_adjust_eviction: bool,
    pub should_enable_prefetching: bool,
    pub recommended_memory_mb: usize,
}

impl Default for CacheMetricsSnapshot {
    fn default() -> Self {
        Self {
            overall_hit_rate: 0.0,
            l1_metrics: TierMetrics::default(),
            l2_metrics: TierMetrics::default(),
            l3_metrics: TierMetrics::default(),
            memory_usage: MemoryMetrics::default(),
            eviction_metrics: EvictionMetrics::default(),
            coordination_metrics: CoordinationMetrics::default(),
            last_updated: SystemTime::now(),
        }
    }
}

impl Default for TierMetrics {
    fn default() -> Self {
        Self {
            hits: 0,
            misses: 0,
            hit_rate: 0.0,
            avg_latency_ms: 0.0,
            p99_latency_ms: 0.0,
            entries: 0,
            size_bytes: 0,
        }
    }
}

impl Default for MemoryMetrics {
    fn default() -> Self {
        Self {
            total_allocated_bytes: 0,
            used_bytes: 0,
            fragmentation_ratio: 0.0,
            allocation_failures: 0,
        }
    }
}

impl Default for EvictionMetrics {
    fn default() -> Self {
        Self {
            total_evictions: 0,
            lru_evictions: 0,
            lfu_evictions: 0,
            arc_evictions: 0,
            memory_pressure_evictions: 0,
            ttl_evictions: 0,
        }
    }
}

impl Default for CoordinationMetrics {
    fn default() -> Self {
        Self {
            cross_cache_hits: 0,
            prefetch_success_rate: 0.0,
            invalidation_cascades: 0,
            memory_rebalances: 0,
        }
    }
}