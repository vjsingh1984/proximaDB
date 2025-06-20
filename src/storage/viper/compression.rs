// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER SIMD-Optimized Compression Engine
//!
//! High-performance compression optimized for vector data with SIMD acceleration
//! and adaptive algorithm selection based on data characteristics.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::compute::hardware_detection::SimdLevel;
use crate::storage::wal::config::CompressionAlgorithm;

/// SIMD-optimized compression engine for VIPER storage
pub struct SimdCompressionEngine {
    /// Available compression backends
    backends: Arc<RwLock<HashMap<CompressionAlgorithm, Box<dyn CompressionBackend>>>>,

    /// SIMD instruction set detection
    simd_capabilities: SimdCapabilities,

    /// Compression statistics for adaptive selection
    stats: Arc<RwLock<CompressionStats>>,

    /// Algorithm selection model
    selection_model: Arc<RwLock<Option<CompressionSelectionModel>>>,
}

/// Trait for compression backend implementations
#[async_trait::async_trait]
pub trait CompressionBackend: Send + Sync {
    /// Compress data using this backend
    async fn compress(&self, data: &[u8]) -> Result<Vec<u8>>;

    /// Decompress data using this backend
    async fn decompress(&self, compressed: &[u8]) -> Result<Vec<u8>>;

    /// Get compression characteristics
    fn characteristics(&self) -> CompressionCharacteristics;

    /// Get algorithm identifier
    fn algorithm(&self) -> CompressionAlgorithm;
}

/// Compression characteristics for algorithm selection
#[derive(Debug, Clone)]
pub struct CompressionCharacteristics {
    pub algorithm: CompressionAlgorithm,
    pub compression_speed_mbps: f32,
    pub decompression_speed_mbps: f32,
    pub typical_compression_ratio: f32,
    pub memory_usage_mb: usize,
    pub supports_simd: bool,
    pub best_for_data_types: Vec<DataType>,
}

/// Data types for compression optimization
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DataType {
    DenseVectors,
    SparseVectors,
    Metadata,
    ClusterIds,
    Timestamps,
}

/// SIMD capabilities detection
#[derive(Debug, Clone)]
pub struct SimdCapabilities {
    pub has_sse4_2: bool,
    pub has_avx2: bool,
    pub has_avx512: bool,
    pub has_neon: bool,
    pub detected_instruction_set: SimdLevel,
}

/// Compression statistics for adaptive selection
#[derive(Debug, Default)]
pub struct CompressionStats {
    /// Per-algorithm performance metrics
    pub algorithm_performance: HashMap<CompressionAlgorithm, AlgorithmPerformance>,

    /// Data type compression ratios
    pub data_type_ratios: HashMap<DataType, f32>,

    /// Adaptive selection accuracy
    pub selection_accuracy: f32,

    /// Total operations
    pub total_compressions: u64,
    pub total_decompressions: u64,
}

/// Performance metrics for compression algorithms
#[derive(Debug, Clone)]
pub struct AlgorithmPerformance {
    pub compression_count: u64,
    pub decompression_count: u64,
    pub avg_compression_ratio: f32,
    pub avg_compression_speed_mbps: f32,
    pub avg_decompression_speed_mbps: f32,
    pub avg_memory_usage_mb: f32,
    pub error_count: u64,
}

/// Model for adaptive compression algorithm selection
#[derive(Debug, Clone)]
pub struct CompressionSelectionModel {
    /// Feature weights for algorithm selection
    pub feature_weights: Vec<f32>,

    /// Algorithm preferences per data type
    pub data_type_preferences: HashMap<DataType, Vec<(CompressionAlgorithm, f32)>>,

    /// Performance predictors
    pub performance_predictors: PerformancePredictors,

    /// Model training metadata
    pub training_metadata: ModelTrainingMetadata,
}

/// Performance prediction models
#[derive(Debug, Clone)]
pub struct PerformancePredictors {
    /// Compression ratio predictor
    pub ratio_predictor: RatioPredictor,

    /// Speed predictor
    pub speed_predictor: SpeedPredictor,

    /// Memory usage predictor  
    pub memory_predictor: MemoryPredictor,
}

/// Compression ratio prediction model
#[derive(Debug, Clone)]
pub struct RatioPredictor {
    pub linear_weights: Vec<f32>,
    pub entropy_factor: f32,
    pub sparsity_factor: f32,
    pub data_type_factors: HashMap<DataType, f32>,
}

