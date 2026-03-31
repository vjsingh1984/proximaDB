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

//! Parallel Reader for Smart I/O Layer
//!
//! Executes multiple range reads concurrently using tokio tasks.
//! Optimized for cloud storage where parallel requests can
//! significantly improve throughput.

use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;
use tracing::{debug, trace, warn};

use crate::storage::persistence::filesystem::{FileSystem, FilesystemError, FsResult};

use super::metrics::IoMetrics;
use super::traits::{ByteRange, IoCostEstimate, IoStrategy};

/// Parallel reader configuration
#[derive(Debug, Clone)]
pub struct ParallelReaderConfig {
    /// Maximum number of concurrent reads
    pub max_concurrent_reads: usize,
    /// Minimum bytes per range to consider parallelism
    pub min_parallel_bytes: u64,
    /// Maximum ranges to process in parallel
    pub max_parallel_ranges: usize,
    /// Whether to use adaptive concurrency
    pub adaptive_concurrency: bool,
}

impl Default for ParallelReaderConfig {
    fn default() -> Self {
        Self {
            max_concurrent_reads: 8,
            min_parallel_bytes: 4096, // 4KB minimum
            max_parallel_ranges: 32,
            adaptive_concurrency: true,
        }
    }
}

impl ParallelReaderConfig {
    /// Create config optimized for local storage
    pub fn for_local() -> Self {
        Self {
            max_concurrent_reads: 4,
            min_parallel_bytes: 64 * 1024, // 64KB
            max_parallel_ranges: 16,
            adaptive_concurrency: false,
        }
    }

    /// Create config optimized for cloud storage
    pub fn for_cloud() -> Self {
        Self {
            max_concurrent_reads: 16,
            min_parallel_bytes: 4096,
            max_parallel_ranges: 64,
            adaptive_concurrency: true,
        }
    }
}

/// Parallel reader that executes range reads concurrently
pub struct ParallelReader {
    /// Underlying filesystem
    filesystem: Arc<dyn FileSystem>,
    /// Configuration
    config: ParallelReaderConfig,
    /// Concurrency semaphore
    semaphore: Arc<Semaphore>,
    /// I/O metrics collector
    metrics: Arc<IoMetrics>,
}

impl std::fmt::Debug for ParallelReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParallelReader")
            .field("config", &self.config)
            .field("filesystem_type", &self.filesystem.filesystem_type())
            .finish()
    }
}

