//! Streaming compression for real-time vector workloads
//!
//! Provides low-latency compression and decompression for vector streams with:
//! - Adaptive buffer sizing based on throughput
//! - Concurrent compression pipelines
//! - Memory-efficient streaming operations
//! - Real-time performance monitoring

use anyhow::Result;
use parking_lot::RwLock;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, info, trace, warn};

use crate::core::serialization::VectorSerializationConfig;
use proximadb_runtime_common::pool::VectorMemoryPool;

/// Backwards-compat alias for [`SerializationStreamingConfig`].
pub type StreamingConfig = SerializationStreamingConfig;

/// Configuration for streaming compression
#[derive(Debug, Clone)]
pub struct SerializationStreamingConfig {
    /// Buffer size for batching vectors
    pub buffer_size: usize,
    /// Maximum time to wait before flushing buffer
    pub flush_timeout: Duration,
    /// Number of concurrent compression workers
    pub worker_count: usize,
    /// Channel capacity for work queues
    pub channel_capacity: usize,
    /// Enable adaptive buffer sizing
    pub adaptive_sizing: bool,
    /// Target compression latency (microseconds)
    pub target_latency_us: u64,
    /// Enable performance monitoring
    pub enable_monitoring: bool,
}

impl Default for SerializationStreamingConfig {
    fn default() -> Self {
        Self {
            buffer_size: 100,                         // Vectors per batch
            flush_timeout: Duration::from_millis(10), // 10ms max latency
            worker_count: num_cpus::get().min(8),     // Up to 8 workers
            channel_capacity: 1000,
            adaptive_sizing: true,
            target_latency_us: 5000, // 5ms target
            enable_monitoring: true,
        }
    }
}

/// Backwards-compat alias for [`SerializationStreamingMetrics`].
pub type StreamingMetrics = SerializationStreamingMetrics;

/// Performance metrics for streaming compression
#[derive(Debug, Clone, Default)]
pub struct SerializationStreamingMetrics {
    /// Total number of vectors processed
    pub vectors_processed: u64,
    /// Number of batches processed
    pub batches_processed: u64,
    /// Total compressed output bytes
    pub total_compressed_bytes: u64,
    /// Total uncompressed input bytes
    pub total_uncompressed_bytes: u64,
    /// Average per-batch latency in microseconds
    pub average_latency_us: u64,
    /// Maximum per-batch latency in microseconds
    pub max_latency_us: u64,
    /// Overall compression ratio (compressed / uncompressed)
    pub compression_ratio: f32,
    /// Sustained throughput in vectors per second
    pub throughput_vectors_per_sec: f32,
    /// Number of currently active compression workers
    pub active_workers: usize,
    /// Current depth of the pending work queue
    pub queue_depth: usize,
}

impl SerializationStreamingMetrics {
    /// Log a human-readable summary of streaming compression metrics
    pub fn print_summary(&self) {
        info!("🌊 Streaming Compression Metrics:");
        info!(
            "   Processed: {} vectors in {} batches",
            self.vectors_processed, self.batches_processed
        );
        info!(
            "   Throughput: {:.1} vectors/sec",
            self.throughput_vectors_per_sec
        );
        info!(
            "   Latency: avg={} μs, max={} μs",
            self.average_latency_us, self.max_latency_us
        );
        info!(
            "   Compression: {:.3} ratio ({} → {} bytes)",
            self.compression_ratio, self.total_uncompressed_bytes, self.total_compressed_bytes
        );
        info!(
            "   Workers: {} active, queue depth: {}",
            self.active_workers, self.queue_depth
        );
    }
}

/// Work item for streaming compression
#[derive(Debug)]
struct CompressionWork {
    batch_id: u64,
    vectors: Vec<Vec<f32>>,
    config: VectorSerializationConfig,
    response_tx: oneshot::Sender<Result<CompressionResult>>,
    submitted_at: Instant,
}

/// Result of compression work
#[derive(Debug)]
pub struct CompressionResult {
    /// Batch identifier for correlation
    pub batch_id: u64,
    /// Compressed output bytes
    pub compressed_data: Vec<u8>,
    /// Original uncompressed size in bytes
    pub original_size: usize,
    /// Compressed size in bytes
    pub compressed_size: usize,
    /// Compression ratio (compressed / original)
    pub compression_ratio: f32,
    /// Time taken to compress this batch
    pub processing_time: Duration,
}

