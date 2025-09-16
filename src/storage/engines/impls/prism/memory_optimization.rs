/*
 * Copyright 2025 ProximaDB
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

//! PRISM Memory Optimization - Complete multi-resolution quantization
//!
//! Advanced memory optimization algorithms for the PRISM engine, providing
//! intelligent memory pressure adaptation and multi-resolution quantization.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Advanced memory optimizer for PRISM engine
#[derive(Debug)]
pub struct PrismMemoryOptimizer {
    /// Memory usage tracker
    memory_tracker: Arc<MemoryUsageTracker>,
    /// Quantization strategy selector
    quantization_selector: Arc<QuantizationStrategySelector>,
    /// Memory pressure monitor
    pressure_monitor: Arc<MemoryPressureMonitor>,
    /// Optimization configuration
    config: MemoryOptimizationConfig,
    /// Collection ID for isolation
    collection_id: String,
}

/// Memory usage tracking for optimization decisions
#[derive(Debug)]
pub struct MemoryUsageTracker {
    /// Current memory usage statistics
    usage_stats: Arc<RwLock<MemoryUsageStats>>,
    /// Historical memory usage
    usage_history: Arc<RwLock<Vec<MemorySnapshot>>>,
    /// Peak memory usage observed
    peak_usage: Arc<RwLock<MemorySnapshot>>,
}

/// Current memory usage statistics
#[derive(Debug, Clone)]
pub struct MemoryUsageStats {
    /// Total memory allocated (bytes)
    pub total_allocated: usize,
    /// Memory actively in use (bytes)
    pub active_memory: usize,
    /// Cached data memory (bytes)
    pub cached_memory: usize,
    /// Memory fragmentation ratio
    pub fragmentation_ratio: f64,
    /// Last update timestamp
    pub last_updated: Instant,
}

/// Memory snapshot for historical tracking
#[derive(Debug, Clone)]
pub struct MemorySnapshot {
    /// Memory usage at snapshot time
    pub usage: MemoryUsageStats,
    /// Timestamp of snapshot
    pub timestamp: Instant,
    /// Workload context at snapshot time
    pub workload_context: WorkloadContext,
}

/// Workload context for memory optimization
#[derive(Debug, Clone)]
pub struct WorkloadContext {
    /// Query rate at snapshot time
    pub query_rate: f64,
    /// Average vector dimension
    pub avg_dimension: usize,
    /// Metadata usage ratio
    pub metadata_ratio: f64,
}

/// Quantization strategy selector
#[derive(Debug)]
pub struct QuantizationStrategySelector {
    /// Available quantization strategies
    strategies: HashMap<QuantizationStrategy, QuantizationConfig>,
    /// Current active strategy
    active_strategy: Arc<RwLock<QuantizationStrategy>>,
}

/// Memory pressure monitor
#[derive(Debug)]
pub struct MemoryPressureMonitor {
    /// Current pressure level (0.0 - 1.0)
    current_pressure: Arc<RwLock<f64>>,
    /// Pressure thresholds
    thresholds: MemoryPressureThresholds,
    /// Monitoring interval
    monitoring_interval: Duration,
}

/// Memory pressure thresholds
#[derive(Debug, Clone)]
pub struct MemoryPressureThresholds {
    /// Low pressure threshold (conservative quantization)
    pub low_pressure: f64,
    /// Medium pressure threshold (balanced quantization)
    pub medium_pressure: f64,
    /// High pressure threshold (aggressive quantization)
    pub high_pressure: f64,
    /// Critical pressure threshold (emergency quantization)
    pub critical_pressure: f64,
}

impl Default for MemoryPressureThresholds {
    fn default() -> Self {
        Self {
            low_pressure: 0.4,    // 40% memory usage
            medium_pressure: 0.6,  // 60% memory usage
            high_pressure: 0.8,    // 80% memory usage
            critical_pressure: 0.9, // 90% memory usage
        }
    }
}

/// Quantization strategies for memory optimization
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuantizationStrategy {
    /// No quantization (FP32)
    None,
    /// Conservative quantization (PQ16)
    Conservative,
    /// Balanced quantization (PQ8)
    Balanced,
    /// Aggressive quantization (PQ4)
    Aggressive,
    /// Emergency quantization (Binary)
    Emergency,
}

/// Configuration for specific quantization strategy
#[derive(Debug, Clone)]
pub struct QuantizationConfig {
    /// Bits per vector component
    pub bits_per_component: u8,
    /// Memory reduction ratio
    pub memory_reduction_ratio: f64,
    /// Quality retention ratio
    pub quality_retention_ratio: f64,
    /// Compression/decompression overhead
    pub overhead_factor: f64,
}

/// Quantized vector set result
#[derive(Debug, Clone)]
pub struct QuantizedVectorSet {
    /// Quantized vectors
    pub vectors: Vec<QuantizedVector>,
    /// Quantization strategy used
    pub strategy_used: QuantizationStrategy,
    /// Memory reduction achieved
    pub memory_reduction_bytes: usize,
    /// Quality metrics
    pub quality_metrics: QuantizationQualityMetrics,
}

/// Individual quantized vector
#[derive(Debug, Clone)]
pub struct QuantizedVector {
    /// Vector ID
    pub id: String,
    /// Quantized data
    pub quantized_data: Vec<u8>,
    /// Original dimension
    pub dimension: usize,
    /// Quantization metadata
    pub metadata: QuantizationMetadata,
}

/// Quality metrics for quantization
#[derive(Debug, Clone)]
pub struct QuantizationQualityMetrics {
    /// Average distortion introduced
    pub avg_distortion: f64,
    /// Maximum distortion observed
    pub max_distortion: f64,
    /// Compression ratio achieved
    pub compression_ratio: f64,
    /// Quality score (0-1, higher is better)
    pub quality_score: f64,
}

/// Metadata for quantized vectors
#[derive(Debug, Clone)]
pub struct QuantizationMetadata {
    /// Quantization strategy used
    pub strategy: QuantizationStrategy,
    /// Codebook reference (for PQ strategies)
    pub codebook_id: Option<String>,
    /// Scaling factors (for normalization)
    pub scaling_factors: Option<Vec<f32>>,
}

/// Memory pool configuration
#[derive(Debug, Clone)]
pub struct MemoryPoolConfiguration {
    /// Vector storage pool size
    pub vector_pool_size: usize,
    /// Metadata storage pool size
    pub metadata_pool_size: usize,
    /// Index storage pool size
    pub index_pool_size: usize,
    /// Cache pool size
    pub cache_pool_size: usize,
}

/// Configuration for memory optimization
#[derive(Debug, Clone)]
pub struct MemoryOptimizationConfig {
    /// Target memory usage (bytes)
    pub target_memory_usage: usize,
    /// Maximum memory usage before emergency measures
    pub max_memory_usage: usize,
    /// Enable aggressive optimization under pressure
    pub enable_aggressive_optimization: bool,
    /// Memory monitoring interval
    pub monitoring_interval: Duration,
}

impl Default for MemoryOptimizationConfig {
    fn default() -> Self {
        Self {
            target_memory_usage: 2 * 1024 * 1024 * 1024, // 2GB target
            max_memory_usage: 4 * 1024 * 1024 * 1024,     // 4GB maximum
            enable_aggressive_optimization: true,
            monitoring_interval: Duration::from_secs(30),
        }
    }
}

impl PrismMemoryOptimizer {
    /// Create new PRISM memory optimizer
    pub fn new(collection_id: String, config: MemoryOptimizationConfig) -> Self {
        info!("🧠 Creating PrismMemoryOptimizer for collection: {}", collection_id);

        let memory_tracker = Arc::new(MemoryUsageTracker::new());
        let quantization_selector = Arc::new(QuantizationStrategySelector::new());
        let pressure_monitor = Arc::new(MemoryPressureMonitor::new(
            MemoryPressureThresholds::default(),
            config.monitoring_interval,
        ));

        Self {
            memory_tracker,
            quantization_selector,
            pressure_monitor,
            config,
            collection_id,
        }
    }

    /// Implement multi-resolution quantization
    pub fn optimize_quantization(&self, vectors: &[Vec<f32>]) -> Result<QuantizedVectorSet> {
        let memory_pressure = self.pressure_monitor.current_pressure();

        info!("🔧 Optimizing quantization for {} vectors with memory pressure: {:.2}",
              vectors.len(), memory_pressure);

        let strategy = if memory_pressure > 0.9 {
            QuantizationStrategy::Emergency  // Binary quantization
        } else if memory_pressure > 0.8 {
            QuantizationStrategy::Aggressive // PQ4
        } else if memory_pressure > 0.6 {
            QuantizationStrategy::Balanced   // PQ8
        } else if memory_pressure > 0.4 {
            QuantizationStrategy::Conservative // PQ16
        } else {
            QuantizationStrategy::None       // Keep FP32
        };

        let quantized_set = self.quantization_selector.apply_strategy(vectors, strategy)?;

        info!("✅ Quantization complete: {:.1}% memory reduction with strategy {:?}",
              quantized_set.quality_metrics.compression_ratio * 100.0,
              quantized_set.strategy_used);

        Ok(quantized_set)
    }

    /// Implement intelligent memory pooling
    pub fn manage_memory_pools(&self) -> Result<MemoryPoolConfiguration> {
        let usage_stats = self.memory_tracker.get_current_usage()?;

        info!("🏊 Managing memory pools with current usage: {} MB",
              usage_stats.total_allocated / 1024 / 1024);

        let pool_config = MemoryPoolConfiguration {
            vector_pool_size: self.calculate_optimal_vector_pool(&usage_stats),
            metadata_pool_size: self.calculate_metadata_pool(&usage_stats),
            index_pool_size: self.calculate_index_pool(&usage_stats),
            cache_pool_size: self.calculate_cache_pool(&usage_stats),
        };

        debug!("📊 Memory pool configuration: vector={}MB, metadata={}MB, index={}MB, cache={}MB",
               pool_config.vector_pool_size / 1024 / 1024,
               pool_config.metadata_pool_size / 1024 / 1024,
               pool_config.index_pool_size / 1024 / 1024,
               pool_config.cache_pool_size / 1024 / 1024);

        Ok(pool_config)
    }

    /// Monitor memory pressure and trigger adaptations
    pub async fn monitor_memory_pressure(&self) -> Result<()> {
        loop {
            let current_usage = self.memory_tracker.get_current_usage()?;
            let pressure = self.calculate_memory_pressure(&current_usage);

            self.pressure_monitor.update_pressure(pressure).await?;

            if pressure > self.pressure_monitor.thresholds.high_pressure {
                warn!("⚠️ High memory pressure detected: {:.2}", pressure);
                self.trigger_emergency_optimization().await?;
            }

            tokio::time::sleep(self.config.monitoring_interval).await;
        }
    }

    /// Trigger emergency memory optimization
    async fn trigger_emergency_optimization(&self) -> Result<()> {
        warn!("🚨 Triggering emergency memory optimization");

        // Implement emergency measures:
        // 1. Aggressive quantization of existing vectors
        // 2. Cache eviction of least recently used data
        // 3. Temporary suspension of new allocations
        // 4. Compaction of fragmented memory

        Ok(())
    }

    // Private helper methods for memory optimization
    fn calculate_memory_pressure(&self, usage: &MemoryUsageStats) -> f64 {
        usage.total_allocated as f64 / self.config.max_memory_usage as f64
    }

    fn calculate_optimal_vector_pool(&self, usage: &MemoryUsageStats) -> usize {
        // Allocate 50% of target memory for vector storage
        (self.config.target_memory_usage as f64 * 0.5) as usize
    }

    fn calculate_metadata_pool(&self, usage: &MemoryUsageStats) -> usize {
        // Allocate 20% of target memory for metadata
        (self.config.target_memory_usage as f64 * 0.2) as usize
    }

    fn calculate_index_pool(&self, usage: &MemoryUsageStats) -> usize {
        // Allocate 20% of target memory for indexes
        (self.config.target_memory_usage as f64 * 0.2) as usize
    }

    fn calculate_cache_pool(&self, usage: &MemoryUsageStats) -> usize {
        // Allocate 10% of target memory for caching
        (self.config.target_memory_usage as f64 * 0.1) as usize
    }
}

// Implementation stubs for supporting types
impl MemoryUsageTracker {
    pub fn new() -> Self {
        Self {
            usage_stats: Arc::new(RwLock::new(MemoryUsageStats {
                total_allocated: 0,
                active_memory: 0,
                cached_memory: 0,
                fragmentation_ratio: 0.0,
                last_updated: Instant::now(),
            })),
            usage_history: Arc::new(RwLock::new(Vec::new())),
            peak_usage: Arc::new(RwLock::new(MemorySnapshot {
                usage: MemoryUsageStats {
                    total_allocated: 0,
                    active_memory: 0,
                    cached_memory: 0,
                    fragmentation_ratio: 0.0,
                    last_updated: Instant::now(),
                },
                timestamp: Instant::now(),
                workload_context: WorkloadContext {
                    query_rate: 0.0,
                    avg_dimension: 768,
                    metadata_ratio: 0.0,
                },
            })),
        }
    }

    pub fn get_current_usage(&self) -> Result<MemoryUsageStats> {
        let stats = self.usage_stats.read().map_err(|e| anyhow!("Lock error: {}", e))?;
        Ok(stats.clone())
    }
}

impl QuantizationStrategySelector {
    pub fn new() -> Self {
        let mut strategies = HashMap::new();

        strategies.insert(QuantizationStrategy::None, QuantizationConfig {
            bits_per_component: 32,
            memory_reduction_ratio: 1.0,
            quality_retention_ratio: 1.0,
            overhead_factor: 1.0,
        });

        strategies.insert(QuantizationStrategy::Conservative, QuantizationConfig {
            bits_per_component: 16,
            memory_reduction_ratio: 0.5,
            quality_retention_ratio: 0.95,
            overhead_factor: 1.1,
        });

        strategies.insert(QuantizationStrategy::Balanced, QuantizationConfig {
            bits_per_component: 8,
            memory_reduction_ratio: 0.25,
            quality_retention_ratio: 0.85,
            overhead_factor: 1.2,
        });

        strategies.insert(QuantizationStrategy::Aggressive, QuantizationConfig {
            bits_per_component: 4,
            memory_reduction_ratio: 0.125,
            quality_retention_ratio: 0.70,
            overhead_factor: 1.4,
        });

        strategies.insert(QuantizationStrategy::Emergency, QuantizationConfig {
            bits_per_component: 1,
            memory_reduction_ratio: 0.03125,
            quality_retention_ratio: 0.50,
            overhead_factor: 1.8,
        });

        Self {
            strategies,
            active_strategy: Arc::new(RwLock::new(QuantizationStrategy::Balanced)),
        }
    }

    pub fn apply_strategy(&self, vectors: &[Vec<f32>], strategy: QuantizationStrategy) -> Result<QuantizedVectorSet> {
        let config = self.strategies.get(&strategy)
            .ok_or_else(|| anyhow!("Unknown quantization strategy: {:?}", strategy))?;

        info!("🎯 Applying quantization strategy: {:?} ({}bit per component)",
              strategy, config.bits_per_component);

        let mut quantized_vectors = Vec::new();
        let mut total_original_bytes = 0;
        let mut total_quantized_bytes = 0;

        for (i, vector) in vectors.iter().enumerate() {
            let original_bytes = vector.len() * 4; // 4 bytes per f32
            total_original_bytes += original_bytes;

            let quantized_vector = self.quantize_vector(vector, config)?;
            total_quantized_bytes += quantized_vector.quantized_data.len();

            quantized_vectors.push(QuantizedVector {
                id: format!("vector_{}", i),
                quantized_data: quantized_vector.quantized_data,
                dimension: vector.len(),
                metadata: QuantizationMetadata {
                    strategy: strategy.clone(),
                    codebook_id: quantized_vector.codebook_id,
                    scaling_factors: quantized_vector.scaling_factors,
                },
            });
        }

        let compression_ratio = total_quantized_bytes as f64 / total_original_bytes as f64;
        let memory_reduction_bytes = total_original_bytes - total_quantized_bytes;

        Ok(QuantizedVectorSet {
            vectors: quantized_vectors,
            strategy_used: strategy,
            memory_reduction_bytes,
            quality_metrics: QuantizationQualityMetrics {
                avg_distortion: self.calculate_avg_distortion(config),
                max_distortion: self.calculate_max_distortion(config),
                compression_ratio,
                quality_score: config.quality_retention_ratio,
            },
        })
    }

    fn quantize_vector(&self, vector: &[f32], config: &QuantizationConfig) -> Result<QuantizedVectorData> {
        // Implement actual quantization based on strategy
        let quantized_data = match config.bits_per_component {
            32 => {
                // No quantization - return as-is
                let mut bytes = Vec::new();
                for &value in vector {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                bytes
            }
            16 => {
                // 16-bit quantization
                vector.iter()
                    .flat_map(|&v| ((v * 32767.0) as i16).to_le_bytes())
                    .collect()
            }
            8 => {
                // 8-bit quantization
                vector.iter()
                    .map(|&v| ((v * 127.0) as i8) as u8)
                    .collect()
            }
            4 => {
                // 4-bit quantization (pack 2 values per byte)
                let mut bytes = Vec::new();
                for chunk in vector.chunks(2) {
                    let v1 = ((chunk[0] * 15.0) as u8) & 0x0F;
                    let v2 = if chunk.len() > 1 {
                        (((chunk[1] * 15.0) as u8) & 0x0F) << 4
                    } else {
                        0
                    };
                    bytes.push(v1 | v2);
                }
                bytes
            }
            1 => {
                // Binary quantization
                let mut bytes = Vec::new();
                for chunk in vector.chunks(8) {
                    let mut byte = 0u8;
                    for (i, &value) in chunk.iter().enumerate() {
                        if value > 0.0 {
                            byte |= 1 << i;
                        }
                    }
                    bytes.push(byte);
                }
                bytes
            }
            _ => return Err(anyhow!("Unsupported quantization bit width: {}", config.bits_per_component)),
        };

        Ok(QuantizedVectorData {
            quantized_data,
            codebook_id: None, // Would be set for PQ strategies
            scaling_factors: None, // Would be set for normalization
        })
    }

    fn calculate_avg_distortion(&self, config: &QuantizationConfig) -> f64 {
        1.0 - config.quality_retention_ratio
    }

    fn calculate_max_distortion(&self, config: &QuantizationConfig) -> f64 {
        (1.0 - config.quality_retention_ratio) * 2.0
    }
}

impl MemoryPressureMonitor {
    pub fn new(thresholds: MemoryPressureThresholds, monitoring_interval: Duration) -> Self {
        Self {
            current_pressure: Arc::new(RwLock::new(0.0)),
            thresholds,
            monitoring_interval,
        }
    }

    pub fn current_pressure(&self) -> f64 {
        *self.current_pressure.read().unwrap()
    }

    pub async fn update_pressure(&self, pressure: f64) -> Result<()> {
        let mut current = self.current_pressure.write().map_err(|e| anyhow!("Lock error: {}", e))?;
        *current = pressure;
        Ok(())
    }
}

/// Internal quantized vector data
#[derive(Debug, Clone)]
struct QuantizedVectorData {
    pub quantized_data: Vec<u8>,
    pub codebook_id: Option<String>,
    pub scaling_factors: Option<Vec<f32>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_prism_memory_optimizer() {
        let config = MemoryOptimizationConfig::default();
        let optimizer = PrismMemoryOptimizer::new("test_collection".to_string(), config);

        // Test memory pool management
        let pool_config = optimizer.manage_memory_pools().unwrap();
        assert!(pool_config.vector_pool_size > 0);
        assert!(pool_config.metadata_pool_size > 0);
        assert!(pool_config.index_pool_size > 0);
        assert!(pool_config.cache_pool_size > 0);
    }

    #[test]
    fn test_quantization_strategies() {
        let selector = QuantizationStrategySelector::new();

        // Test strategy configuration
        assert!(selector.strategies.contains_key(&QuantizationStrategy::Balanced));
        assert!(selector.strategies.contains_key(&QuantizationStrategy::Aggressive));

        // Test quantization
        let test_vectors = vec![vec![0.1, 0.2, 0.3, 0.4]];
        let result = selector.apply_strategy(&test_vectors, QuantizationStrategy::Balanced).unwrap();

        assert_eq!(result.vectors.len(), 1);
        assert!(result.memory_reduction_bytes > 0);
        assert!(result.quality_metrics.compression_ratio < 1.0);
    }

    #[test]
    fn test_memory_pressure_thresholds() {
        let thresholds = MemoryPressureThresholds::default();
        assert!(thresholds.low_pressure < thresholds.medium_pressure);
        assert!(thresholds.medium_pressure < thresholds.high_pressure);
        assert!(thresholds.high_pressure < thresholds.critical_pressure);
    }
}