impl ParallelReader {
    /// Create a new parallel reader
    pub fn new(filesystem: Arc<dyn FileSystem>, metrics: Arc<IoMetrics>) -> Self {
        let config = ParallelReaderConfig::default();
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_reads));

        Self {
            filesystem,
            config,
            semaphore,
            metrics,
        }
    }

    /// Create with custom configuration
    pub fn with_config(
        filesystem: Arc<dyn FileSystem>,
        config: ParallelReaderConfig,
        metrics: Arc<IoMetrics>,
    ) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_reads));

        Self {
            filesystem,
            config,
            semaphore,
            metrics,
        }
    }

    /// Decide whether to use parallel or sequential reads
    fn should_parallelize(&self, ranges: &[ByteRange]) -> bool {
        if ranges.len() <= 1 {
            return false;
        }

        if ranges.len() > self.config.max_parallel_ranges {
            return true; // Always parallelize large batches
        }

        // Check total bytes
        let total_bytes: u64 = ranges.iter().map(|r| r.len()).sum();
        total_bytes >= self.config.min_parallel_bytes
    }

    /// Execute reads sequentially
    async fn sequential_read(&self, file: &str, ranges: &[ByteRange]) -> FsResult<Vec<Vec<u8>>> {
        let mut results = Vec::with_capacity(ranges.len());

        for range in ranges {
            let start = Instant::now();
            let data = self
                .filesystem
                .read_range(file, range.start, range.len())
                .await?;

            self.metrics.record_read(range.len(), start.elapsed());
            results.push(data);
        }

        Ok(results)
    }

    /// Execute reads in parallel
    async fn parallel_read(&self, file: &str, ranges: &[ByteRange]) -> FsResult<Vec<Vec<u8>>> {
        let file = file.to_string();
        let mut handles = Vec::with_capacity(ranges.len());

        for (idx, range) in ranges.iter().enumerate() {
            let fs = self.filesystem.clone();
            let sem = self.semaphore.clone();
            let metrics = self.metrics.clone();
            let file_clone = file.clone();
            let range_clone = range.clone();

            let handle = tokio::spawn(async move {
                // Acquire semaphore permit
                let _permit = sem.acquire().await.map_err(|e| {
                    FilesystemError::InvalidOperation(format!("Semaphore error: {}", e))
                })?;

                let start = Instant::now();
                let result = fs
                    .read_range(&file_clone, range_clone.start, range_clone.len())
                    .await;

                if result.is_ok() {
                    metrics.record_read(range_clone.len(), start.elapsed());
                }

                result.map(|data| (idx, data))
            });

            handles.push(handle);
        }

        // Collect results in order
        let mut results: Vec<Option<Vec<u8>>> = vec![None; ranges.len()];
        let mut errors = Vec::new();

        for handle in handles {
            match handle.await {
                Ok(Ok((idx, data))) => {
                    results[idx] = Some(data);
                }
                Ok(Err(e)) => {
                    errors.push(e);
                }
                Err(join_err) => {
                    warn!("Task join error: {}", join_err);
                    errors.push(FilesystemError::InvalidOperation(format!(
                        "Task join error: {}",
                        join_err
                    )));
                }
            }
        }

        // If any errors occurred, return the first one
        if let Some(err) = errors.into_iter().next() {
            return Err(err);
        }

        // Convert all results to Vec<u8> (all should be Some at this point if no errors occurred)
        let final_results: Vec<Vec<u8>> = results
            .into_iter()
            .enumerate()
            .map(|(idx, opt)| {
                opt.ok_or_else(|| {
                    FilesystemError::InvalidOperation(format!(
                        "Missing result for range index {} (this should not happen if no errors occurred)",
                        idx
                    ))
                })
            })
            .collect::<FsResult<Vec<_>>>()?;

        Ok(final_results)
    }

    /// Get current concurrency level
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Update configuration
    pub fn update_config(&mut self, config: ParallelReaderConfig) {
        self.config = config;
        self.semaphore = Arc::new(Semaphore::new(self.config.max_concurrent_reads));
    }
}

#[async_trait]
impl IoStrategy for ParallelReader {
    async fn execute_read(&self, file: &str, ranges: &[ByteRange]) -> FsResult<Vec<Vec<u8>>> {
        if ranges.is_empty() {
            return Ok(vec![]);
        }

        let start = Instant::now();
        let total_bytes: u64 = ranges.iter().map(|r| r.len()).sum();

        let result = if self.should_parallelize(ranges) {
            debug!(
                "Parallel read: {} ranges, {} bytes from {}",
                ranges.len(),
                total_bytes,
                file
            );
            self.parallel_read(file, ranges).await
        } else {
            trace!(
                "Sequential read: {} ranges, {} bytes from {}",
                ranges.len(),
                total_bytes,
                file
            );
            self.sequential_read(file, ranges).await
        };

        let elapsed = start.elapsed();
        debug!(
            "Read completed: {} ranges, {} bytes in {:?}",
            ranges.len(),
            total_bytes,
            elapsed
        );

        result
    }

    fn estimate_cost(&self, ranges: &[ByteRange]) -> IoCostEstimate {
        let total_bytes: u64 = ranges.iter().map(|r| r.len()).sum();

        // Calculate estimated I/O operations
        // With parallelism, we can do multiple reads at once
        let io_operations = if self.should_parallelize(ranges) {
            // Parallel: ceiling of ranges / max_concurrent
            ranges.len().div_ceil(self.config.max_concurrent_reads)
        } else {
            ranges.len()
        };

        let mut estimate = IoCostEstimate::new(io_operations, total_bytes);
        estimate.recommend_parallel = self.should_parallelize(ranges);
        estimate
    }

    fn strategy_name(&self) -> &'static str {
        "ParallelReader"
    }
}

/// Sequential reader for comparison and fallback
#[derive(Debug)]
pub struct SequentialReader {
    filesystem: Arc<dyn FileSystem>,
    metrics: Arc<IoMetrics>,
}

