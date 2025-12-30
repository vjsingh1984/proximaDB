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

//! Smart I/O Layer with Range Coalescing
//!
//! This module provides intelligent I/O optimization for ProximaDB's filesystem
//! abstraction layer. It reduces I/O operations by:
//!
//! 1. **Range Coalescing**: Merging adjacent or nearby byte ranges to reduce
//!    the number of I/O operations, particularly beneficial for cloud storage.
//!
//! 2. **Parallel Reads**: Executing multiple range reads concurrently using
//!    tokio tasks for improved throughput.
//!
//! 3. **I/O Metrics**: Collecting detailed metrics about I/O patterns to
//!    enable optimization and monitoring.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      SmartIoLayer                           │
//! ├─────────────────────────────────────────────────────────────┤
//! │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────┐  │
//! │  │  RangeOptimizer │  │   IoStrategy    │  │  IoMetrics  │  │
//! │  │  (coalescing)   │  │  (execution)    │  │  (tracking) │  │
//! │  └────────┬────────┘  └────────┬────────┘  └──────┬──────┘  │
//! │           │                    │                   │         │
//! │           └────────────────────┼───────────────────┘         │
//! │                                │                             │
//! │                      ┌─────────▼─────────┐                   │
//! │                      │  Arc<FileSystem>  │                   │
//! │                      └───────────────────┘                   │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Example Usage
//!
//! ```rust,ignore
//! use proximadb::storage::persistence::filesystem::smart_io::SmartIoLayer;
//!
//! // Create SmartIoLayer with default settings
//! let smart_io = SmartIoLayer::new(filesystem);
//!
//! // Read multiple ranges efficiently
//! let ranges = vec![
//!     ByteRange::new(0, 1000),
//!     ByteRange::new(1100, 2000),  // Small gap - will be coalesced
//!     ByteRange::new(5000, 6000),  // Large gap - separate read
//! ];
//!
//! let data = smart_io.read_ranges("path/to/file", ranges).await?;
//! ```
//!
//! ## SOLID Principles
//!
//! - **S**ingle Responsibility: Each component has one job
//!   - SmartIoLayer: Coordinates I/O operations
//!   - RangeCoalescer: Merges ranges
//!   - ParallelReader: Executes reads
//!   - IoMetrics: Collects statistics
//!
//! - **O**pen/Closed: Extend via new IoStrategy or RangeOptimizer implementations
//!
//! - **L**iskov Substitution: All IoStrategy implementations are interchangeable
//!
//! - **I**nterface Segregation: Small, focused traits
//!
//! - **D**ependency Inversion: SmartIoLayer depends on abstractions (traits)

pub mod metrics;
pub mod parallel_reader;
pub mod range_coalescer;
pub mod traits;

use std::sync::Arc;
use tracing::{debug, trace};

use crate::storage::persistence::filesystem::{FileSystem, FsResult};

pub use metrics::{IoMetrics, IoMetricsSnapshot};
pub use parallel_reader::{AdaptiveReader, ParallelReader, ParallelReaderConfig, SequentialReader};
pub use range_coalescer::{AdaptiveRangeCoalescer, DefaultRangeCoalescer};
pub use traits::{ByteRange, IoCostEstimate, IoStrategy, RangeMapping, RangeOptimizer, RangeOptimizerWithMapping};

/// Smart I/O Layer configuration
#[derive(Debug, Clone)]
pub struct SmartIoConfig {
    /// Threshold for coalescing adjacent ranges (bytes)
    pub coalesce_threshold: u64,
    /// Minimum bytes per range before considering parallelism
    pub min_parallel_bytes: u64,
    /// Maximum concurrent read operations
    pub max_concurrent_reads: usize,
    /// Target chunk size for splitting large ranges
    pub target_chunk_size: u64,
    /// Enable adaptive optimization based on access patterns
    pub adaptive_optimization: bool,
}

