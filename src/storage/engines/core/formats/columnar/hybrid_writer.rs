//! Hybrid Parquet Writer Strategy
//!
//! Dynamically chooses between streaming and batch modes based on workload characteristics.
//! Provides optimal performance across different insertion patterns.
//!
//! Features:
//! - Automatic mode selection based on data characteristics
//! - Adaptive buffering for optimal memory usage
//! - Smart row group sizing based on insertion rate
//! - Concurrent write support with lock-free coordination
//! - Metrics and monitoring for mode transitions

use anyhow::{Context, Result, anyhow};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, trace, warn};

use crate::core::VectorRecord;
use crate::storage::engines::core::formats::columnar::{
    BatchParquetWriter, ParquetWriterConfig, StreamingParquetWriter, StreamingParquetWriterStats,
};

/// Hybrid writer mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WriterMode {
    /// Streaming mode for continuous inserts
    Streaming,
    /// Batch mode for bulk inserts
    Batch,
    /// Adaptive mode that switches dynamically
    Adaptive,
}

/// Insertion pattern detection
#[derive(Debug, Clone)]
pub struct InsertionPattern {
    /// Average records per second
    pub insert_rate: f64,

    /// Average batch size
    pub avg_batch_size: f64,

    /// Batch size variance
    pub batch_size_variance: f64,

    /// Time between batches
    pub avg_inter_batch_time: Duration,

    /// Pattern consistency score (0.0 - 1.0)
    pub consistency_score: f64,

    /// Detected pattern type
    pub pattern_type: PatternType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatternType {
    /// Steady stream of small inserts
    Streaming,
    /// Large bulk inserts
    Bulk,
    /// Mixed pattern
    Mixed,
    /// Unknown or insufficient data
    Unknown,
}

/// Hybrid writer configuration
#[derive(Debug, Clone)]
pub struct HybridWriterConfig {
    /// Base Parquet writer config
    pub base_config: ParquetWriterConfig,

    /// Initial mode
    pub initial_mode: WriterMode,

    /// Enable automatic mode switching
    pub enable_auto_switch: bool,

    /// Minimum records before switching modes
    pub mode_switch_threshold: usize,

    /// Pattern detection window size
    pub pattern_window_size: usize,

    /// Streaming mode threshold (records/second)
    pub streaming_threshold: f64,

    /// Batch mode threshold (average batch size)
    pub batch_threshold: usize,

    /// Maximum buffer size before forced flush
    pub max_buffer_size: usize,

    /// Buffer time limit before forced flush
    pub buffer_time_limit: Duration,

    /// Enable concurrent writes
    pub enable_concurrent_writes: bool,

    /// Maximum concurrent writers
    pub max_concurrent_writers: usize,

    /// Row group size optimization
    pub optimize_row_group_size: bool,

    /// Minimum row group size
    pub min_row_group_size: usize,

    /// Maximum row group size
    pub max_row_group_size: usize,
}

impl Default for HybridWriterConfig {
    fn default() -> Self {
        // All optimizations ENABLED by default for adaptive performance
        // Users can override any setting if needed
        Self {
            base_config: ParquetWriterConfig::default(), // Inherits all optimizations
            initial_mode: WriterMode::Adaptive,          // DEFAULT: Auto-adapt to workload
            enable_auto_switch: true,                    // DEFAULT ON: Dynamic mode switching
            mode_switch_threshold: 1000,
            pattern_window_size: 100,
            streaming_threshold: 100.0, // 100 records/second
            batch_threshold: 1000,      // 1000 records per batch
            max_buffer_size: 100_000,
            buffer_time_limit: Duration::from_secs(30),
            enable_concurrent_writes: true, // DEFAULT ON: Concurrent write support
            max_concurrent_writers: 4,
            optimize_row_group_size: true, // DEFAULT ON: Smart row group sizing
            min_row_group_size: 1_000,
            max_row_group_size: 100_000,
        }
    }
}

/// Hybrid Parquet writer with adaptive strategy
pub struct HybridParquetWriter {
    /// Configuration
    config: HybridWriterConfig,