impl SequentialReader {
    pub fn new(filesystem: Arc<dyn FileSystem>, metrics: Arc<IoMetrics>) -> Self {
        Self {
            filesystem,
            metrics,
        }
    }
}

#[async_trait]
impl IoStrategy for SequentialReader {
    async fn execute_read(&self, file: &str, ranges: &[ByteRange]) -> FsResult<Vec<Vec<u8>>> {
        let mut results = Vec::with_capacity(ranges.len());

        for range in ranges {
            let start = Instant::now();
            let data = self
                .filesystem
                .read_range(file, range.start, range.len())
                .await?;

            self.metrics.record_read(range.len(), start.elapsed());
            results.push(data);
        }

        Ok(results)
    }

    fn estimate_cost(&self, ranges: &[ByteRange]) -> IoCostEstimate {
        let total_bytes: u64 = ranges.iter().map(|r| r.len()).sum();
        let mut estimate = IoCostEstimate::new(ranges.len(), total_bytes);
        estimate.recommend_parallel = false;
        estimate
    }

    fn strategy_name(&self) -> &'static str {
        "SequentialReader"
    }
}

/// Adaptive reader that switches between strategies
pub struct AdaptiveReader {
    parallel: ParallelReader,
    sequential: SequentialReader,
    /// Threshold for switching to parallel (number of ranges)
    parallel_threshold: usize,
    /// Threshold for switching to parallel (total bytes)
    bytes_threshold: u64,
}

impl std::fmt::Debug for AdaptiveReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdaptiveReader")
            .field("parallel_threshold", &self.parallel_threshold)
            .field("bytes_threshold", &self.bytes_threshold)
            .finish()
    }
}

impl AdaptiveReader {
    pub fn new(filesystem: Arc<dyn FileSystem>, metrics: Arc<IoMetrics>) -> Self {
        Self {
            parallel: ParallelReader::new(filesystem.clone(), metrics.clone()),
            sequential: SequentialReader::new(filesystem, metrics),
            parallel_threshold: 4,
            bytes_threshold: 256 * 1024, // 256KB
        }
    }

    fn should_use_parallel(&self, ranges: &[ByteRange]) -> bool {
        let total_bytes: u64 = ranges.iter().map(|r| r.len()).sum();
        ranges.len() >= self.parallel_threshold || total_bytes >= self.bytes_threshold
    }
}

#[async_trait]
impl IoStrategy for AdaptiveReader {
    async fn execute_read(&self, file: &str, ranges: &[ByteRange]) -> FsResult<Vec<Vec<u8>>> {
        if self.should_use_parallel(ranges) {
            self.parallel.execute_read(file, ranges).await
        } else {
            self.sequential.execute_read(file, ranges).await
        }
    }

    fn estimate_cost(&self, ranges: &[ByteRange]) -> IoCostEstimate {
        if self.should_use_parallel(ranges) {
            self.parallel.estimate_cost(ranges)
        } else {
            self.sequential.estimate_cost(ranges)
        }
    }

