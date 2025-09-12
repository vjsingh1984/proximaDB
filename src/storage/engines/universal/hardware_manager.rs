//! Hardware Acceleration Manager for Universal Adapter
//!
//! This module manages hardware acceleration capabilities and optimization strategies
//! for the universal distance adapter system.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::core::hardware_capabilities::HardwareCapabilities;

use super::{AdapterError, AdapterResult, config::HardwareAccelerationConfig};

/// Hardware acceleration manager for the universal adapter
#[derive(Debug)]
pub struct HardwareAccelerationManager {
    /// Hardware capabilities detected at runtime
    capabilities: HardwareCapabilities,

    /// Configuration for hardware acceleration
    config: HardwareAccelerationConfig,

    /// Current optimization strategy
    current_strategy: OptimizationStrategy,

    /// SIMD capabilities
    simd_capabilities: SIMDCapabilities,

    /// Performance statistics
    stats: HardwarePerformanceStats,
}

/// SIMD capabilities available on the system
#[derive(Debug, Clone)]
pub struct SIMDCapabilities {
    /// SSE support
    pub sse_supported: bool,

    /// SSE2 support
    pub sse2_supported: bool,

    /// AVX support
    pub avx_supported: bool,

    /// AVX2 support
    pub avx2_supported: bool,

    /// AVX-512 support
    pub avx512_supported: bool,

    /// ARM NEON support
    pub neon_supported: bool,

    /// Population count instruction support
    pub popcnt_supported: bool,

    /// Bit manipulation instruction support
    pub bmi_supported: bool,

    /// Vector register width in bits
    pub vector_register_width: usize,

    /// Maximum SIMD lane count for different data types
    pub max_f32_lanes: usize,
    pub max_i32_lanes: usize,
    pub max_i8_lanes: usize,
}

/// Hardware acceleration capabilities
#[derive(Debug, Clone)]
pub struct AccelerationCapabilities {
    /// SIMD capabilities
    pub simd: SIMDCapabilities,

    /// CPU core count
    pub cpu_cores: usize,

    /// L1 cache size per core in KB
    pub l1_cache_size_kb: usize,

    /// L2 cache size per core in KB
    pub l2_cache_size_kb: usize,

    /// L3 cache size in KB
    pub l3_cache_size_kb: usize,

    /// Memory bandwidth in GB/s
    pub memory_bandwidth_gbps: f32,

    /// CPU frequency in GHz
    pub cpu_frequency_ghz: f32,
}

/// Optimization strategies for hardware acceleration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OptimizationStrategy {
    /// Use scalar operations only
    Scalar,

    /// Use SIMD instructions
    SIMD,

    /// Use vectorized operations with batching
    Vectorized,

    /// Use parallel processing across cores
    Parallel,

    /// Use hybrid approach combining multiple strategies
    Hybrid,

    /// Adaptive strategy based on workload
    Adaptive,
}

/// Performance statistics for hardware acceleration
#[derive(Debug, Clone, Default)]
pub struct HardwarePerformanceStats {
    /// Total operations performed
    pub total_operations: u64,

    /// Operations using SIMD acceleration
    pub simd_operations: u64,

    /// Operations using parallel processing
    pub parallel_operations: u64,

    /// Average operation time in microseconds
    pub average_operation_time_us: u64,

    /// SIMD acceleration speedup ratio
    pub simd_speedup_ratio: f32,

    /// Parallel processing speedup ratio
    pub parallel_speedup_ratio: f32,

    /// Memory bandwidth utilization (0.0-1.0)
    pub memory_bandwidth_utilization: f32,

    /// Cache hit rate (0.0-1.0)
    pub cache_hit_rate: f32,

    /// Acceleration usage rate (0.0-1.0)
    pub acceleration_usage_rate: f32,
}

impl HardwareAccelerationManager {
    /// Create a new hardware acceleration manager
    pub async fn new(
        config: &HardwareAccelerationConfig,
        capabilities: &HardwareCapabilities,
    ) -> AdapterResult<Self> {
        info!("Initializing hardware acceleration manager");

        let simd_capabilities = Self::detect_simd_capabilities(capabilities)?;
        let current_strategy = Self::select_optimal_strategy(config, &simd_capabilities)?;

        debug!("Detected SIMD capabilities: {:?}", simd_capabilities);
        debug!(
            "Selected optimization // strategy removed -  {:?}",
            current_strategy
        );

        Ok(Self {
            capabilities: capabilities.clone(),
            config: config.clone(),
            current_strategy,
            simd_capabilities,
            stats: HardwarePerformanceStats::default(),
        })
    }