/// Streaming vector compressor for real-time workloads
pub struct StreamingCompressor {
    config: SerializationStreamingConfig,
    #[allow(dead_code)]
    memory_pool: Arc<VectorMemoryPool>,
    work_tx: mpsc::Sender<CompressionWork>,
    metrics: Arc<RwLock<SerializationStreamingMetrics>>,
    workers: Vec<JoinHandle<()>>,
    next_batch_id: Arc<std::sync::atomic::AtomicU64>,
    adaptive_controller: Arc<RwLock<AdaptiveController>>,
}

/// Adaptive controller for buffer sizing and performance tuning
#[derive(Debug)]
struct AdaptiveController {
    recent_latencies: VecDeque<u64>,
    recent_throughputs: VecDeque<f32>,
    last_adjustment: Instant,
    current_buffer_size: usize,
    performance_window: Duration,
}

impl AdaptiveController {
    fn new(initial_buffer_size: usize) -> Self {
        Self {
            recent_latencies: VecDeque::with_capacity(100),
            recent_throughputs: VecDeque::with_capacity(100),
            last_adjustment: Instant::now(),
            current_buffer_size: initial_buffer_size,
            performance_window: Duration::from_secs(10),
        }
    }

    fn record_performance(&mut self, latency_us: u64, throughput: f32) {
        self.recent_latencies.push_back(latency_us);
        self.recent_throughputs.push_back(throughput);

        // Keep only recent samples
        if self.recent_latencies.len() > 100 {
            self.recent_latencies.pop_front();
        }
        if self.recent_throughputs.len() > 100 {
            self.recent_throughputs.pop_front();
        }
    }

    fn should_adjust(&self, target_latency_us: u64) -> bool {
        if self.last_adjustment.elapsed() < self.performance_window {
            return false;
        }

        if self.recent_latencies.len() < 10 {
            return false;
        }

        let avg_latency =
            self.recent_latencies.iter().sum::<u64>() / self.recent_latencies.len() as u64;

        // Adjust if significantly off target
        (avg_latency as f64 - target_latency_us as f64).abs() / target_latency_us as f64 > 0.2
    }

    fn adjust_buffer_size(&mut self, target_latency_us: u64) -> usize {
        if !self.should_adjust(target_latency_us) {
            return self.current_buffer_size;
        }

        let avg_latency =
            self.recent_latencies.iter().sum::<u64>() / self.recent_latencies.len() as u64;

        let new_size = if avg_latency > target_latency_us {
            // Latency too high, reduce buffer size
            (self.current_buffer_size as f32 * 0.8).round() as usize
        } else {
            // Latency acceptable, try to increase throughput
            (self.current_buffer_size as f32 * 1.2).round() as usize
        };

        self.current_buffer_size = new_size.clamp(10, 1000);
        self.last_adjustment = Instant::now();

        debug!(
            "🎛️ Adaptive buffer size adjustment: {} → {} (latency: {} μs)",
            self.current_buffer_size, new_size, avg_latency
        );

        self.current_buffer_size
    }
}

impl StreamingCompressor {
    /// Create a new streaming compressor
    pub fn new(config: SerializationStreamingConfig) -> Result<Self> {
        let memory_pool = Arc::new(VectorMemoryPool::new());
        let (work_tx, work_rx) = mpsc::channel(config.channel_capacity);
        let metrics = Arc::new(RwLock::new(SerializationStreamingMetrics::default()));
        let next_batch_id = Arc::new(std::sync::atomic::AtomicU64::new(1));
        let adaptive_controller =
            Arc::new(RwLock::new(AdaptiveController::new(config.buffer_size)));

        // Start worker threads
        let workers = Self::start_workers(
            config.worker_count,
            work_rx,
            memory_pool.clone(),
            metrics.clone(),
        )?;

        Ok(Self {
            config,
            memory_pool,
            work_tx,
            metrics,
            workers,
            next_batch_id,
            adaptive_controller,
        })
    }

    /// Compress a batch of vectors asynchronously
    pub async fn compress_batch(
        &self,
        vectors: Vec<Vec<f32>>,
        config: VectorSerializationConfig,
    ) -> Result<CompressionResult> {
        let batch_id = self
            .next_batch_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (response_tx, response_rx) = oneshot::channel();

        let work = CompressionWork {
            batch_id,
            vectors,
            config,
            response_tx,
            submitted_at: Instant::now(),
        };

        self.work_tx
            .send(work)
            .await
            .map_err(|_| anyhow::anyhow!("Compression workers are shut down"))?;

        response_rx
            .await
            .map_err(|_| anyhow::anyhow!("Compression work was cancelled"))?
    }