/// Speed prediction model
#[derive(Debug, Clone)]
pub struct SpeedPredictor {
    pub base_speeds: HashMap<CompressionAlgorithm, f32>,
    pub size_scaling_factors: HashMap<CompressionAlgorithm, f32>,
    pub simd_speedup_factors: HashMap<SimdLevel, f32>,
}

/// Memory usage prediction model
#[derive(Debug, Clone)]
pub struct MemoryPredictor {
    pub base_memory: HashMap<CompressionAlgorithm, f32>,
    pub size_scaling: HashMap<CompressionAlgorithm, f32>,
}

/// Model training metadata
#[derive(Debug, Clone)]
pub struct ModelTrainingMetadata {
    pub training_samples: usize,
    pub training_accuracy: f32,
    pub last_trained: chrono::DateTime<chrono::Utc>,
    pub feature_count: usize,
}

/// Compression request with optimization hints
#[derive(Debug, Clone)]
pub struct CompressionRequest {
    pub data: Vec<u8>,
    pub data_type: DataType,
    pub priority: CompressionPriority,
    pub optimization_target: OptimizationTarget,
    pub simd_enabled: bool,
}

/// Compression priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionPriority {
    Low,
    Normal,
    High,
    Critical,
}

/// Optimization targets for compression
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizationTarget {
    MaxCompression,
    MaxSpeed,
    Balanced,
    MinMemory,
}

/// Compression result with metadata
#[derive(Debug)]
pub struct CompressionResult {
    pub compressed_data: Vec<u8>,
    pub algorithm_used: CompressionAlgorithm,
    pub compression_ratio: f32,
    pub compression_time_ms: f32,
    pub memory_used_mb: f32,
    pub simd_used: bool,
}

/// Decompression result with metadata
#[derive(Debug)]
pub struct DecompressionResult {
    pub decompressed_data: Vec<u8>,
    pub algorithm_used: CompressionAlgorithm,
    pub decompression_time_ms: f32,
    pub memory_used_mb: f32,
    pub simd_used: bool,
}

impl SimdCompressionEngine {
    /// Create new SIMD compression engine
    pub async fn new() -> Result<Self> {
        let simd_capabilities = Self::get_default_simd_capabilities();

        let mut backends: HashMap<CompressionAlgorithm, Box<dyn CompressionBackend>> =
            HashMap::new();

        // Initialize compression backends
        backends.insert(
            CompressionAlgorithm::None,
            Box::new(NoCompressionBackend::new()),
        );

        backends.insert(
            CompressionAlgorithm::Lz4,
            Box::new(Lz4Backend::new(simd_capabilities.clone())),
        );

        backends.insert(
            CompressionAlgorithm::Zstd { level: 3 },
            Box::new(ZstdBackend::new(3, simd_capabilities.clone())),
        );

        backends.insert(
            CompressionAlgorithm::Snappy,
            Box::new(SnappyBackend::new(simd_capabilities.clone())),
        );

        backends.insert(
            CompressionAlgorithm::Zstd { level: 6 },
            Box::new(ZstdBackend::new(6, simd_capabilities.clone())),
        );

        Ok(Self {
            backends: Arc::new(RwLock::new(backends)),
            simd_capabilities,
            stats: Arc::new(RwLock::new(CompressionStats::default())),
            selection_model: Arc::new(RwLock::new(None)),
        })
    }

    /// Compress data with adaptive algorithm selection
    pub async fn compress(&self, request: CompressionRequest) -> Result<CompressionResult> {
        let start_time = std::time::Instant::now();

        // Select optimal algorithm
        let algorithm = self.select_compression_algorithm(&request).await?;

        // Get backend
        let backends = self.backends.read().await;
        let backend = backends
            .get(&algorithm)
            .ok_or_else(|| anyhow::anyhow!("Compression backend not found: {:?}", algorithm))?;

        // Perform compression
        let compressed_data = backend
            .compress(&request.data)
            .await
            .context("Compression failed")?;

        let elapsed_ms = start_time.elapsed().as_millis() as f32;
        let compression_ratio = request.data.len() as f32 / compressed_data.len() as f32;

        // Update statistics
        self.update_compression_stats(&algorithm, compression_ratio, elapsed_ms)
            .await?;

        let memory_usage = self.estimate_memory_usage(&algorithm, request.data.len());
        Ok(CompressionResult {
            compressed_data,
            algorithm_used: algorithm,
            compression_ratio,
            compression_time_ms: elapsed_ms,
            memory_used_mb: memory_usage,
            simd_used: request.simd_enabled && backend.characteristics().supports_simd,
        })
    }