    /// Get the current optimization strategy
    pub fn get_optimization_strategy(&self) -> OptimizationStrategy {
        self.current_strategy
    }

    /// Update optimization strategy based on workload characteristics
    pub async fn update_strategy(
        &mut self,
        workload_characteristics: &WorkloadCharacteristics,
    ) -> AdapterResult<()> {
        let new_strategy = self.select_strategy_for_workload(workload_characteristics)?;

        if new_strategy != self.current_strategy {
            debug!(
                "Updating optimization strategy from {:?} to {:?}",
                self.current_strategy, new_strategy
            );
            self.current_strategy = new_strategy;
        }

        Ok(())
    }

    /// Check if SIMD acceleration is available for given vector size
    pub fn is_simd_available(&self, vector_size: usize) -> bool {
        if !self.config.enable_simd {
            return false;
        }

        if vector_size < self.config.min_vector_size_for_acceleration {
            return false;
        }

        self.simd_capabilities.sse2_supported
            || self.simd_capabilities.avx_supported
            || self.simd_capabilities.neon_supported
    }

    /// Get optimal SIMD lane count for given data type
    pub fn get_optimal_simd_lanes(&self, data_type: SIMDData) -> usize {
        match data_type {
            SIMDData::F32 => {
                if self.simd_capabilities.avx512_supported && self.config.enable_avx512 {
                    16 // AVX-512 supports 16 f32 values
                } else if self.simd_capabilities.avx2_supported && self.config.enable_avx2 {
                    8 // AVX2 supports 8 f32 values
                } else if self.simd_capabilities.sse2_supported {
                    4 // SSE2 supports 4 f32 values
                } else if self.simd_capabilities.neon_supported && self.config.enable_neon {
                    4 // NEON supports 4 f32 values
                } else {
                    1 // Scalar fallback
                }
            }
            SIMDData::I32 => self.get_optimal_simd_lanes(SIMDData::F32), // Same as F32
            SIMDData::I8 => {
                if self.simd_capabilities.avx512_supported && self.config.enable_avx512 {
                    64 // AVX-512 supports 64 i8 values
                } else if self.simd_capabilities.avx2_supported && self.config.enable_avx2 {
                    32 // AVX2 supports 32 i8 values
                } else if self.simd_capabilities.sse2_supported {
                    16 // SSE2 supports 16 i8 values
                } else if self.simd_capabilities.neon_supported && self.config.enable_neon {
                    16 // NEON supports 16 i8 values
                } else {
                    1 // Scalar fallback
                }
            }
            SIMDData::U8 => self.get_optimal_simd_lanes(SIMDData::I8), // Same as I8
        }
    }

    /// Execute distance computation with hardware acceleration
    pub async fn execute_accelerated_computation<T>(
        &mut self,
        computation: T,
        vector_size: usize,
        batch_size: usize,
    ) -> AdapterResult<T::Output>
    where
        T: AcceleratedComputation,
    {
        let start_time = std::time::Instant::now();

        // Select optimal strategy for this computation
        let strategy = self.select_computation_strategy(vector_size, batch_size)?;

        // Execute computation with selected strategy
        let result = match strategy {
            OptimizationStrategy::SIMD => {
                self.stats.simd_operations += 1;
                computation.execute_simd(&self.simd_capabilities).await
            }
            OptimizationStrategy::Parallel => {
                self.stats.parallel_operations += 1;
                computation
                    .execute_parallel(self.capabilities.cpu.physical_cores)
                    .await
            }
            OptimizationStrategy::Hybrid => {
                self.stats.simd_operations += 1;
                self.stats.parallel_operations += 1;
                computation
                    .execute_hybrid(
                        &self.simd_capabilities,
                        self.capabilities.cpu.physical_cores,
                    )
                    .await
            }
            _ => computation.execute_scalar().await,
        }
        .map_err(|e| {
            AdapterError::HardwareAcceleration(format!("Accelerated computation failed: {}", e))
        })?;

        // Update performance statistics
        let operation_time = start_time.elapsed().as_micros() as u64;
        self.update_performance_stats(operation_time, strategy)
            .await;

        Ok(result)
    }