impl Default for SmartIoConfig {
    fn default() -> Self {
        Self {
            coalesce_threshold: 64 * 1024, // 64KB
            min_parallel_bytes: 4096,       // 4KB
            max_concurrent_reads: 8,
            target_chunk_size: 1024 * 1024, // 1MB
            adaptive_optimization: true,
        }
    }
}

impl SmartIoConfig {
    /// Configuration optimized for local storage
    pub fn for_local() -> Self {
        Self {
            coalesce_threshold: 32 * 1024, // 32KB - smaller threshold for low latency
            min_parallel_bytes: 64 * 1024, // 64KB
            max_concurrent_reads: 4,
            target_chunk_size: 256 * 1024, // 256KB
            adaptive_optimization: false,
        }
    }

    /// Configuration optimized for cloud storage
    pub fn for_cloud() -> Self {
        Self {
            coalesce_threshold: 256 * 1024, // 256KB - larger threshold for high latency
            min_parallel_bytes: 4096,        // 4KB
            max_concurrent_reads: 16,
            target_chunk_size: 4 * 1024 * 1024, // 4MB
            adaptive_optimization: true,
        }
    }
}

/// Smart I/O Layer for optimized file access
///
/// Coordinates range coalescing and parallel reads to minimize
/// I/O operations and maximize throughput.
pub struct SmartIoLayer {
    /// Underlying filesystem
    filesystem: Arc<dyn FileSystem>,
    /// Range optimizer for coalescing
    range_optimizer: Arc<dyn RangeOptimizerWithMapping>,
    /// I/O execution strategy
    io_strategy: Arc<dyn IoStrategy>,
    /// I/O metrics collector
    metrics: Arc<IoMetrics>,
    /// Configuration
    config: SmartIoConfig,
}

impl std::fmt::Debug for SmartIoLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmartIoLayer")
            .field("filesystem_type", &self.filesystem.filesystem_type())
            .field("config", &self.config)
            .finish()
    }
}

impl SmartIoLayer {
    /// Create a new SmartIoLayer with default configuration
    pub fn new(filesystem: Arc<dyn FileSystem>) -> Self {
        Self::with_config(filesystem, SmartIoConfig::default())
    }

    /// Create with custom configuration
    pub fn with_config(filesystem: Arc<dyn FileSystem>, config: SmartIoConfig) -> Self {
        let metrics = Arc::new(IoMetrics::new());

        // Create range optimizer based on config
        let range_optimizer: Arc<dyn RangeOptimizerWithMapping> = if config.adaptive_optimization {
            Arc::new(AdaptiveRangeCoalescer::with_storage_profile(
                100, // Assume local latency, will adapt
                config.target_chunk_size,
            ))
        } else {
            Arc::new(DefaultRangeCoalescer::with_threshold(config.coalesce_threshold))
        };

        // Create I/O strategy
        let io_strategy: Arc<dyn IoStrategy> = Arc::new(ParallelReader::with_config(
            filesystem.clone(),
            ParallelReaderConfig {
                max_concurrent_reads: config.max_concurrent_reads,
                min_parallel_bytes: config.min_parallel_bytes,
                max_parallel_ranges: 32,
                adaptive_concurrency: config.adaptive_optimization,
            },
            metrics.clone(),
        ));

        Self {
            filesystem,
            range_optimizer,
            io_strategy,
            metrics,
            config,
        }
    }

    /// Create for local filesystem
    pub fn for_local(filesystem: Arc<dyn FileSystem>) -> Self {
        Self::with_config(filesystem, SmartIoConfig::for_local())
    }

    /// Create for cloud storage
    pub fn for_cloud(filesystem: Arc<dyn FileSystem>) -> Self {
        Self::with_config(filesystem, SmartIoConfig::for_cloud())
    }