    /// Compress vectors with streaming batching
    pub async fn compress_stream(
        &self,
        mut vectors: Vec<Vec<f32>>,
        config: VectorSerializationConfig,
    ) -> Result<Vec<CompressionResult>> {
        let mut results = Vec::new();
        let mut buffer = Vec::new();
        let buffer_size = if self.config.adaptive_sizing {
            self.adaptive_controller.read().current_buffer_size
        } else {
            self.config.buffer_size
        };

        // Process vectors in batches
        for vector in vectors.drain(..) {
            buffer.push(vector);

            if buffer.len() >= buffer_size {
                let batch = std::mem::take(&mut buffer);
                let result = self.compress_batch(batch, config.clone()).await?;

                // Update adaptive controller
                if self.config.adaptive_sizing {
                    let throughput =
                        result.compressed_size as f32 / result.processing_time.as_secs_f32();
                    self.adaptive_controller
                        .write()
                        .record_performance(result.processing_time.as_micros() as u64, throughput);
                }

                results.push(result);
            }
        }

        // Process remaining vectors
        if !buffer.is_empty() {
            let result = self.compress_batch(buffer, config).await?;
            results.push(result);
        }

        Ok(results)
    }

    /// Get current performance metrics
    pub fn metrics(&self) -> SerializationStreamingMetrics {
        self.metrics.read().clone()
    }

    /// Adjust compression parameters based on performance
    pub async fn optimize_performance(&self) -> Result<()> {
        if !self.config.adaptive_sizing {
            return Ok(());
        }

        let mut controller = self.adaptive_controller.write();
        let new_size = controller.adjust_buffer_size(self.config.target_latency_us);

        drop(controller);

        debug!(
            "🎯 Performance optimization completed, buffer size: {}",
            new_size
        );

        Ok(())
    }

    /// Start compression worker threads
    fn start_workers(
        worker_count: usize,
        work_rx: mpsc::Receiver<CompressionWork>,
        memory_pool: Arc<VectorMemoryPool>,
        metrics: Arc<RwLock<SerializationStreamingMetrics>>,
    ) -> Result<Vec<JoinHandle<()>>> {
        let work_rx = Arc::new(tokio::sync::Mutex::new(work_rx));
        let mut handles = Vec::new();

        for worker_id in 0..worker_count {
            let work_rx = work_rx.clone();
            let memory_pool = memory_pool.clone();
            let metrics = metrics.clone();

            let handle = tokio::spawn(async move {
                Self::worker_loop(worker_id, work_rx, memory_pool, metrics).await;
            });

            handles.push(handle);
        }

        info!("🏭 Started {} streaming compression workers", worker_count);

        Ok(handles)
    }

    /// Worker loop for processing compression jobs
    async fn worker_loop(
        worker_id: usize,
        work_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<CompressionWork>>>,
        memory_pool: Arc<VectorMemoryPool>,
        metrics: Arc<RwLock<SerializationStreamingMetrics>>,
    ) {
        trace!("🔧 Worker {} started", worker_id);

        loop {
            let work = {
                let mut rx = work_rx.lock().await;
                rx.recv().await
            };

            match work {
                Some(work) => {
                    let start_time = Instant::now();
                    let queue_time = work.submitted_at.elapsed();

                    let result = Self::process_compression_work(&work, &memory_pool).await;
                    let processing_time = start_time.elapsed();

                    // Update metrics
                    {
                        let mut m = metrics.write();
                        m.vectors_processed += work.vectors.len() as u64;
                        m.batches_processed += 1;

                        if let Ok(ref res) = result {
                            m.total_uncompressed_bytes += res.original_size as u64;
                            m.total_compressed_bytes += res.compressed_size as u64;
                            m.compression_ratio =
                                m.total_compressed_bytes as f32 / m.total_uncompressed_bytes as f32;

                            let latency_us = processing_time.as_micros() as u64;
                            m.average_latency_us = (m.average_latency_us + latency_us) / 2;
                            m.max_latency_us = m.max_latency_us.max(latency_us);
                        }
                    }

                    // Send result
                    if work.response_tx.send(result).is_err() {
                        warn!(
                            "🚫 Worker {}: Failed to send result for batch {}",
                            worker_id, work.batch_id
                        );
                    }

                    trace!(
                        "⚡ Worker {} processed batch {} in {:?} (queue: {:?})",
                        worker_id, work.batch_id, processing_time, queue_time
                    );
                }
                None => {
                    debug!("🔧 Worker {} shutting down", worker_id);
                    break;
                }
            }
        }
    }