    /// Get hardware performance statistics
    pub async fn get_statistics(&self) -> HardwarePerformanceStats {
        let mut stats = self.stats.clone();

        // Calculate derived statistics
        if stats.total_operations > 0 {
            stats.acceleration_usage_rate = (stats.simd_operations + stats.parallel_operations)
                as f32
                / stats.total_operations as f32;
        }

        stats
    }

    /// Get acceleration capabilities
    pub fn get_capabilities(&self) -> AccelerationCapabilities {
        AccelerationCapabilities {
            simd: self.simd_capabilities.clone(),
            cpu_cores: self.capabilities.cpu.physical_cores,
            l1_cache_size_kb: 32,        // Typical L1 cache size
            l2_cache_size_kb: 256,       // Typical L2 cache size
            l3_cache_size_kb: 8192,      // Typical L3 cache size
            memory_bandwidth_gbps: 25.6, // Typical DDR4 bandwidth
            cpu_frequency_ghz: 3.0,      // Typical CPU frequency
        }
    }

    // Private helper methods

    fn detect_simd_capabilities(
        capabilities: &HardwareCapabilities,
    ) -> AdapterResult<SIMDCapabilities> {
        Ok(SIMDCapabilities {
            sse_supported: capabilities.cpu.features.sse42_support,
            sse2_supported: capabilities.cpu.features.sse42_support,
            avx_supported: capabilities.cpu.features.avx2_support, // Use AVX2 as proxy for AVX
            avx2_supported: capabilities.cpu.features.avx2_support,
            avx512_supported: capabilities.cpu.features.avx512_support,
            neon_supported: capabilities.cpu.features.neon_support,
            popcnt_supported: capabilities.cpu.features.sse42_support, // Usually comes with SSE4.2
            bmi_supported: false, // Would need to detect BMI support
            vector_register_width: if capabilities.cpu.features.avx512_support {
                512
            } else if capabilities.cpu.features.avx2_support {
                256
            } else if capabilities.cpu.features.sse42_support
                || capabilities.cpu.features.neon_support
            {
                128
            } else {
                64
            },
            max_f32_lanes: if capabilities.cpu.features.avx512_support {
                16
            } else if capabilities.cpu.features.avx2_support {
                8
            } else if capabilities.cpu.features.sse42_support
                || capabilities.cpu.features.neon_support
            {
                4
            } else {
                1
            },
            max_i32_lanes: if capabilities.cpu.features.avx512_support {
                16
            } else if capabilities.cpu.features.avx2_support {
                8
            } else if capabilities.cpu.features.sse42_support
                || capabilities.cpu.features.neon_support
            {
                4
            } else {
                1
            },
            max_i8_lanes: if capabilities.cpu.features.avx512_support {
                64
            } else if capabilities.cpu.features.avx2_support {
                32
            } else if capabilities.cpu.features.sse42_support
                || capabilities.cpu.features.neon_support
            {
                16
            } else {
                1
            },
        })
    }

    fn select_optimal_strategy(
        config: &HardwareAccelerationConfig,
        simd_capabilities: &SIMDCapabilities,
    ) -> AdapterResult<OptimizationStrategy> {
        if !config.enable_simd {
            return Ok(OptimizationStrategy::Scalar);
        }

        if simd_capabilities.avx2_supported && config.enable_avx2 {
            Ok(OptimizationStrategy::SIMD)
        } else if simd_capabilities.sse2_supported {
            Ok(OptimizationStrategy::SIMD)
        } else if simd_capabilities.neon_supported && config.enable_neon {
            Ok(OptimizationStrategy::SIMD)
        } else {
            warn!("No suitable SIMD capabilities found, falling back to scalar operations");
            Ok(OptimizationStrategy::Scalar)
        }
    }

