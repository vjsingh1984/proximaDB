// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified Server Builder Pattern
//!
//! This module provides a high-level builder that coordinates all ProximaDB subsystems:
//! - Storage system (data storage, WAL, filesystem)
//! - Network configuration (host, port, TLS)
//! - Compute system (hardware acceleration, SIMD, CUDA)
//! - Indexing system (algorithms, distance metrics)
//! - Monitoring and observability
//!
//! Each subsystem has its own focused builder, maintaining separation of concerns.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::storage::builder::{StorageSystem, StorageSystemBuilder};

/// Server network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Server host/interface (e.g., "0.0.0.0", "127.0.0.1", "localhost")
    pub host: String,

    /// Server port
    pub port: u16,

    /// Enable TLS/SSL
    pub enable_tls: bool,

    /// TLS certificate path (optional)
    pub tls_cert_path: Option<String>,

    /// TLS private key path (optional)
    pub tls_key_path: Option<String>,

    /// Maximum concurrent connections
    pub max_connections: usize,

    /// Connection timeout (seconds)
    pub connection_timeout_seconds: u64,

    /// Request timeout (seconds)
    pub request_timeout_seconds: u64,

    /// Enable HTTP/2
    pub enable_http2: bool,

    /// Enable gRPC
    pub enable_grpc: bool,

    /// gRPC port (if different from main port)
    pub grpc_port: Option<u16>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 5678,
            enable_tls: false,
            tls_cert_path: None,
            tls_key_path: None,
            max_connections: 10000,
            connection_timeout_seconds: 30,
            request_timeout_seconds: 300,
            enable_http2: true,
            enable_grpc: true,
            grpc_port: Some(5679),
        }
    }
}

/// Hardware acceleration configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Copy)]
pub enum HardwareAcceleration {
    /// CPU-only with SIMD optimizations
    SIMD,
    /// NVIDIA CUDA acceleration
    CUDA,
    /// AMD ROCm acceleration
    ROCm,
    /// OpenCL acceleration (cross-platform)
    OpenCL,
    /// Auto-detect and use best available
    Auto,
    /// No acceleration (fallback)
    None,
}

impl Default for HardwareAcceleration {
    fn default() -> Self {
        Self::Auto // Auto-detect best acceleration
    }
}

/// Compute system configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeConfig {
    /// Hardware acceleration method
    pub acceleration: HardwareAcceleration,

    /// Number of compute threads
    pub compute_threads: usize,

    /// Enable SIMD optimizations
    pub enable_simd: bool,

    /// Enable GPU acceleration (if available)
    pub enable_gpu: bool,

    /// GPU memory limit (MB)
    pub gpu_memory_limit_mb: Option<usize>,

    /// CUDA device IDs to use (empty = use all)
    pub cuda_devices: Vec<u32>,

    /// Enable mixed precision computations
    pub enable_mixed_precision: bool,
}

impl Default for ComputeConfig {
    fn default() -> Self {
        Self {
            acceleration: HardwareAcceleration::default(),
            compute_threads: num_cpus::get() * 2,
            enable_simd: true,
            enable_gpu: false, // Disabled by default for compatibility
            gpu_memory_limit_mb: None,
            cuda_devices: vec![],
            enable_mixed_precision: false,
        }
    }
}

/// Distance metric algorithms
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DistanceMetric {
    /// Cosine similarity (default for most text/semantic vectors)
    Cosine,
    /// Euclidean distance (L2)
    Euclidean,
    /// Manhattan distance (L1)
    Manhattan,
    /// Dot product (inner product)
    DotProduct,
    /// Hamming distance (for binary vectors)
    Hamming,
    /// Jaccard similarity
    Jaccard,
    /// Minkowski distance with custom p parameter
    Minkowski { p: f32 },
    /// Angular distance
    Angular,
    /// Canberra distance
    Canberra,
    /// Chebyshev distance (L∞)
    Chebyshev,
    /// Auto-select based on data characteristics
    Auto,
}

impl Default for DistanceMetric {
    fn default() -> Self {
        Self::Cosine // Cosine is best for most ML embeddings
    }
}

/// Indexing algorithm configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IndexingAlgorithm {
    /// Hierarchical Navigable Small World (default for most cases)
    HNSW,
    /// Inverted File with Product Quantization
    IVFPQ,
    /// Locality Sensitive Hashing
    LSH,
    /// Brute force for small datasets
    BruteForce,
    /// Graph-based approximate search
    NSW,
    /// Auto-select based on data characteristics
    Auto,
}