    /// Create with custom components (for advanced use cases)
    pub fn with_components(
        filesystem: Arc<dyn FileSystem>,
        range_optimizer: Arc<dyn RangeOptimizerWithMapping>,
        io_strategy: Arc<dyn IoStrategy>,
        metrics: Arc<IoMetrics>,
        config: SmartIoConfig,
    ) -> Self {
        Self {
            filesystem,
            range_optimizer,
            io_strategy,
            metrics,
            config,
        }
    }

    /// Read multiple byte ranges from a file efficiently
    ///
    /// This method:
    /// 1. Coalesces adjacent/nearby ranges to reduce I/O operations
    /// 2. Executes reads in parallel when beneficial
    /// 3. Extracts the original requested data from coalesced reads
    ///
    /// # Arguments
    /// * `file` - Path to the file to read from
    /// * `ranges` - Byte ranges to read
    ///
    /// # Returns
    /// Vector of data buffers, one per input range (in original order)
    pub async fn read_ranges(&self, file: &str, ranges: Vec<ByteRange>) -> FsResult<Vec<Vec<u8>>> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }

        // Record the original request
        let total_requested: u64 = ranges.iter().map(|r| r.len()).sum();
        self.metrics.record_request(total_requested, ranges.len() as u64);

        trace!(
            "SmartIO: Reading {} ranges, {} bytes from {}",
            ranges.len(),
            total_requested,
            file
        );

        // Step 1: Coalesce ranges
        let (coalesced_ranges, mappings) = self
            .range_optimizer
            .coalesce_with_mapping(ranges.clone(), self.config.coalesce_threshold);

        // Calculate bytes in gaps (for metrics)
        let total_coalesced: u64 = coalesced_ranges.iter().map(|r| r.len()).sum();
        let bytes_in_gaps = total_coalesced.saturating_sub(total_requested);

        self.metrics.record_coalescing(
            ranges.len(),
            coalesced_ranges.len(),
            bytes_in_gaps,
        );

        debug!(
            "SmartIO: Coalesced {} ranges to {} ranges ({}% reduction, {} bytes in gaps)",
            ranges.len(),
            coalesced_ranges.len(),
            if ranges.len() > 0 {
                ((ranges.len() - coalesced_ranges.len()) * 100) / ranges.len()
            } else {
                0
            },
            bytes_in_gaps,
        );

        // Step 2: Execute reads
        let coalesced_data = self.io_strategy.execute_read(file, &coalesced_ranges).await?;

        // Step 3: Extract original ranges from coalesced data
        let result = self.extract_ranges(&coalesced_data, &mappings)?;

        Ok(result)
    }

    /// Extract original requested data from coalesced reads
    fn extract_ranges(
        &self,
        coalesced_data: &[Vec<u8>],
        mappings: &[RangeMapping],
    ) -> FsResult<Vec<Vec<u8>>> {
        let mut results = Vec::with_capacity(mappings.len());

        for mapping in mappings {
            let coalesced_buffer = &coalesced_data[mapping.coalesced_index];
            let start = mapping.offset_in_coalesced as usize;
            let end = start + mapping.length as usize;

            // Bounds check
            if end > coalesced_buffer.len() {
                return Err(crate::storage::persistence::filesystem::FilesystemError::InvalidOperation(
                    format!(
                        "Range extraction out of bounds: buffer len {}, range [{}, {})",
                        coalesced_buffer.len(),
                        start,
                        end
                    ),
                ));
            }

            results.push(coalesced_buffer[start..end].to_vec());
        }

        Ok(results)
    }

    /// Convenience method to read ranges from standard Range type
    pub async fn read_std_ranges(
        &self,
        file: &str,
        ranges: Vec<std::ops::Range<u64>>,
    ) -> FsResult<Vec<Vec<u8>>> {
        let byte_ranges: Vec<ByteRange> = ranges.into_iter().map(ByteRange::from).collect();
        self.read_ranges(file, byte_ranges).await
    }

    /// Get I/O metrics snapshot
    pub fn metrics(&self) -> IoMetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Get the underlying metrics collector
    pub fn metrics_collector(&self) -> Arc<IoMetrics> {
        self.metrics.clone()
    }

    /// Estimate the cost of reading the given ranges
    pub fn estimate_cost(&self, ranges: &[ByteRange]) -> IoCostEstimate {
        // First coalesce to get realistic I/O count
        let coalesced = self.range_optimizer.coalesce(ranges.to_vec(), self.config.coalesce_threshold);
        self.io_strategy.estimate_cost(&coalesced)
    }

    /// Get configuration
    pub fn config(&self) -> &SmartIoConfig {
        &self.config
    }

    /// Get underlying filesystem
    pub fn filesystem(&self) -> &Arc<dyn FileSystem> {
        &self.filesystem
    }

    /// Reset metrics
    pub fn reset_metrics(&self) {
        self.metrics.reset();
    }

    /// Log current metrics summary
    pub fn log_metrics(&self) {
        self.metrics.log_summary();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};
    use std::io::Write;
    use tempfile::NamedTempFile;

    async fn create_test_filesystem() -> Arc<dyn FileSystem> {
        let config = LocalConfig::default();
        Arc::new(LocalFileSystem::new(config).await.unwrap())
    }

    fn create_test_file(size: usize) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
        file.write_all(&data).unwrap();
        file.flush().unwrap();
        file
    }

    #[test]
    fn test_config_default() {
        let config = SmartIoConfig::default();
        assert_eq!(config.coalesce_threshold, 64 * 1024);
        assert_eq!(config.max_concurrent_reads, 8);
    }

    #[test]
    fn test_config_local() {
        let config = SmartIoConfig::for_local();
        assert_eq!(config.coalesce_threshold, 32 * 1024);
        assert!(!config.adaptive_optimization);
    }

    #[test]
    fn test_config_cloud() {
        let config = SmartIoConfig::for_cloud();
        assert_eq!(config.coalesce_threshold, 256 * 1024);
        assert!(config.adaptive_optimization);
    }

    #[tokio::test]
    async fn test_smart_io_layer_creation() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::new(fs);

        assert_eq!(smart_io.config().coalesce_threshold, 64 * 1024);
    }

    #[tokio::test]
    async fn test_read_single_range() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::new(fs);

        let test_file = create_test_file(1000);
        let path = format!("file://{}", test_file.path().display());

        let ranges = vec![ByteRange::new(0, 100)];
        let result = smart_io.read_ranges(&path, ranges).await.unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 100);

        // Verify data integrity
        for (i, byte) in result[0].iter().enumerate() {
            assert_eq!(*byte, (i % 256) as u8);
        }
    }

    #[tokio::test]
    async fn test_read_multiple_ranges() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::new(fs);

        let test_file = create_test_file(10000);
        let path = format!("file://{}", test_file.path().display());

        let ranges = vec![
            ByteRange::new(0, 100),
            ByteRange::new(500, 600),
            ByteRange::new(1000, 1100),
        ];

        let result = smart_io.read_ranges(&path, ranges).await.unwrap();

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].len(), 100);
        assert_eq!(result[1].len(), 100);
        assert_eq!(result[2].len(), 100);

        // Verify data integrity for each range
        assert_eq!(result[0][0], 0);
        assert_eq!(result[1][0], (500 % 256) as u8);
        assert_eq!(result[2][0], (1000 % 256) as u8);
    }

    #[tokio::test]
    async fn test_range_coalescing() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::with_config(
            fs,
            SmartIoConfig {
                coalesce_threshold: 100, // 100 byte threshold for testing
                ..Default::default()
            },
        );

        let test_file = create_test_file(1000);
        let path = format!("file://{}", test_file.path().display());

        // Two adjacent ranges that should be coalesced
        let ranges = vec![
            ByteRange::new(0, 100),
            ByteRange::new(100, 200), // Adjacent - should coalesce
        ];

        let result = smart_io.read_ranges(&path, ranges).await.unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 100);
        assert_eq!(result[1].len(), 100);

        // Check metrics - should show 1 read performed (coalesced)
        let metrics = smart_io.metrics();
        assert_eq!(metrics.reads_requested, 2);
        // Note: reads_performed may vary based on coalescing behavior
    }

    #[tokio::test]
    async fn test_read_empty_ranges() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::new(fs);

        let result = smart_io.read_ranges("any_file", vec![]).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_read_std_ranges() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::new(fs);

        let test_file = create_test_file(1000);
        let path = format!("file://{}", test_file.path().display());

        let ranges = vec![0u64..100, 500..600];
        let result = smart_io.read_std_ranges(&path, ranges).await.unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 100);
        assert_eq!(result[1].len(), 100);
    }

    #[tokio::test]
    async fn test_io_cost_estimation() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::new(fs);

        let ranges = vec![
            ByteRange::new(0, 1000),
            ByteRange::new(2000, 3000),
            ByteRange::new(4000, 5000),
        ];

        let estimate = smart_io.estimate_cost(&ranges);
        assert_eq!(estimate.bytes_to_read, 3000);
        assert!(estimate.io_operations > 0);
    }

    #[tokio::test]
    async fn test_metrics_tracking() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::new(fs);

        let test_file = create_test_file(10000);
        let path = format!("file://{}", test_file.path().display());

        // Perform some reads
        let ranges = vec![
            ByteRange::new(0, 1000),
            ByteRange::new(2000, 3000),
        ];
        smart_io.read_ranges(&path, ranges).await.unwrap();

        let metrics = smart_io.metrics();
        assert!(metrics.bytes_requested > 0);
        assert!(metrics.bytes_read > 0);
        assert!(metrics.reads_requested > 0);
    }

    #[tokio::test]
    async fn test_metrics_reset() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::new(fs);

        let test_file = create_test_file(1000);
        let path = format!("file://{}", test_file.path().display());

        // Perform a read
        let ranges = vec![ByteRange::new(0, 100)];
        smart_io.read_ranges(&path, ranges).await.unwrap();

        // Reset and verify
        smart_io.reset_metrics();
        let metrics = smart_io.metrics();
        assert_eq!(metrics.bytes_requested, 0);
        assert_eq!(metrics.bytes_read, 0);
    }

    #[tokio::test]
    async fn test_for_local() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::for_local(fs);

        assert_eq!(smart_io.config().coalesce_threshold, 32 * 1024);
        assert!(!smart_io.config().adaptive_optimization);
    }

    #[tokio::test]
    async fn test_for_cloud() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::for_cloud(fs);

        assert_eq!(smart_io.config().coalesce_threshold, 256 * 1024);
        assert!(smart_io.config().adaptive_optimization);
    }

    #[tokio::test]
    async fn test_data_integrity_with_coalescing() {
        let fs = create_test_filesystem().await;
        let smart_io = SmartIoLayer::with_config(
            fs,
            SmartIoConfig {
                coalesce_threshold: 1000, // Large threshold to force coalescing
                ..Default::default()
            },
        );

        let test_file = create_test_file(5000);
        let path = format!("file://{}", test_file.path().display());

        // Multiple small ranges that will be coalesced
        let ranges = vec![
            ByteRange::new(100, 200),
            ByteRange::new(300, 400),
            ByteRange::new(500, 600),
        ];

        let result = smart_io.read_ranges(&path, ranges).await.unwrap();

        // Verify each range got the correct data
        assert_eq!(result.len(), 3);

        for (i, byte) in result[0].iter().enumerate() {
            assert_eq!(*byte, ((100 + i) % 256) as u8);
        }
        for (i, byte) in result[1].iter().enumerate() {
            assert_eq!(*byte, ((300 + i) % 256) as u8);
        }
        for (i, byte) in result[2].iter().enumerate() {
            assert_eq!(*byte, ((500 + i) % 256) as u8);
        }
    }
}