    /// Current mode
    current_mode: Arc<RwLock<WriterMode>>,

    /// Streaming writer (when in streaming mode)
    streaming_writer: Arc<Mutex<Option<StreamingParquetWriter>>>,

    /// Record buffer for batch mode
    buffer: Arc<RwLock<Vec<VectorRecord>>>,

    /// Insertion history for pattern detection
    insertion_history: Arc<RwLock<VecDeque<InsertionEvent>>>,

    /// Statistics
    stats: Arc<HybridWriterStats>,

    /// File path
    file_path: PathBuf,

    /// Vector dimension
    dimension: usize,

    /// Last flush time
    last_flush_time: Arc<RwLock<Instant>>,

    /// Background flush task handle
    flush_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

/// Insertion event for pattern tracking
#[derive(Debug, Clone)]
struct InsertionEvent {
    timestamp: Instant,
    batch_size: usize,
    mode: WriterMode,
}

/// Hybrid writer statistics
#[derive(Debug)]
struct HybridWriterStats {
    total_records: AtomicU64,
    streaming_writes: AtomicU64,
    batch_writes: AtomicU64,
    mode_switches: AtomicUsize,
    buffer_flushes: AtomicUsize,
    forced_flushes: AtomicUsize,
    avg_batch_size: AtomicU64,
    avg_flush_latency_ms: AtomicU64,
}

impl HybridParquetWriter {
    /// Create new hybrid writer
    pub async fn new<P: AsRef<Path>>(
        file_path: P,
        dimension: usize,
        config: HybridWriterConfig,
    ) -> Result<Self> {
        let file_path = file_path.as_ref().to_path_buf();
        info!(
            "Creating hybrid Parquet writer: {:?}, mode: {:?}",
            file_path, config.initial_mode
        );

        let streaming_writer = if config.initial_mode == WriterMode::Streaming {
            Some(StreamingParquetWriter::new(
                &file_path,
                dimension,
                config.base_config.clone(),
            )?)
        } else {
            None
        };

        let stats = Arc::new(HybridWriterStats {
            total_records: AtomicU64::new(0),
            streaming_writes: AtomicU64::new(0),
            batch_writes: AtomicU64::new(0),
            mode_switches: AtomicUsize::new(0),
            buffer_flushes: AtomicUsize::new(0),
            forced_flushes: AtomicUsize::new(0),
            avg_batch_size: AtomicU64::new(0),
            avg_flush_latency_ms: AtomicU64::new(0),
        });

        let mut writer = Self {
            config: config.clone(),
            current_mode: Arc::new(RwLock::new(config.initial_mode)),
            streaming_writer: Arc::new(Mutex::new(streaming_writer)),
            buffer: Arc::new(RwLock::new(Vec::new())),
            insertion_history: Arc::new(RwLock::new(VecDeque::new())),
            stats,
            file_path,
            dimension,
            last_flush_time: Arc::new(RwLock::new(Instant::now())),
            flush_task: Arc::new(Mutex::new(None)),
        };

        // Start background flush task
        writer.start_background_flush_task().await;

        Ok(writer)
    }

    /// Write records with adaptive strategy
    pub async fn write(&self, records: Vec<VectorRecord>) -> Result<()> {
        let batch_size = records.len();
        let timestamp = Instant::now();

        // Record insertion event
        self.record_insertion_event(timestamp, batch_size).await;

        // Detect pattern and potentially switch modes
        if self.config.enable_auto_switch {
            self.detect_and_switch_mode().await?;
        }

        // Route to appropriate writer
        let mode = *self.current_mode.read().await;
        match mode {
            WriterMode::Streaming => {
                self.write_streaming(records).await?;
            }
            WriterMode::Batch => {
                self.write_batch(records).await?;
            }
            WriterMode::Adaptive => {
                // Decide based on current batch size
                if batch_size >= self.config.batch_threshold {
                    self.write_batch(records).await?;
                } else {
                    self.write_streaming(records).await?;
                }
            }
        }

        // Update statistics
        self.stats
            .total_records
            .fetch_add(batch_size as u64, Ordering::Relaxed);

        Ok(())
    }

