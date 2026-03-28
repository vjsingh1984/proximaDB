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

    // === Vector Bounds Statistics (TD-040) ===
    /// Minimum L2 norm across all vectors in this write batch
    pub vector_norm_min: Option<f32>,

    /// Maximum L2 norm across all vectors in this write batch
    pub vector_norm_max: Option<f32>,

    /// Mean L2 norm across all vectors in this write batch
    pub vector_norm_mean: Option<f32>,

    /// Per-dimension minimum values (for distance bound estimation)
    pub vector_component_min: Option<Vec<f32>>,

    /// Per-dimension maximum values (for distance bound estimation)
    pub vector_component_max: Option<Vec<f32>>,
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
            self.compression_ratio =
                1.0 - (self.compressed_size as f64 / self.uncompressed_size as f64);
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

    /// Update vector norm bounds from a batch of vectors (TD-040).
    ///
    /// Computes L2 norms and per-dimension min/max for distance bound estimation.
    /// Called during write to track statistics that enable row group pruning
    /// based on approximate distance from query vector.
    pub fn update_vector_bounds(&mut self, vectors: &[Vec<f32>]) {
        if vectors.is_empty() {
            return;
        }

        let dim = vectors[0].len();

        let mut norm_min = self.vector_norm_min.unwrap_or(f32::MAX);
        let mut norm_max = self.vector_norm_max.unwrap_or(f32::MIN);
        let mut norm_sum = self.vector_norm_mean.unwrap_or(0.0)
            * self.total_records as f32;
        let mut comp_min = self.vector_component_min.clone().unwrap_or_else(|| vec![f32::MAX; dim]);
        let mut comp_max = self.vector_component_max.clone().unwrap_or_else(|| vec![f32::MIN; dim]);

        // Ensure dimension matches
        if comp_min.len() != dim {
            comp_min = vec![f32::MAX; dim];
            comp_max = vec![f32::MIN; dim];
        }

        for vec in vectors {
            // Compute L2 norm
            let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm < norm_min { norm_min = norm; }
            if norm > norm_max { norm_max = norm; }
            norm_sum += norm;

            // Update per-dimension bounds
            for (i, &v) in vec.iter().enumerate() {
                if i < dim {
                    if v < comp_min[i] { comp_min[i] = v; }
                    if v > comp_max[i] { comp_max[i] = v; }
                }
            }
        }

        let total = self.total_records + vectors.len();
        self.vector_norm_min = Some(norm_min);
        self.vector_norm_max = Some(norm_max);
        self.vector_norm_mean = if total > 0 { Some(norm_sum / total as f32) } else { None };
        self.vector_component_min = Some(comp_min);
        self.vector_component_max = Some(comp_max);
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

#[cfg(test)]
mod tests {
    use super::*;

    // === StreamingParquetWriterStats tests ===

    #[test]
    fn test_writer_stats_default() {
        let stats = StreamingParquetWriterStats::default();
        assert_eq!(stats.total_records, 0);
        assert_eq!(stats.file_size, 0);
        assert_eq!(stats.uncompressed_size, 0);
        assert_eq!(stats.compressed_size, 0);
        assert!((stats.compression_ratio - 0.0).abs() < f64::EPSILON);
        assert!(!stats.quantization_enabled);
        assert!(stats.quantization_levels.is_empty());
        assert_eq!(stats.write_duration, Duration::default());
    }

    #[test]
    fn test_writer_stats_new_equals_default() {
        let a = StreamingParquetWriterStats::new();
        let b = StreamingParquetWriterStats::default();
        assert_eq!(a.total_records, b.total_records);
        assert_eq!(a.file_size, b.file_size);
    }

    #[test]
    fn test_update_compression_ratio_no_data() {
        let mut stats = StreamingParquetWriterStats::new();
        stats.update_compression_ratio();
        assert!((stats.compression_ratio - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_update_compression_ratio_50_percent() {
        let mut stats = StreamingParquetWriterStats::new();
        stats.uncompressed_size = 1000;
        stats.compressed_size = 500;
        stats.update_compression_ratio();
        assert!((stats.compression_ratio - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_update_compression_ratio_no_compression() {
        let mut stats = StreamingParquetWriterStats::new();
        stats.uncompressed_size = 1000;
        stats.compressed_size = 1000;
        stats.update_compression_ratio();
        assert!((stats.compression_ratio - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_update_compression_ratio_90_percent() {
        let mut stats = StreamingParquetWriterStats::new();
        stats.uncompressed_size = 1000;
        stats.compressed_size = 100;
        stats.update_compression_ratio();
        assert!((stats.compression_ratio - 0.9).abs() < 1e-10);
    }

    #[test]
    fn test_update_throughput_zero_duration() {
        let mut stats = StreamingParquetWriterStats::new();
        stats.total_records = 100;
        stats.update_throughput();
        assert!((stats.throughput_records_per_sec - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_update_throughput() {
        let mut stats = StreamingParquetWriterStats::new();
        stats.total_records = 1000;
        stats.uncompressed_size = 2 * 1024 * 1024; // 2 MB
        stats.write_duration = Duration::from_secs(2);
        stats.update_throughput();
        assert!((stats.throughput_records_per_sec - 500.0).abs() < 1e-10);
        assert!((stats.throughput_mb_per_sec - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_writer_stats_merge() {
        let mut a = StreamingParquetWriterStats::new();
        a.total_records = 100;
        a.unique_ids = 90;
        a.duplicate_ids = 10;
        a.uncompressed_size = 5000;
        a.compressed_size = 2500;
        a.row_groups_written = 2;
        a.write_duration = Duration::from_millis(100);

        let b = StreamingParquetWriterStats {
            total_records: 200,
            unique_ids: 180,
            duplicate_ids: 20,
            uncompressed_size: 10000,
            compressed_size: 4000,
            row_groups_written: 3,
            write_duration: Duration::from_millis(200),
            ..Default::default()
        };

        a.merge(&b);

        assert_eq!(a.total_records, 300);
        assert_eq!(a.unique_ids, 270);
        assert_eq!(a.duplicate_ids, 30);
        assert_eq!(a.uncompressed_size, 15000);
        assert_eq!(a.compressed_size, 6500);
        assert_eq!(a.row_groups_written, 5);
        assert_eq!(a.write_duration, Duration::from_millis(300));
        // Compression ratio should be updated after merge
        let expected_ratio = 1.0 - (6500.0 / 15000.0);
        assert!((a.compression_ratio - expected_ratio).abs() < 1e-10);
    }

    #[test]
    fn test_writer_stats_summary_contains_info() {
        let mut stats = StreamingParquetWriterStats::new();
        stats.total_records = 500;
        stats.row_groups_written = 3;
        stats.bloom_filter_count = 2;
        let summary = stats.summary();
        assert!(summary.contains("500 records"));
        assert!(summary.contains("3"));
        assert!(summary.contains("Bloom filters: 2"));
    }

    // === BatchWriteStats tests ===

    #[test]
    fn test_batch_write_stats_default() {
        let stats = BatchWriteStats::default();
        assert_eq!(stats.batch_size, 0);
        assert!(!stats.success);
        assert!(stats.error_message.is_none());
        assert_eq!(stats.processing_time, Duration::default());
    }

    // === AggregatedBatchStats tests ===

    #[test]
    fn test_aggregated_batch_stats_default() {
        let stats = AggregatedBatchStats::default();
        assert_eq!(stats.total_batches, 0);
        assert_eq!(stats.successful_batches, 0);
        assert_eq!(stats.failed_batches, 0);
        assert_eq!(stats.total_records, 0);
    }

    #[test]
    fn test_success_rate_no_batches() {
        let stats = AggregatedBatchStats::default();
        assert!((stats.success_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_success_rate_all_successful() {
        let mut agg = AggregatedBatchStats::default();
        let batch = BatchWriteStats {
            batch_size: 100,
            processing_time: Duration::from_millis(10),
            success: true,
            ..Default::default()
        };
        agg.add_batch(&batch);
        agg.add_batch(&batch);
        assert!((agg.success_rate() - 100.0).abs() < 1e-10);
    }

    #[test]
    fn test_success_rate_mixed() {
        let mut agg = AggregatedBatchStats::default();
        let success_batch = BatchWriteStats {
            batch_size: 50,
            processing_time: Duration::from_millis(5),
            success: true,
            ..Default::default()
        };
        let fail_batch = BatchWriteStats {
            batch_size: 30,
            processing_time: Duration::from_millis(3),
            success: false,
            error_message: Some("test error".to_string()),
            ..Default::default()
        };
        agg.add_batch(&success_batch);
        agg.add_batch(&fail_batch);

        assert_eq!(agg.total_batches, 2);
        assert_eq!(agg.successful_batches, 1);
        assert_eq!(agg.failed_batches, 1);
        assert!((agg.success_rate() - 50.0).abs() < 1e-10);
    }

    #[test]
    fn test_add_batch_updates_averages() {
        let mut agg = AggregatedBatchStats::default();

        let batch1 = BatchWriteStats {
            batch_size: 100,
            processing_time: Duration::from_millis(10),
            success: true,
            ..Default::default()
        };
        agg.add_batch(&batch1);
        assert!((agg.avg_batch_size - 100.0).abs() < 1e-10);
        assert_eq!(agg.max_batch_size, 100);
        assert_eq!(agg.min_batch_size, 100);

        let batch2 = BatchWriteStats {
            batch_size: 200,
            processing_time: Duration::from_millis(20),
            success: true,
            ..Default::default()
        };
        agg.add_batch(&batch2);
        assert!((agg.avg_batch_size - 150.0).abs() < 1e-10);
        assert_eq!(agg.max_batch_size, 200);
        assert_eq!(agg.min_batch_size, 100);
        assert_eq!(agg.total_records, 300);
    }

    #[test]
    fn test_add_batch_min_batch_size_first_batch() {
        let mut agg = AggregatedBatchStats::default();
        // min_batch_size starts at 0, first batch should set it
        let batch = BatchWriteStats {
            batch_size: 50,
            processing_time: Duration::from_millis(5),
            success: true,
            ..Default::default()
        };
        agg.add_batch(&batch);
        assert_eq!(agg.min_batch_size, 50);
    }
}