impl Default for IndexingAlgorithm {
    fn default() -> Self {
        Self::HNSW // HNSW is the best general-purpose algorithm
    }
}

/// Indexing system configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingConfig {
    /// Default vector indexing algorithm
    pub default_algorithm: IndexingAlgorithm,

    /// Default distance metric
    pub default_distance_metric: DistanceMetric,

    /// Fallback algorithms for different scenarios
    pub fallback_algorithms: Vec<IndexingAlgorithm>,

    /// Supported distance metrics
    pub supported_distance_metrics: Vec<DistanceMetric>,

    /// Algorithm-specific parameters
    pub algorithm_params: std::collections::HashMap<String, serde_json::Value>,

    /// Distance metric specific parameters
    pub distance_params: std::collections::HashMap<String, serde_json::Value>,

    /// Enable automatic algorithm selection based on data characteristics
    pub enable_auto_selection: bool,

    /// Enable automatic distance metric selection
    pub enable_auto_distance_selection: bool,

    /// Index building parallelism
    pub build_parallelism: usize,

    /// Index memory limit (MB)
    pub memory_limit_mb: usize,
}

impl Default for IndexingConfig {
    fn default() -> Self {
        Self {
            default_algorithm: IndexingAlgorithm::default(),
            default_distance_metric: DistanceMetric::default(),
            fallback_algorithms: vec![IndexingAlgorithm::IVFPQ, IndexingAlgorithm::BruteForce],
            supported_distance_metrics: vec![
                DistanceMetric::Cosine,
                DistanceMetric::Euclidean,
                DistanceMetric::Manhattan,
                DistanceMetric::DotProduct,
            ],
            algorithm_params: std::collections::HashMap::new(),
            distance_params: std::collections::HashMap::new(),
            enable_auto_selection: true,
            enable_auto_distance_selection: false,
            build_parallelism: num_cpus::get(),
            memory_limit_mb: 1024,
        }
    }
}

/// Monitoring and observability configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    /// Enable detailed metrics collection
    pub enable_metrics: bool,

    /// Metrics collection interval (seconds)
    pub metrics_interval_seconds: u64,

    /// Enable performance profiling
    pub enable_profiling: bool,

    /// Enable health checks
    pub enable_health_checks: bool,

    /// Health check interval (seconds)
    pub health_check_interval_seconds: u64,

    /// Enable distributed tracing
    pub enable_tracing: bool,

    /// Tracing endpoint (e.g., Jaeger, Zipkin)
    pub tracing_endpoint: Option<String>,

    /// Log level
    pub log_level: String,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            enable_metrics: true,
            metrics_interval_seconds: 60,
            enable_profiling: false,
            enable_health_checks: true,
            health_check_interval_seconds: 30,
            enable_tracing: false,
            tracing_endpoint: None,
            log_level: "info".to_string(),
        }
    }
}

/// Complete server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Network configuration
    pub network: NetworkConfig,

    /// Compute system configuration
    pub compute: ComputeConfig,

    /// Indexing system configuration
    pub indexing: IndexingConfig,

    /// Monitoring configuration
    pub monitoring: MonitoringConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            network: NetworkConfig::default(),
            compute: ComputeConfig::default(),
            indexing: IndexingConfig::default(),
            monitoring: MonitoringConfig::default(),
        }
    }
}

/// Unified server builder that coordinates all subsystems
pub struct ServerBuilder {
    server_config: ServerConfig,
    storage_builder: StorageSystemBuilder,
}

impl ServerBuilder {
    /// Create a new server builder
    pub fn new() -> Self {
        Self {
            server_config: ServerConfig::default(),
            storage_builder: StorageSystemBuilder::new(),
        }
    }

    // === Network Configuration ===

    /// Set server endpoint (host and port)
    pub fn with_server_endpoint(mut self, host: impl Into<String>, port: u16) -> Self {
        self.server_config.network.host = host.into();
        self.server_config.network.port = port;
        self
    }

    /// Enable TLS with certificate paths
    pub fn with_tls(mut self, cert_path: impl Into<String>, key_path: impl Into<String>) -> Self {
        self.server_config.network.enable_tls = true;
        self.server_config.network.tls_cert_path = Some(cert_path.into());
        self.server_config.network.tls_key_path = Some(key_path.into());
        self
    }

    /// Configure network settings
    pub fn with_network_config(mut self, config: NetworkConfig) -> Self {
        self.server_config.network = config;
        self
    }

    // === Hardware/Compute Configuration ===