    fn select_strategy_for_workload(
        &self,
        workload: &WorkloadCharacteristics,
    ) -> AdapterResult<OptimizationStrategy> {
        match (
            workload.vector_size,
            workload.batch_size,
            workload.operation_type,
        ) {
            // Large vectors with large batches benefit from parallel + SIMD
            (vs, bs, _) if vs >= 512 && bs >= 100 => Ok(OptimizationStrategy::Hybrid),

            // Medium vectors with medium batches benefit from SIMD
            (vs, bs, _) if vs >= 128 && bs >= 10 => Ok(OptimizationStrategy::SIMD),

            // Small vectors with large batches benefit from parallel processing
            (vs, bs, _) if vs < 128 && bs >= 1000 => Ok(OptimizationStrategy::Parallel),

            // Binary operations benefit from specialized SIMD
            (_, _, WorkloadOperationType::Binary) if self.simd_capabilities.popcnt_supported => {
                Ok(OptimizationStrategy::SIMD)
            }

            // Adaptive for mixed workloads
            _ => Ok(OptimizationStrategy::Adaptive),
        }
    }

    fn select_computation_strategy(
        &self,
        vector_size: usize,
        batch_size: usize,
    ) -> AdapterResult<OptimizationStrategy> {
        let workload = WorkloadCharacteristics {
            vector_size,
            batch_size,
            operation_type: WorkloadOperationType::FloatingPoint, // Default assumption
        };

        self.select_strategy_for_workload(&workload)
    }

    async fn update_performance_stats(
        &mut self,
        operation_time_us: u64,
        strategy: OptimizationStrategy,
    ) {
        self.stats.total_operations += 1;

        // Update average operation time (exponential moving average)
        let alpha = 0.1; // Smoothing factor
        self.stats.average_operation_time_us = (alpha * operation_time_us as f32
            + (1.0 - alpha) * self.stats.average_operation_time_us as f32)
            as u64;

        // Calculate speedup ratios (simplified)
        match strategy {
            OptimizationStrategy::SIMD => {
                self.stats.simd_speedup_ratio = (self.stats.simd_speedup_ratio * 0.9) + (2.5 * 0.1); // Assume ~2.5x speedup
            }
            OptimizationStrategy::Parallel => {
                let core_speedup = (self.capabilities.cpu.physical_cores as f32 * 0.8).min(8.0); // 80% efficiency cap
                self.stats.parallel_speedup_ratio =
                    (self.stats.parallel_speedup_ratio * 0.9) + (core_speedup * 0.1);
            }
            _ => {}
        }
    }
}

/// Workload characteristics for optimization strategy selection
#[derive(Debug, Clone)]
pub struct WorkloadCharacteristics {
    /// Vector dimension size
    pub vector_size: usize,

    /// Batch size for processing
    pub batch_size: usize,

    /// Type of operation being performed
    pub operation_type: WorkloadOperationType,
}

/// Types of operations for workload characterization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadOperationType {
    /// Floating point operations (FP32)
    FloatingPoint,

    /// Integer operations (INT8)
    Integer,

    /// Binary operations (Hamming distance)
    Binary,

    /// Product quantization operations
    ProductQuantization,

    /// Mixed operations
    Mixed,
}

/// Data types supported by SIMD operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SIMDData {
    /// 32-bit floating point
    F32,

    /// 32-bit integer
    I32,

    /// 8-bit signed integer
    I8,

    /// 8-bit unsigned integer
    U8,
}

/// Trait for computations that support hardware acceleration
#[async_trait::async_trait]
pub trait AcceleratedComputation {
    type Output;
    type Error: std::error::Error + Send + Sync + 'static;

    /// Execute computation using scalar operations
    async fn execute_scalar(&self) -> Result<Self::Output, Self::Error>;

    /// Execute computation using SIMD acceleration
    async fn execute_simd(
        &self,
        capabilities: &SIMDCapabilities,
    ) -> Result<Self::Output, Self::Error>;

    /// Execute computation using parallel processing
    async fn execute_parallel(&self, num_cores: usize) -> Result<Self::Output, Self::Error>;

    /// Execute computation using hybrid approach (SIMD + parallel)
    async fn execute_hybrid(
        &self,
        capabilities: &SIMDCapabilities,
        num_cores: usize,
    ) -> Result<Self::Output, Self::Error>;
}