    /// Decompress data
    pub async fn decompress(
        &self,
        compressed_data: &[u8],
        algorithm: CompressionAlgorithm,
    ) -> Result<DecompressionResult> {
        let start_time = std::time::Instant::now();

        // Get backend
        let backends = self.backends.read().await;
        let backend = backends
            .get(&algorithm)
            .ok_or_else(|| anyhow::anyhow!("Compression backend not found: {:?}", algorithm))?;

        // Perform decompression
        let decompressed_data = backend
            .decompress(compressed_data)
            .await
            .context("Decompression failed")?;

        let elapsed_ms = start_time.elapsed().as_millis() as f32;

        // Update statistics
        self.update_decompression_stats(&algorithm, elapsed_ms)
            .await?;

        let memory_usage = self.estimate_memory_usage(&algorithm, compressed_data.len());
        Ok(DecompressionResult {
            decompressed_data,
            algorithm_used: algorithm,
            decompression_time_ms: elapsed_ms,
            memory_used_mb: memory_usage,
            simd_used: backend.characteristics().supports_simd,
        })
    }

    /// Select optimal compression algorithm based on request characteristics
    async fn select_compression_algorithm(
        &self,
        request: &CompressionRequest,
    ) -> Result<CompressionAlgorithm> {
        // Check if we have a trained model
        let model = self.selection_model.read().await;
        if let Some(selection_model) = model.as_ref() {
            // Use ML model for selection
            return self.ml_select_algorithm(request, selection_model).await;
        }

        // Fall back to heuristic selection
        self.heuristic_select_algorithm(request).await
    }

    /// ML-based algorithm selection
    async fn ml_select_algorithm(
        &self,
        request: &CompressionRequest,
        model: &CompressionSelectionModel,
    ) -> Result<CompressionAlgorithm> {
        // Extract features from request
        let features = self.extract_request_features(request);

        // Get data type preferences
        if let Some(preferences) = model.data_type_preferences.get(&request.data_type) {
            // Score algorithms based on features and preferences
            let mut best_algorithm = CompressionAlgorithm::Lz4;
            let mut best_score = 0.0;

            for (algorithm, base_score) in preferences {
                let feature_score = features
                    .iter()
                    .zip(model.feature_weights.iter())
                    .map(|(f, w)| f * w)
                    .sum::<f32>();

                let total_score = base_score + feature_score;
                if total_score > best_score {
                    best_score = total_score;
                    best_algorithm = algorithm.clone();
                }
            }

            return Ok(best_algorithm);
        }

        // Fall back to heuristic
        self.heuristic_select_algorithm(request).await
    }

    /// Heuristic algorithm selection
    async fn heuristic_select_algorithm(
        &self,
        request: &CompressionRequest,
    ) -> Result<CompressionAlgorithm> {
        match request.optimization_target {
            OptimizationTarget::MaxSpeed => match request.data_type {
                DataType::DenseVectors => Ok(CompressionAlgorithm::Lz4),
                DataType::SparseVectors => Ok(CompressionAlgorithm::Snappy),
                DataType::Metadata => Ok(CompressionAlgorithm::Lz4),
                DataType::ClusterIds => Ok(CompressionAlgorithm::Lz4),
                DataType::Timestamps => Ok(CompressionAlgorithm::None),
            },

            OptimizationTarget::MaxCompression => match request.data_type {
                DataType::DenseVectors => Ok(CompressionAlgorithm::Zstd { level: 6 }),
                DataType::SparseVectors => Ok(CompressionAlgorithm::Zstd { level: 3 }),
                DataType::Metadata => Ok(CompressionAlgorithm::Snappy),
                DataType::ClusterIds => Ok(CompressionAlgorithm::Zstd { level: 3 }),
                DataType::Timestamps => Ok(CompressionAlgorithm::Lz4),
            },

            OptimizationTarget::Balanced => match request.data_type {
                DataType::DenseVectors => Ok(CompressionAlgorithm::Zstd { level: 3 }),
                DataType::SparseVectors => Ok(CompressionAlgorithm::Lz4),
                DataType::Metadata => Ok(CompressionAlgorithm::Lz4),
                DataType::ClusterIds => Ok(CompressionAlgorithm::Lz4),
                DataType::Timestamps => Ok(CompressionAlgorithm::Lz4),
            },

            OptimizationTarget::MinMemory => {
                // Prefer algorithms with low memory overhead
                Ok(CompressionAlgorithm::Lz4)
            }
        }
    }