    /// Process a single compression work item
    async fn process_compression_work(
        work: &CompressionWork,
        memory_pool: &VectorMemoryPool,
    ) -> Result<CompressionResult> {
        let start_time = Instant::now();

        // Calculate original size
        let original_size = work
            .vectors
            .iter()
            .map(|v| v.len() * 4) // f32 = 4 bytes
            .sum::<usize>();

        // Serialize vectors using pooled buffer
        let compressed_data = {
            let mut pooled = memory_pool.serialization_buffers.acquire();
            let buf = &mut *pooled;
            buf.clear();
            let est = work.vectors.iter().map(|v| v.len() * 4 + 4).sum();
            buf.reserve(est);
            for v in &work.vectors {
                let vd = work.config.serialize_vector(v)?;
                buf.extend_from_slice(&(vd.len() as u32).to_le_bytes());
                buf.extend_from_slice(&vd);
            }
            buf.clone()
        };

        let processing_time = start_time.elapsed();
        let compressed_size = compressed_data.len();
        let compression_ratio = compressed_size as f32 / original_size as f32;

        Ok(CompressionResult {
            batch_id: work.batch_id,
            compressed_data,
            original_size,
            compressed_size,
            compression_ratio,
            processing_time,
        })
    }

    /// Shutdown the compressor and wait for workers to complete
    pub async fn shutdown(self) -> Result<()> {
        // Close work channel
        drop(self.work_tx);

        // Wait for workers to complete
        for worker in self.workers {
            worker.await?;
        }

        info!("🛑 Streaming compressor shutdown complete");

        Ok(())
    }
}

/// Streaming decompressor for real-time workloads
pub struct StreamingDecompressor {
    config: VectorSerializationConfig,
}

impl StreamingDecompressor {
    /// Create a new streaming decompressor
    pub fn new(config: VectorSerializationConfig) -> Self {
        Self { config }
    }

    /// Decompress a batch of compressed data
    pub async fn decompress_batch(&self, compressed_data: &[u8]) -> Result<Vec<Vec<f32>>> {
        let start_time = Instant::now();

        let vectors = {
            let mut result = Vec::new();
            let mut cursor = 0usize;
            while cursor + 4 <= compressed_data.len() {
                let len =
                    u32::from_le_bytes(compressed_data[cursor..cursor + 4].try_into().unwrap())
                        as usize;
                cursor += 4;
                if cursor + len > compressed_data.len() {
                    return Err(anyhow::anyhow!("Invalid vector data: length mismatch"));
                }
                result.push(
                    self.config
                        .deserialize_vector(&compressed_data[cursor..cursor + len])?,
                );
                cursor += len;
            }
            result
        };

        let processing_time = start_time.elapsed();

        trace!(
            "🔓 Decompressed {} vectors in {:?}",
            vectors.len(),
            processing_time
        );

        Ok(vectors)
    }