    fn strategy_name(&self) -> &'static str {
        "AdaptiveReader"
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
        Arc::new(
            LocalFileSystem::new(config)
                .await
                .expect("Failed to create test filesystem"),
        )
    }

    fn create_test_file(size: usize) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("Failed to create temp file for testing");
        let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
        file.write_all(&data)
            .expect("Failed to write test data to temp file");
        file.flush().expect("Failed to flush temp file");
        file
    }

    #[test]
    fn test_parallel_reader_config_default() {
        let config = ParallelReaderConfig::default();
        assert_eq!(config.max_concurrent_reads, 8);
        assert_eq!(config.min_parallel_bytes, 4096);
    }

    #[test]
    fn test_parallel_reader_config_local() {
        let config = ParallelReaderConfig::for_local();
        assert_eq!(config.max_concurrent_reads, 4);
        assert!(!config.adaptive_concurrency);
    }

    #[test]
    fn test_parallel_reader_config_cloud() {
        let config = ParallelReaderConfig::for_cloud();
        assert_eq!(config.max_concurrent_reads, 16);
        assert!(config.adaptive_concurrency);
    }

    #[tokio::test]
    async fn test_parallel_reader_single_range() {
        let fs = create_test_filesystem().await;
        let metrics = Arc::new(IoMetrics::new());
        let reader = ParallelReader::new(fs, metrics);

        let test_file = create_test_file(1000);
        let path = format!("file://{}", test_file.path().display());

        let ranges = vec![ByteRange::new(0, 100)];
        let result = reader
            .execute_read(&path, &ranges)
            .await
            .expect("Failed to execute read for single range test");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 100);

        // Verify data integrity
        for (i, byte) in result[0].iter().enumerate() {
            assert_eq!(*byte, (i % 256) as u8);
        }
    }

    #[tokio::test]
    async fn test_parallel_reader_multiple_ranges() {
        let fs = create_test_filesystem().await;
        let metrics = Arc::new(IoMetrics::new());
        let reader = ParallelReader::new(fs, metrics);

        let test_file = create_test_file(10000);
        let path = format!("file://{}", test_file.path().display());

        let ranges = vec![
            ByteRange::new(0, 100),
            ByteRange::new(500, 600),
            ByteRange::new(1000, 1100),
        ];

        let result = reader
            .execute_read(&path, &ranges)
            .await
            .expect("Failed to execute read for multiple ranges test");

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
    async fn test_parallel_reader_empty_ranges() {
        let fs = create_test_filesystem().await;
        let metrics = Arc::new(IoMetrics::new());
        let reader = ParallelReader::new(fs, metrics);

        let result = reader
            .execute_read("any_file", &[])
            .await
            .expect("Failed to execute read for empty ranges test");
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_sequential_reader() {
        let fs = create_test_filesystem().await;
        let metrics = Arc::new(IoMetrics::new());
        let reader = SequentialReader::new(fs, metrics);

        let test_file = create_test_file(1000);
        let path = format!("file://{}", test_file.path().display());

        let ranges = vec![ByteRange::new(0, 50), ByteRange::new(100, 150)];

        let result = reader
            .execute_read(&path, &ranges)
            .await
            .expect("Failed to execute read for sequential reader test");

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 50);
        assert_eq!(result[1].len(), 50);
    }

    #[tokio::test]
    async fn test_adaptive_reader() {
        let fs = create_test_filesystem().await;
        let metrics = Arc::new(IoMetrics::new());
        let reader = AdaptiveReader::new(fs, metrics);

        let test_file = create_test_file(10000);
        let path = format!("file://{}", test_file.path().display());

        // Small number of ranges - should use sequential
        let small_ranges = vec![ByteRange::new(0, 100), ByteRange::new(200, 300)];
        assert!(!reader.should_use_parallel(&small_ranges));

        // Large number of ranges - should use parallel
        let large_ranges: Vec<ByteRange> = (0..10)
            .map(|i| ByteRange::new(i * 100, i * 100 + 50))
            .collect();
        assert!(reader.should_use_parallel(&large_ranges));

        // Test actual execution
        let result = reader
            .execute_read(&path, &small_ranges)
            .await
            .expect("Failed to execute read for adaptive reader test");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_io_cost_estimate() {
        let fs_result = tokio::runtime::Runtime::new()
            .expect("Failed to create tokio runtime for test")
            .block_on(create_test_filesystem());
        let metrics = Arc::new(IoMetrics::new());
        let reader = ParallelReader::new(fs_result, metrics);

        let ranges = vec![
            ByteRange::new(0, 1000),
            ByteRange::new(1000, 2000),
            ByteRange::new(2000, 3000),
        ];

        let estimate = reader.estimate_cost(&ranges);
        assert_eq!(estimate.bytes_to_read, 3000);
        assert!(estimate.io_operations <= ranges.len());
    }

    #[test]
    fn test_strategy_names() {
        let fs_result = tokio::runtime::Runtime::new()
            .expect("Failed to create tokio runtime for test")
            .block_on(create_test_filesystem());
        let metrics = Arc::new(IoMetrics::new());

        let parallel = ParallelReader::new(fs_result.clone(), metrics.clone());
        let sequential = SequentialReader::new(fs_result.clone(), metrics.clone());
        let adaptive = AdaptiveReader::new(fs_result, metrics);

        assert_eq!(parallel.strategy_name(), "ParallelReader");
        assert_eq!(sequential.strategy_name(), "SequentialReader");
        assert_eq!(adaptive.strategy_name(), "AdaptiveReader");
    }
}