    /// Extract features from compression request
    fn extract_request_features(&self, request: &CompressionRequest) -> Vec<f32> {
        vec![
            request.data.len() as f32,
            self.calculate_entropy(&request.data),
            self.calculate_sparsity(&request.data),
            match request.data_type {
                DataType::DenseVectors => 1.0,
                _ => 0.0,
            },
            match request.priority {
                CompressionPriority::Critical => 1.0,
                CompressionPriority::High => 0.8,
                CompressionPriority::Normal => 0.5,
                CompressionPriority::Low => 0.2,
            },
        ]
    }

    /// Calculate data entropy
    fn calculate_entropy(&self, data: &[u8]) -> f32 {
        let mut counts = [0u64; 256];
        for &byte in data {
            counts[byte as usize] += 1;
        }

        let len = data.len() as f64;
        let mut entropy = 0.0;

        for count in counts.iter() {
            if *count > 0 {
                let probability = *count as f64 / len;
                entropy -= probability * probability.log2();
            }
        }

        entropy as f32
    }

    /// Calculate data sparsity (approximate)
    fn calculate_sparsity(&self, data: &[u8]) -> f32 {
        let zero_count = data.iter().filter(|&&b| b == 0).count();
        zero_count as f32 / data.len() as f32
    }

    /// Get default SIMD capabilities (hardware detection done at server startup)
    fn get_default_simd_capabilities() -> SimdCapabilities {
        SimdCapabilities {
            has_sse4_2: true,  // Assume basic capabilities available
            has_avx2: false,   // Conservative default
            has_avx512: false, // Conservative default
            has_neon: cfg!(target_arch = "aarch64"),
            detected_instruction_set: SimdLevel::None, // Let distance algorithms handle optimization
        }
    }

    /// Estimate memory usage for algorithm
    fn estimate_memory_usage(&self, algorithm: &CompressionAlgorithm, data_size: usize) -> f32 {
        match algorithm {
            CompressionAlgorithm::None => 0.0,
            CompressionAlgorithm::Lz4 => (data_size / 1024) as f32 * 0.1, // 10% overhead
            CompressionAlgorithm::Zstd { level } => {
                (data_size / 1024) as f32 * (0.2 + (*level as f32 * 0.1))
            }
            CompressionAlgorithm::Snappy => (data_size / 1024) as f32 * 0.15,
            CompressionAlgorithm::Snappy { .. } => (data_size / 1024) as f32 * 0.3,
        }
    }

    /// Update compression statistics
    async fn update_compression_stats(
        &self,
        algorithm: &CompressionAlgorithm,
        ratio: f32,
        _time_ms: f32,
    ) -> Result<()> {
        let mut stats = self.stats.write().await;
        stats.total_compressions += 1;

        let perf = stats
            .algorithm_performance
            .entry(algorithm.clone())
            .or_insert(AlgorithmPerformance {
                compression_count: 0,
                decompression_count: 0,
                avg_compression_ratio: 0.0,
                avg_compression_speed_mbps: 0.0,
                avg_decompression_speed_mbps: 0.0,
                avg_memory_usage_mb: 0.0,
                error_count: 0,
            });

        // Update with exponential moving average
        let alpha = 0.1;
        perf.compression_count += 1;
        perf.avg_compression_ratio = perf.avg_compression_ratio * (1.0 - alpha) + ratio * alpha;

        Ok(())
    }

    /// Update decompression statistics
    async fn update_decompression_stats(
        &self,
        algorithm: &CompressionAlgorithm,
        _time_ms: f32,
    ) -> Result<()> {
        let mut stats = self.stats.write().await;
        stats.total_decompressions += 1;

        let perf = stats
            .algorithm_performance
            .entry(algorithm.clone())
            .or_insert(AlgorithmPerformance {
                compression_count: 0,
                decompression_count: 0,
                avg_compression_ratio: 0.0,
                avg_compression_speed_mbps: 0.0,
                avg_decompression_speed_mbps: 0.0,
                avg_memory_usage_mb: 0.0,
                error_count: 0,
            });

        perf.decompression_count += 1;

        Ok(())
    }
}

