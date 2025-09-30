//! Writer Statistics and Metrics
//!
//! This module provides statistics tracking for Parquet write operations,
//! including performance metrics, compression ratios, and data characteristics.

use std::time::Duration;

/// Comprehensive statistics for Parquet write operations
#[derive(Debug, Clone, Default)]
pub struct StreamingParquetWriterStats {
    // === File Information ===
    /// File path where data was written
    pub file_path: String,

    /// Total file size in bytes
    pub file_size: u64,

    // === Record Statistics ===
    /// Total number of records written
    pub total_records: usize,

    /// Number of unique IDs
    pub unique_ids: usize,

    /// Number of duplicate IDs encountered
    pub duplicate_ids: usize,

    // === Size Statistics ===
    /// Total uncompressed size in bytes
    pub uncompressed_size: usize,

    /// Total compressed size in bytes
    pub compressed_size: usize,

    /// Size of vector data (uncompressed)
    pub vector_data_size: usize,

    /// Size of metadata (uncompressed)
    pub metadata_size: usize,

    // === Compression Statistics ===
    /// Overall compression ratio (compressed/uncompressed)
    pub compression_ratio: f64,

    /// Vector compression ratio
    pub vector_compression_ratio: f64,

    /// Metadata compression ratio
    pub metadata_compression_ratio: f64,

    // === Row Group Statistics ===
    /// Total number of row groups written
    pub total_row_groups: usize,

    /// Number of row groups written
    pub row_groups_written: usize,

    /// Average row group size
    pub avg_row_group_size: usize,

    /// Smallest row group size
    pub min_row_group_size: usize,

    /// Largest row group size
    pub max_row_group_size: usize,

    // === Bloom Filter Statistics ===
    /// Number of bloom filters created
    pub bloom_filter_count: usize,

    /// Total size of bloom filters
    pub bloom_filter_total_size: usize,

    // === Performance Statistics ===
    /// Total write duration
    pub write_duration: Duration,

    /// Time spent in compression
    pub compression_duration: Duration,

    /// Time spent building indexes
    pub index_build_duration: Duration,

    /// Write throughput (records/second)
    pub throughput_records_per_sec: f64,

    /// Write throughput (MB/second)
    pub throughput_mb_per_sec: f64,

    // === Quantization Statistics ===
    /// Whether quantization was used
    pub quantization_enabled: bool,

    /// Quantization levels used
    pub quantization_levels: Vec<String>,

    /// Space saved by quantization
    pub quantization_space_saved: usize,

    // === Metadata Statistics ===
    /// Number of filterable columns
    pub filterable_columns_count: usize,

    /// Number of records with metadata
    pub records_with_metadata: usize,

    /// Average metadata fields per record
    pub avg_metadata_fields: f32,
}

impl StreamingParquetWriterStats {
    /// Create new statistics instance
    pub fn new() -> Self {
        Self::default()
    }

    /// Update compression ratio as space savings
    /// Formula: 1 - (compressed/uncompressed)
    /// - 0.0 = no compression
    /// - 0.5 = 50% space savings
    /// - 0.9 = 90% space savings
    pub fn update_compression_ratio(&mut self) {
        if self.uncompressed_size > 0 {
            self.compression_ratio = 1.0 - (self.compressed_size as f64 / self.uncompressed_size as f64);
        }
    }

    /// Update throughput metrics
    pub fn update_throughput(&mut self) {
        let duration_secs = self.write_duration.as_secs_f64();
        if duration_secs > 0.0 {
            self.throughput_records_per_sec = self.total_records as f64 / duration_secs;
            self.throughput_mb_per_sec =
                (self.uncompressed_size as f64 / (1024.0 * 1024.0)) / duration_secs;
        }
    }

    /// Merge statistics from another instance
    pub fn merge(&mut self, other: &StreamingParquetWriterStats) {
        self.total_records += other.total_records;
        self.unique_ids += other.unique_ids;
        self.duplicate_ids += other.duplicate_ids;
        self.uncompressed_size += other.uncompressed_size;
        self.compressed_size += other.compressed_size;
        self.vector_data_size += other.vector_data_size;
        self.metadata_size += other.metadata_size;
        self.row_groups_written += other.row_groups_written;
        self.bloom_filter_count += other.bloom_filter_count;
        self.bloom_filter_total_size += other.bloom_filter_total_size;
        self.write_duration += other.write_duration;
        self.compression_duration += other.compression_duration;
        self.index_build_duration += other.index_build_duration;
        self.quantization_space_saved += other.quantization_space_saved;
        self.records_with_metadata += other.records_with_metadata;

        // Update derived metrics
        self.update_compression_ratio();
        self.update_throughput();
    }

    /// Get a human-readable summary
    pub fn summary(&self) -> String {
        format!(
            "Written {} records in {:?}\n\
             Compression: {:.2}x ({} -> {} bytes)\n\
             Throughput: {:.2} records/sec, {:.2} MB/sec\n\
             Row groups: {} (avg size: {})\n\
             Bloom filters: {} (total size: {} bytes)",
            self.total_records,
            self.write_duration,
            1.0 / self.compression_ratio.max(0.001),
            self.uncompressed_size,
            self.compressed_size,
            self.throughput_records_per_sec,
            self.throughput_mb_per_sec,
            self.row_groups_written,
            self.avg_row_group_size,
            self.bloom_filter_count,
            self.bloom_filter_total_size
        )
    }
}

/// Statistics for batch write operations
#[derive(Debug, Clone, Default)]
pub struct BatchWriteStats {
    /// Number of records in batch
    pub batch_size: usize,

    /// Time to process batch
    pub processing_time: Duration,

    /// Size before compression
    pub uncompressed_size: usize,

    /// Size after compression
    pub compressed_size: usize,

    /// Whether batch was written successfully
    pub success: bool,

    /// Error message if failed
    pub error_message: Option<String>,
}

/// Aggregated statistics across multiple batches
#[derive(Debug, Clone, Default)]
pub struct AggregatedBatchStats {
    /// Total batches processed
    pub total_batches: usize,

    /// Successful batches
    pub successful_batches: usize,

    /// Failed batches
    pub failed_batches: usize,

    /// Total records across all batches
    pub total_records: usize,

    /// Total processing time
    pub total_processing_time: Duration,

    /// Average batch size
    pub avg_batch_size: f64,

    /// Average processing time per batch
    pub avg_processing_time: Duration,

    /// Peak batch size
    pub max_batch_size: usize,

    /// Minimum batch size
    pub min_batch_size: usize,
}

impl AggregatedBatchStats {
    /// Add statistics from a batch
    pub fn add_batch(&mut self, stats: &BatchWriteStats) {
        self.total_batches += 1;

        if stats.success {
            self.successful_batches += 1;
        } else {
            self.failed_batches += 1;
        }

        self.total_records += stats.batch_size;
        self.total_processing_time += stats.processing_time;

        self.max_batch_size = self.max_batch_size.max(stats.batch_size);
        self.min_batch_size = if self.min_batch_size == 0 {
            stats.batch_size
        } else {
            self.min_batch_size.min(stats.batch_size)
        };

        // Update averages
        if self.total_batches > 0 {
            self.avg_batch_size = self.total_records as f64 / self.total_batches as f64;
            self.avg_processing_time = self.total_processing_time / self.total_batches as u32;
        }
    }

    /// Get success rate as percentage
    pub fn success_rate(&self) -> f64 {
        if self.total_batches == 0 {
            return 0.0;
        }
        (self.successful_batches as f64 / self.total_batches as f64) * 100.0
    }
}