    /// Set hardware acceleration method
    pub fn with_hardware_acceleration(mut self, acceleration: HardwareAcceleration) -> Self {
        self.server_config.compute.acceleration = acceleration;
        self.server_config.compute.enable_simd = matches!(
            acceleration,
            HardwareAcceleration::SIMD | HardwareAcceleration::Auto
        );
        self.server_config.compute.enable_gpu = matches!(
            acceleration,
            HardwareAcceleration::CUDA | HardwareAcceleration::ROCm | HardwareAcceleration::OpenCL
        );
        self
    }

    /// Enable SIMD optimizations
    pub fn with_simd_acceleration(mut self) -> Self {
        self.server_config.compute.enable_simd = true;
        self.server_config.compute.acceleration = HardwareAcceleration::SIMD;
        self
    }

    /// Enable CUDA acceleration
    pub fn with_cuda_acceleration(mut self, devices: Vec<u32>) -> Self {
        self.server_config.compute.enable_gpu = true;
        self.server_config.compute.acceleration = HardwareAcceleration::CUDA;
        self.server_config.compute.cuda_devices = devices;
        self
    }

    /// Configure compute settings
    pub fn with_compute_config(mut self, config: ComputeConfig) -> Self {
        self.server_config.compute = config;
        self
    }

    // === Indexing Configuration ===

    /// Set default indexing algorithm
    pub fn with_indexing_algorithm(mut self, algorithm: IndexingAlgorithm) -> Self {
        self.server_config.indexing.default_algorithm = algorithm;
        self
    }

    /// Set default distance metric
    pub fn with_distance_metric(mut self, metric: DistanceMetric) -> Self {
        self.server_config.indexing.default_distance_metric = metric;
        self
    }

    /// Configure HNSW parameters
    pub fn with_hnsw_params(mut self, m: u32, ef_construction: u32) -> Self {
        self.server_config.indexing.algorithm_params.insert(
            "hnsw".to_string(),
            serde_json::json!({
                "m": m,
                "ef_construction": ef_construction,
                "max_m": m,
                "max_m0": m * 2,
            }),
        );
        self
    }

    /// Configure IVFPQ parameters
    pub fn with_ivfpq_params(mut self, nlist: u32, m: u32, nbits: u32) -> Self {
        self.server_config.indexing.algorithm_params.insert(
            "ivfpq".to_string(),
            serde_json::json!({
                "nlist": nlist,
                "m": m,
                "nbits": nbits,
            }),
        );
        self
    }

    /// Configure indexing system
    pub fn with_indexing_config(mut self, config: IndexingConfig) -> Self {
        self.server_config.indexing = config;
        self
    }

    // === Monitoring Configuration ===

    /// Enable detailed monitoring
    pub fn with_detailed_monitoring(mut self) -> Self {
        self.server_config.monitoring = MonitoringConfig {
            enable_metrics: true,
            metrics_interval_seconds: 10,
            enable_profiling: true,
            enable_health_checks: true,
            health_check_interval_seconds: 5,
            enable_tracing: true,
            tracing_endpoint: None,
            log_level: "debug".to_string(),
        };
        self
    }

    /// Configure monitoring
    pub fn with_monitoring_config(mut self, config: MonitoringConfig) -> Self {
        self.server_config.monitoring = config;
        self
    }

    // === Storage Configuration (delegated to StorageSystemBuilder) ===

    /// Configure storage system using a closure
    pub fn configure_storage<F>(mut self, configure: F) -> Self
    where
        F: FnOnce(StorageSystemBuilder) -> StorageSystemBuilder,
    {
        self.storage_builder = configure(self.storage_builder);
        self
    }

    /// Build the complete ProximaDB server
    pub async fn build(self) -> Result<ProximaDBServer> {
        tracing::info!("🚀 Building ProximaDB server");
        tracing::info!(
            "🌐 Network: {}:{}",
            self.server_config.network.host,
            self.server_config.network.port
        );
        tracing::info!(
            "⚡ Hardware acceleration: {:?}",
            self.server_config.compute.acceleration
        );
        tracing::info!(
            "🔍 Indexing algorithm: {:?}",
            self.server_config.indexing.default_algorithm
        );
        tracing::info!(
            "📏 Distance metric: {:?}",
            self.server_config.indexing.default_distance_metric
        );

        // Build storage system
        let storage_system = self.storage_builder.build().await?;
        tracing::info!("✅ Storage system initialized");

        // TODO: Initialize network layer based on config
        // TODO: Initialize compute engines based on hardware config
        // TODO: Initialize indexing system
        // TODO: Initialize monitoring systems

        let server = ProximaDBServer {
            config: self.server_config,
            storage_system: Arc::new(storage_system),
        };

        tracing::info!("🎉 ProximaDB server build complete");

        Ok(server)
    }