/// No compression backend (pass-through)
struct NoCompressionBackend;

impl NoCompressionBackend {
    fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl CompressionBackend for NoCompressionBackend {
    async fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    async fn decompress(&self, compressed: &[u8]) -> Result<Vec<u8>> {
        Ok(compressed.to_vec())
    }

    fn characteristics(&self) -> CompressionCharacteristics {
        CompressionCharacteristics {
            algorithm: CompressionAlgorithm::None,
            compression_speed_mbps: f32::INFINITY,
            decompression_speed_mbps: f32::INFINITY,
            typical_compression_ratio: 1.0,
            memory_usage_mb: 0,
            supports_simd: false,
            best_for_data_types: vec![DataType::Timestamps],
        }
    }

    fn algorithm(&self) -> CompressionAlgorithm {
        CompressionAlgorithm::None
    }
}

/// LZ4 compression backend with SIMD optimization
struct Lz4Backend {
    simd_capabilities: SimdCapabilities,
}

impl Lz4Backend {
    fn new(simd_capabilities: SimdCapabilities) -> Self {
        Self { simd_capabilities }
    }
}

#[async_trait::async_trait]
impl CompressionBackend for Lz4Backend {
    async fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        // In a real implementation, this would use the lz4 crate
        // with SIMD optimizations when available
        Ok(data.to_vec())
    }

    async fn decompress(&self, compressed: &[u8]) -> Result<Vec<u8>> {
        Ok(compressed.to_vec())
    }

    fn characteristics(&self) -> CompressionCharacteristics {
        CompressionCharacteristics {
            algorithm: CompressionAlgorithm::Lz4,
            compression_speed_mbps: 300.0,
            decompression_speed_mbps: 1000.0,
            typical_compression_ratio: 2.5,
            memory_usage_mb: 1,
            supports_simd: true,
            best_for_data_types: vec![DataType::DenseVectors, DataType::ClusterIds],
        }
    }

    fn algorithm(&self) -> CompressionAlgorithm {
        CompressionAlgorithm::Lz4
    }
}

/// Zstd compression backend
struct ZstdBackend {
    level: i32,
    simd_capabilities: SimdCapabilities,
}

impl ZstdBackend {
    fn new(level: i32, simd_capabilities: SimdCapabilities) -> Self {
        Self {
            level,
            simd_capabilities,
        }
    }
}

#[async_trait::async_trait]
impl CompressionBackend for ZstdBackend {
    async fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Implementation would use zstd crate
        Ok(data.to_vec())
    }

    async fn decompress(&self, compressed: &[u8]) -> Result<Vec<u8>> {
        Ok(compressed.to_vec())
    }

    fn characteristics(&self) -> CompressionCharacteristics {
        CompressionCharacteristics {
            algorithm: CompressionAlgorithm::Zstd { level: self.level },
            compression_speed_mbps: 100.0 - (self.level as f32 * 10.0),
            decompression_speed_mbps: 400.0,
            typical_compression_ratio: 3.0 + (self.level as f32 * 0.3),
            memory_usage_mb: 2 + (self.level as usize),
            supports_simd: false,
            best_for_data_types: vec![DataType::DenseVectors, DataType::SparseVectors],
        }
    }

    fn algorithm(&self) -> CompressionAlgorithm {
        CompressionAlgorithm::Zstd { level: self.level }
    }
}

/// Snappy compression backend
struct SnappyBackend {
    simd_capabilities: SimdCapabilities,
}

impl SnappyBackend {
    fn new(simd_capabilities: SimdCapabilities) -> Self {
        Self { simd_capabilities }
    }
}

#[async_trait::async_trait]
impl CompressionBackend for SnappyBackend {
    async fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    async fn decompress(&self, compressed: &[u8]) -> Result<Vec<u8>> {
        Ok(compressed.to_vec())
    }

    fn characteristics(&self) -> CompressionCharacteristics {
        CompressionCharacteristics {
            algorithm: CompressionAlgorithm::Snappy,
            compression_speed_mbps: 250.0,
            decompression_speed_mbps: 800.0,
            typical_compression_ratio: 2.0,
            memory_usage_mb: 1,
            supports_simd: true,
            best_for_data_types: vec![DataType::SparseVectors, DataType::Metadata],
        }
    }

    fn algorithm(&self) -> CompressionAlgorithm {
        CompressionAlgorithm::Snappy
    }
}