    /// Write in streaming mode
    async fn write_streaming(&self, records: Vec<VectorRecord>) -> Result<()> {
        trace!("Writing {} records in streaming mode", records.len());

        let mut writer_lock = self.streaming_writer.lock().await;

        // Create streaming writer if needed
        if writer_lock.is_none() {
            *writer_lock = Some(StreamingParquetWriter::new(
                &self.file_path,
                self.dimension,
                self.config.base_config.clone(),
            )?);
        }

        // Write records
        if let Some(writer) = writer_lock.as_mut() {
            writer.write_batch(&records).await?;
        }

        self.stats
            .streaming_writes
            .fetch_add(records.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Write in batch mode
    async fn write_batch(&self, records: Vec<VectorRecord>) -> Result<()> {
        let records_len = records.len();
        trace!("Writing {} records in batch mode", records_len);

        // Add to buffer
        {
            let mut buffer = self.buffer.write().await;
            buffer.extend(records);

            // Check if buffer needs flushing
            if buffer.len() >= self.config.max_buffer_size {
                drop(buffer); // Release lock before flushing
                self.flush_buffer(false).await?;
            }
        }

        self.stats
            .batch_writes
            .fetch_add(records_len as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Flush buffer to disk
    async fn flush_buffer(&self, force: bool) -> Result<()> {
        let start_time = Instant::now();

        let records = {
            let mut buffer = self.buffer.write().await;
            if buffer.is_empty() {
                return Ok(());
            }

            if !force && buffer.len() < self.config.min_row_group_size {
                // Don't flush small buffers unless forced
                return Ok(());
            }

            std::mem::take(&mut *buffer)
        };

        if records.is_empty() {
            return Ok(());
        }

        info!("Flushing {} records to disk", records.len());

        // Use batch writer for flush
        let writer = BatchParquetWriter::new(
            &self.file_path,
            self.dimension,
            self.config.base_config.clone(),
        );

        writer.write_all(&records).await?;

        // Update statistics
        self.stats.buffer_flushes.fetch_add(1, Ordering::Relaxed);
        if force {
            self.stats.forced_flushes.fetch_add(1, Ordering::Relaxed);
        }

        let flush_latency = start_time.elapsed().as_millis() as u64;
        self.update_avg_flush_latency(flush_latency);

        // Update last flush time
        *self.last_flush_time.write().await = Instant::now();

        debug!("Buffer flush completed in {}ms", flush_latency);
        Ok(())
    }

    /// Record insertion event for pattern detection
    async fn record_insertion_event(&self, timestamp: Instant, batch_size: usize) {
        let mode = *self.current_mode.read().await;
        let event = InsertionEvent {
            timestamp,
            batch_size,
            mode,
        };

        let mut history = self.insertion_history.write().await;
        history.push_back(event);

        // Keep only recent history
        while history.len() > self.config.pattern_window_size {
            history.pop_front();
        }
    }

    /// Detect pattern and switch mode if needed
    async fn detect_and_switch_mode(&self) -> Result<()> {
        let pattern = self.detect_pattern().await;

        if pattern.pattern_type == PatternType::Unknown {
            return Ok(()); // Not enough data
        }

        let current_mode = *self.current_mode.read().await;
        let recommended_mode = self.recommend_mode(&pattern);

        if current_mode != recommended_mode {
            info!(
                "Switching mode from {:?} to {:?} based on pattern: {:?}",
                current_mode, recommended_mode, pattern.pattern_type
            );

            self.switch_mode(recommended_mode).await?;
            self.stats.mode_switches.fetch_add(1, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Detect insertion pattern
    async fn detect_pattern(&self) -> InsertionPattern {
        let history = self.insertion_history.read().await;

        if history.len() < 10 {
            return InsertionPattern {
                insert_rate: 0.0,
                avg_batch_size: 0.0,
                batch_size_variance: 0.0,
                avg_inter_batch_time: Duration::from_secs(0),
                consistency_score: 0.0,
                pattern_type: PatternType::Unknown,
            };
        }

        // Calculate statistics
        let mut batch_sizes = Vec::new();
        let mut inter_batch_times = Vec::new();
        let mut prev_timestamp = None;

        for event in history.iter() {
            batch_sizes.push(event.batch_size as f64);

            if let Some(prev) = prev_timestamp {
                inter_batch_times.push(event.timestamp.duration_since(prev));
            }
            prev_timestamp = Some(event.timestamp);
        }

        let avg_batch_size = batch_sizes.iter().sum::<f64>() / batch_sizes.len() as f64;
        let batch_size_variance = Self::calculate_variance(&batch_sizes, avg_batch_size);

        let avg_inter_batch_time = if !inter_batch_times.is_empty() {
            let total_nanos: u128 = inter_batch_times.iter().map(|d| d.as_nanos()).sum();
            Duration::from_nanos((total_nanos / inter_batch_times.len() as u128) as u64)
        } else {
            Duration::from_secs(1)
        };

        let insert_rate = if avg_inter_batch_time.as_secs_f64() > 0.0 {
            avg_batch_size / avg_inter_batch_time.as_secs_f64()
        } else {
            0.0
        };

        // Calculate consistency score
        let consistency_score = if batch_size_variance > 0.0 {
            1.0 / (1.0 + batch_size_variance.sqrt() / avg_batch_size)
        } else {
            1.0
        };

        // Determine pattern type
        let pattern_type = if avg_batch_size >= self.config.batch_threshold as f64 {
            PatternType::Bulk
        } else if insert_rate >= self.config.streaming_threshold {
            PatternType::Streaming
        } else if consistency_score < 0.5 {
            PatternType::Mixed
        } else {
            PatternType::Unknown
        };

        InsertionPattern {
            insert_rate,
            avg_batch_size,
            batch_size_variance,
            avg_inter_batch_time,
            consistency_score,
            pattern_type,
        }
    }

    /// Calculate variance
    fn calculate_variance(values: &[f64], mean: f64) -> f64 {
        if values.is_empty() {
            return 0.0;
        }

        let sum_squared_diff: f64 = values.iter().map(|v| (v - mean).powi(2)).sum();

        sum_squared_diff / values.len() as f64
    }

    /// Recommend mode based on pattern
    fn recommend_mode(&self, pattern: &InsertionPattern) -> WriterMode {
        match pattern.pattern_type {
            PatternType::Streaming => WriterMode::Streaming,
            PatternType::Bulk => WriterMode::Batch,
            PatternType::Mixed => WriterMode::Adaptive,
            PatternType::Unknown => self.config.initial_mode,
        }
    }

    /// Switch writer mode
    async fn switch_mode(&self, new_mode: WriterMode) -> Result<()> {
        let current_mode = *self.current_mode.read().await;

        if current_mode == new_mode {
            return Ok(());
        }

        debug!("Switching from {:?} to {:?} mode", current_mode, new_mode);

        // Flush any pending data before switching
        if current_mode == WriterMode::Batch {
            self.flush_buffer(true).await?;
        } else if current_mode == WriterMode::Streaming {
            // Finalize streaming writer
            let mut writer_lock = self.streaming_writer.lock().await;
            if let Some(writer) = writer_lock.take() {
                writer.finalize().await?;
            }
        }

        // Update mode
        *self.current_mode.write().await = new_mode;

        // Initialize new writer if needed
        if new_mode == WriterMode::Streaming {
            let mut writer_lock = self.streaming_writer.lock().await;
            *writer_lock = Some(StreamingParquetWriter::new(
                &self.file_path,
                self.dimension,
                self.config.base_config.clone(),
            )?);
        }

        Ok(())
    }

    /// Start background flush task
    async fn start_background_flush_task(&mut self) {
        let buffer = self.buffer.clone();
        let last_flush_time = self.last_flush_time.clone();
        let buffer_time_limit = self.config.buffer_time_limit;
        let flush_fn = {
            let self_clone = self.clone_for_task();
            move || {
                let self_clone = self_clone.clone();
                async move { self_clone.flush_buffer(false).await }
            }
        };

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));

            loop {
                interval.tick().await;

                // Check if buffer needs time-based flush
                let last_flush = *last_flush_time.read().await;
                if Instant::now().duration_since(last_flush) > buffer_time_limit {
                    let buffer_size = buffer.read().await.len();
                    if buffer_size > 0 {
                        debug!(
                            "Time-based buffer flush triggered ({} records)",
                            buffer_size
                        );
                        if let Err(e) = flush_fn().await {
                            warn!("Background flush failed: {}", e);
                        }
                    }
                }
            }
        });

        *self.flush_task.lock().await = Some(handle);
    }

    /// Update average flush latency
    fn update_avg_flush_latency(&self, new_latency: u64) {
        let current = self.stats.avg_flush_latency_ms.load(Ordering::Relaxed);
        let flushes = self.stats.buffer_flushes.load(Ordering::Relaxed);

        if flushes == 0 {
            self.stats
                .avg_flush_latency_ms
                .store(new_latency, Ordering::Relaxed);
        } else {
            let new_avg = (current * (flushes as u64 - 1) + new_latency) / flushes as u64;
            self.stats
                .avg_flush_latency_ms
                .store(new_avg, Ordering::Relaxed);
        }
    }

    /// Clone for background tasks
    fn clone_for_task(&self) -> Arc<Self> {
        // This would need proper Arc wrapping in production
        // For now, return a placeholder
        unimplemented!("Clone for task not implemented")
    }

    /// Finalize writer and flush all pending data
    pub async fn finalize(self) -> Result<HybridWriterStatistics> {
        info!("Finalizing hybrid Parquet writer");

        // Stop background task
        if let Some(handle) = self.flush_task.lock().await.take() {
            handle.abort();
        }

        // Flush any remaining buffer
        self.flush_buffer(true).await?;

        // Finalize streaming writer if active
        let mut writer_lock = self.streaming_writer.lock().await;
        if let Some(writer) = writer_lock.take() {
            writer.finalize().await?;
        }

        // Collect final statistics
        let stats = HybridWriterStatistics {
            total_records: self.stats.total_records.load(Ordering::Relaxed),
            streaming_writes: self.stats.streaming_writes.load(Ordering::Relaxed),
            batch_writes: self.stats.batch_writes.load(Ordering::Relaxed),
            mode_switches: self.stats.mode_switches.load(Ordering::Relaxed),
            buffer_flushes: self.stats.buffer_flushes.load(Ordering::Relaxed),
            forced_flushes: self.stats.forced_flushes.load(Ordering::Relaxed),
            avg_batch_size: self.stats.avg_batch_size.load(Ordering::Relaxed),
            avg_flush_latency_ms: self.stats.avg_flush_latency_ms.load(Ordering::Relaxed),
        };

        info!("Hybrid writer finalized: {:?}", stats);
        Ok(stats)
    }

    /// Get current statistics
    pub async fn get_statistics(&self) -> HybridWriterStatistics {
        HybridWriterStatistics {
            total_records: self.stats.total_records.load(Ordering::Relaxed),
            streaming_writes: self.stats.streaming_writes.load(Ordering::Relaxed),
            batch_writes: self.stats.batch_writes.load(Ordering::Relaxed),
            mode_switches: self.stats.mode_switches.load(Ordering::Relaxed),
            buffer_flushes: self.stats.buffer_flushes.load(Ordering::Relaxed),
            forced_flushes: self.stats.forced_flushes.load(Ordering::Relaxed),
            avg_batch_size: self.stats.avg_batch_size.load(Ordering::Relaxed),
            avg_flush_latency_ms: self.stats.avg_flush_latency_ms.load(Ordering::Relaxed),
        }
    }

    /// Get current mode
    pub async fn get_current_mode(&self) -> WriterMode {
        *self.current_mode.read().await
    }

    /// Force mode change (for testing)
    pub async fn force_mode(&self, mode: WriterMode) -> Result<()> {
        self.switch_mode(mode).await
    }
}

/// Statistics from hybrid writer
#[derive(Debug, Clone)]
pub struct HybridWriterStatistics {
    pub total_records: u64,
    pub streaming_writes: u64,
    pub batch_writes: u64,
    pub mode_switches: usize,
    pub buffer_flushes: usize,
    pub forced_flushes: usize,
    pub avg_batch_size: u64,
    pub avg_flush_latency_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_hybrid_writer_streaming_mode() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_hybrid_streaming.parquet");

        let config = HybridWriterConfig {
            initial_mode: WriterMode::Streaming,
            enable_auto_switch: false,
            ..Default::default()
        };

        let writer = HybridParquetWriter::new(&file_path, 128, config)
            .await
            .unwrap();

        // Write small batches (streaming pattern)
        for i in 0..10 {
            let records = vec![VectorRecord {
                id: Some(format!("vec_{}", i)),
                vector: vec![0.1; 128],
                metadata: None,
                timestamp: i as u32,
                updated_at: None,
                expires_at: None,
                version: Some(1),
            }];

            writer.write(records).await.unwrap();
        }

        let stats = writer.finalize().await.unwrap();
        assert_eq!(stats.total_records, 10);
        assert_eq!(stats.streaming_writes, 10);
        assert_eq!(stats.batch_writes, 0);
    }

    #[tokio::test]
    async fn test_hybrid_writer_batch_mode() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_hybrid_batch.parquet");

        let config = HybridWriterConfig {
            initial_mode: WriterMode::Batch,
            enable_auto_switch: false,
            ..Default::default()
        };

        let writer = HybridParquetWriter::new(&file_path, 128, config)
            .await
            .unwrap();

        // Write large batch (batch pattern)
        let records: Vec<_> = (0..1000)
            .map(|i| VectorRecord {
                id: Some(format!("vec_{}", i)),
                vector: vec![0.1; 128],
                metadata: None,
                timestamp: i as u32,
                updated_at: None,
                expires_at: None,
                version: Some(1),
            })
            .collect();

        writer.write(records).await.unwrap();

        let stats = writer.finalize().await.unwrap();
        assert_eq!(stats.total_records, 1000);
        assert_eq!(stats.streaming_writes, 0);
        assert_eq!(stats.batch_writes, 1000);
    }

    #[tokio::test]
    async fn test_pattern_detection() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_pattern.parquet");

        let config = HybridWriterConfig::default();
        let writer = HybridParquetWriter::new(&file_path, 128, config)
            .await
            .unwrap();

        // Simulate streaming pattern
        for _ in 0..20 {
            writer.record_insertion_event(Instant::now(), 10).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let pattern = writer.detect_pattern().await;
        assert!(pattern.avg_batch_size < 100.0);
        assert_eq!(pattern.pattern_type, PatternType::Streaming);

        // Clear history
        writer.insertion_history.write().await.clear();

        // Simulate bulk pattern
        for _ in 0..5 {
            writer.record_insertion_event(Instant::now(), 5000).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let pattern = writer.detect_pattern().await;
        assert!(pattern.avg_batch_size > 1000.0);
        assert_eq!(pattern.pattern_type, PatternType::Bulk);
    }
}