    /// Get current server configuration (for inspection)
    pub fn server_config(&self) -> &ServerConfig {
        &self.server_config
    }

    /// Get current storage builder (for inspection)
    pub fn storage_builder(&self) -> &StorageSystemBuilder {
        &self.storage_builder
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Complete ProximaDB server with all subsystems
pub struct ProximaDBServer {
    config: ServerConfig,
    storage_system: Arc<StorageSystem>,
}

impl ProximaDBServer {
    /// Get server configuration
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// Get storage system
    pub fn storage_system(&self) -> &Arc<StorageSystem> {
        &self.storage_system
    }

    /// Get server endpoint
    pub fn endpoint(&self) -> (&str, u16) {
        (&self.config.network.host, self.config.network.port)
    }

    /// Get indexing configuration
    pub fn indexing_config(&self) -> &IndexingConfig {
        &self.config.indexing
    }

    /// Get compute configuration
    pub fn compute_config(&self) -> &ComputeConfig {
        &self.config.compute
    }

    /// Start the server (placeholder)
    pub async fn start(&self) -> Result<()> {
        tracing::info!(
            "🚀 Starting ProximaDB server on {}:{}",
            self.config.network.host,
            self.config.network.port
        );

        // TODO: Start network listeners
        // TODO: Initialize indexing engines
        // TODO: Start monitoring systems
        // TODO: Start health check endpoints

        Ok(())
    }
}

impl std::fmt::Debug for ProximaDBServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProximaDBServer")
            .field(
                "endpoint",
                &format!("{}:{}", self.config.network.host, self.config.network.port),
            )
            .field("acceleration", &self.config.compute.acceleration)
            .field(
                "indexing_algorithm",
                &self.config.indexing.default_algorithm,
            )
            .field(
                "distance_metric",
                &self.config.indexing.default_distance_metric,
            )
            .field("storage_layout", &self.storage_system.storage_layout())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::builder::StorageLayoutStrategy;

    #[tokio::test]
    async fn test_server_builder_basic() {
        let builder = ServerBuilder::new()
            .with_server_endpoint("localhost", 8080)
            .with_simd_acceleration()
            .with_indexing_algorithm(IndexingAlgorithm::HNSW)
            .with_distance_metric(DistanceMetric::Cosine);

        assert_eq!(builder.server_config.network.host, "localhost");
        assert_eq!(builder.server_config.network.port, 8080);
        assert_eq!(
            builder.server_config.compute.acceleration,
            HardwareAcceleration::SIMD
        );
        assert_eq!(
            builder.server_config.indexing.default_algorithm,
            IndexingAlgorithm::HNSW
        );
        assert_eq!(
            builder.server_config.indexing.default_distance_metric,
            DistanceMetric::Cosine
        );
    }

    #[tokio::test]
    async fn test_layered_configuration() {
        let builder = ServerBuilder::new()
            .with_server_endpoint("0.0.0.0", 5678)
            .with_cuda_acceleration(vec![0, 1])
            .configure_storage(|storage| {
                storage
                    .with_viper_layout()
                    .with_high_performance_storage_mode()
                    .with_multi_disk_data_storage(vec![
                        "/nvme1/data".to_string(),
                        "/nvme2/data".to_string(),
                    ])
            })
            .with_hnsw_params(16, 200)
            .with_detailed_monitoring();

        // Server config
        assert_eq!(builder.server_config.network.port, 5678);
        assert_eq!(
            builder.server_config.compute.acceleration,
            HardwareAcceleration::CUDA
        );
        assert_eq!(builder.server_config.compute.cuda_devices, vec![0, 1]);

        // Storage config (inspected through storage builder)
        assert_eq!(
            builder
                .storage_builder
                .config()
                .data_storage
                .layout_strategy,
            StorageLayoutStrategy::Viper
        );
        assert_eq!(
            builder
                .storage_builder
                .config()
                .data_storage
                .data_urls
                .len(),
            2
        );

        // Indexing config
        let hnsw_params = builder
            .server_config
            .indexing
            .algorithm_params
            .get("hnsw")
            .unwrap();
        assert_eq!(hnsw_params["m"], 16);
        assert_eq!(hnsw_params["ef_construction"], 200);

        // Monitoring config
        assert!(builder.server_config.monitoring.enable_profiling);
        assert!(builder.server_config.monitoring.enable_tracing);
    }
}