    /// Decompress multiple compression results
    pub async fn decompress_results(
        &self,
        results: Vec<CompressionResult>,
    ) -> Result<Vec<Vec<f32>>> {
        let mut all_vectors = Vec::new();

        for result in results {
            let vectors = self.decompress_batch(&result.compressed_data).await?;
            all_vectors.extend(vectors);
        }

        Ok(all_vectors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::serialization::CompressionAlgorithm;
    use anyhow::Context;

    fn create_test_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
        (0..count)
            .map(|i| {
                (0..dimension)
                    .map(|j| (i * dimension + j) as f32 * 0.001)
                    .collect()
            })
            .collect()
    }

    #[tokio::test]
    async fn test_streaming_compression_basic() -> Result<()> {
        let config = SerializationStreamingConfig {
            worker_count: 2,
            buffer_size: 10,
            ..Default::default()
        };

        let compressor =
            StreamingCompressor::new(config).context("Failed to create streaming compressor")?;
        let vectors = create_test_vectors(25, 128);

        let vector_config = VectorSerializationConfig {
            compression_algorithm: CompressionAlgorithm::Zstd,
            ..Default::default()
        };

        let results = compressor
            .compress_stream(vectors.clone(), vector_config.clone())
            .await
            .context("Failed to compress stream")?;

        assert!(!results.is_empty());

        // Test decompression
        let decompressor = StreamingDecompressor::new(vector_config);
        let decompressed = decompressor
            .decompress_results(results)
            .await
            .context("Failed to decompress results")?;

        assert_eq!(vectors.len(), decompressed.len());

        compressor
            .shutdown()
            .await
            .context("Failed to shutdown compressor")?;

        Ok(())
    }

    #[tokio::test]
    async fn test_batch_compression() -> Result<()> {
        let config = SerializationStreamingConfig::default();
        let compressor =
            StreamingCompressor::new(config).context("Failed to create streaming compressor")?;

        let vectors = create_test_vectors(50, 256);
        let vector_config = VectorSerializationConfig::default();

        let result = compressor
            .compress_batch(vectors.clone(), vector_config.clone())
            .await
            .context("Failed to compress batch")?;

        assert!(result.compression_ratio > 0.0);
        assert!(result.processing_time.as_millis() < 1000);

        // Test decompression
        let decompressor = StreamingDecompressor::new(vector_config);
        let decompressed = decompressor
            .decompress_batch(&result.compressed_data)
            .await
            .context("Failed to decompress batch")?;

        assert_eq!(vectors.len(), decompressed.len());

        compressor
            .shutdown()
            .await
            .context("Failed to shutdown compressor")?;

        Ok(())
    }

    #[tokio::test]
    async fn test_performance_monitoring() -> Result<()> {
        let mut config = SerializationStreamingConfig::default();
        config.enable_monitoring = true;
        config.worker_count = 1;

        let compressor =
            StreamingCompressor::new(config).context("Failed to create streaming compressor")?;
        let vectors = create_test_vectors(100, 512);
        let vector_config = VectorSerializationConfig::default();

        let _results = compressor
            .compress_stream(vectors, vector_config)
            .await
            .context("Failed to compress stream")?;

        let metrics = compressor.metrics();
        assert!(metrics.vectors_processed > 0);
        assert!(metrics.batches_processed > 0);
        assert!(metrics.compression_ratio > 0.0);

        metrics.print_summary();

        compressor
            .shutdown()
            .await
            .context("Failed to shutdown compressor")?;

        Ok(())
    }

    #[tokio::test]
    async fn test_adaptive_sizing() -> Result<()> {
        let mut config = SerializationStreamingConfig::default();
        config.adaptive_sizing = true;
        config.target_latency_us = 1000; // 1ms target
        config.buffer_size = 20;

        let compressor =
            StreamingCompressor::new(config).context("Failed to create streaming compressor")?;

        // Process multiple batches to trigger adaptation
        for _ in 0..5 {
            let vectors = create_test_vectors(50, 128);
            let vector_config = VectorSerializationConfig::default();

            let _results = compressor
                .compress_stream(vectors, vector_config)
                .await
                .context("Failed to compress stream")?;

            // Trigger optimization
            compressor
                .optimize_performance()
                .await
                .context("Failed to optimize performance")?;
        }

        let metrics = compressor.metrics();
        assert!(metrics.vectors_processed > 0);

        compressor
            .shutdown()
            .await
            .context("Failed to shutdown compressor")?;

        Ok(())
    }

    #[tokio::test]
    async fn test_concurrent_compression() -> Result<()> {
        let config = SerializationStreamingConfig {
            worker_count: 4,
            channel_capacity: 100,
            ..Default::default()
        };

        let compressor = Arc::new(
            StreamingCompressor::new(config).context("Failed to create streaming compressor")?,
        );

        // Start multiple concurrent compression tasks
        let mut handles = Vec::new();

        for _i in 0..10 {
            let compressor = compressor.clone();
            let handle = tokio::spawn(async move {
                let vectors = create_test_vectors(20, 256);
                let vector_config = VectorSerializationConfig::default();

                compressor.compress_batch(vectors, vector_config).await
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            let result = handle
                .await
                .context("Failed to join compression task")?
                .context("Failed to compress batch")?;
            assert!(result.compression_ratio > 0.0);
        }

        let final_compressor = Arc::try_unwrap(compressor)
            .map_err(|_| anyhow::anyhow!("Failed to unwrap Arc: still has multiple references"))?;
        final_compressor
            .shutdown()
            .await
            .context("Failed to shutdown compressor")?;

        Ok(())
    }
}
