//! # Common Types for DataSource Connectors
//!
//! This module defines shared types used across the connector interface including
//! table metadata, statistics, and write results. These types enable query planning
//! and cost-based optimization in external query engines.
//!
//! ## Type Categories
//!
//! - **TableInfo**: Schema and metadata for tables/collections
//! - **Statistics**: Row counts, size estimates, and column statistics
//! - **WriteResult**: Results from committed write operations
//!
//! ## Statistics for Query Planning
//!
//! Statistics are crucial for cost-based query optimization. The connector provides:
//!
//! 1. **Table Statistics**: Row count, total size, partition info
//! 2. **Column Statistics**: Min/max values, null count, distinct count
//! 3. **Histograms**: Value distribution for selectivity estimation
//!
//! ## Example
//!
//! ```rust,ignore
//! let table_info = connector.get_table("vectors").await?;
//! println!("Schema: {:?}", table_info.schema);
//!
//! if let Some(stats) = table_info.statistics {
//!     println!("Row count: {}", stats.row_count);
//!     println!("Size: {} bytes", stats.size_bytes);
//! }
//! ```

use arrow::datatypes::Schema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Metadata about a table/collection in ProximaDB.
///
/// Contains schema information, partitioning details, and statistics
/// for query planning and optimization.
#[derive(Debug, Clone)]
pub struct TableInfo {
    /// Table/collection name
    pub name: String,

    /// Arrow schema describing the table structure
    #[allow(dead_code)]
    pub schema: Arc<Schema>,

    /// Partition columns (for partitioned tables)
    pub partitioning: Option<Vec<String>>,

    /// Table properties and configuration
    pub properties: HashMap<String, String>,

    /// Statistics for query planning (if available)
    pub statistics: Option<TableStatistics>,
}

impl TableInfo {
    /// Create a new TableInfo with minimal required fields.
    pub fn new(name: impl Into<String>, schema: Arc<Schema>) -> Self {
        Self {
            name: name.into(),
            schema,
            partitioning: None,
            properties: HashMap::new(),
            statistics: None,
        }
    }

    /// Set partitioning columns.
    pub fn with_partitioning(mut self, columns: Vec<String>) -> Self {
        self.partitioning = Some(columns);
        self
    }

    /// Add a property.
    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Set statistics.
    pub fn with_statistics(mut self, stats: TableStatistics) -> Self {
        self.statistics = Some(stats);
        self
    }

    /// Get the number of columns in the schema.
    pub fn num_columns(&self) -> usize {
        self.schema.fields().len()
    }

    /// Get column names.
    pub fn column_names(&self) -> Vec<&str> {
        self.schema
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect()
    }

    /// Check if a column exists.
    pub fn has_column(&self, name: &str) -> bool {
        self.schema.field_with_name(name).is_ok()
    }

    /// Check if the table is partitioned.
    pub fn is_partitioned(&self) -> bool {
        self.partitioning.as_ref().is_some_and(|p| !p.is_empty())
    }

    /// Get a property value.
    pub fn get_property(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(|s| s.as_str())
    }

    /// Get the estimated row count (from statistics).
    pub fn estimated_row_count(&self) -> Option<u64> {
        self.statistics.as_ref().map(|s| s.row_count)
    }

    /// Get the estimated size in bytes (from statistics).
    pub fn estimated_size_bytes(&self) -> Option<u64> {
        self.statistics.as_ref().map(|s| s.size_bytes)
    }
}

/// Table-level statistics for query planning.
///
/// Provides aggregate statistics about the entire table including
/// row count, size, and per-column statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TableStatistics {
    /// Total number of rows in the table
    pub row_count: u64,

    /// Total size of the table in bytes
    pub size_bytes: u64,

    /// Per-column statistics
    #[allow(dead_code)]
    pub column_stats: HashMap<String, ColumnStatistics>,

    /// Number of files comprising the table
    pub file_count: Option<u64>,

    /// Average file size in bytes
    pub avg_file_size: Option<u64>,

    /// Number of partitions (for partitioned tables)
    pub partition_count: Option<u64>,

    /// Last modified timestamp (Unix epoch millis)
    pub last_modified: Option<i64>,

    /// Whether statistics are exact or estimated
    pub is_exact: bool,
}

impl TableStatistics {
    /// Create new statistics with row count and size.
    pub fn new(row_count: u64, size_bytes: u64) -> Self {
        Self {
            row_count,
            size_bytes,
            is_exact: true,
            ..Default::default()
        }
    }

    /// Add column statistics.
    pub fn with_column_stats(mut self, column: impl Into<String>, stats: ColumnStatistics) -> Self {
        self.column_stats.insert(column.into(), stats);
        self
    }

    /// Set file count.
    pub fn with_file_count(mut self, count: u64) -> Self {
        self.file_count = Some(count);
        self
    }

    /// Set partition count.
    pub fn with_partition_count(mut self, count: u64) -> Self {
        self.partition_count = Some(count);
        self
    }

    /// Set last modified time.
    pub fn with_last_modified(mut self, timestamp: i64) -> Self {
        self.last_modified = Some(timestamp);
        self
    }

    /// Mark statistics as estimated (not exact).
    pub fn as_estimated(mut self) -> Self {
        self.is_exact = false;
        self
    }

    /// Get statistics for a specific column.
    pub fn get_column_stats(&self, column: &str) -> Option<&ColumnStatistics> {
        self.column_stats.get(column)
    }

    /// Estimate the average row size in bytes.
    pub fn avg_row_size(&self) -> f64 {
        if self.row_count > 0 {
            self.size_bytes as f64 / self.row_count as f64
        } else {
            0.0
        }
    }

    /// Merge with another statistics object.
    pub fn merge(&mut self, other: &TableStatistics) {
        self.row_count += other.row_count;
        self.size_bytes += other.size_bytes;

        if let Some(count) = other.file_count {
            *self.file_count.get_or_insert(0) += count;
        }

        // Merge column stats (take max of distinct counts, etc.)
        for (col, stats) in &other.column_stats {
            if let Some(existing) = self.column_stats.get_mut(col) {
                existing.merge(stats);
            } else {
                self.column_stats.insert(col.clone(), stats.clone());
            }
        }

        // If either is estimated, result is estimated
        if !other.is_exact {
            self.is_exact = false;
        }
    }
}

/// Per-column statistics for selectivity estimation.
///
/// Provides detailed statistics about a single column including
/// value distribution, null counts, and optional histograms.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ColumnStatistics {
    /// Number of null values
    pub null_count: u64,

    /// Number of distinct values (approximate)
    pub distinct_count: Option<u64>,

    /// Minimum value (as string for flexibility)
    pub min_value: Option<String>,

    /// Maximum value (as string for flexibility)
    pub max_value: Option<String>,

    /// Average length for string/binary columns
    pub avg_length: Option<f64>,

    /// Total bytes for this column
    pub size_bytes: Option<u64>,

    /// Histogram for value distribution
    pub histogram: Option<Histogram>,

    /// Whether column is sorted
    pub is_sorted: bool,

    /// Whether column has only ascending values
    pub is_ascending: Option<bool>,

    /// Top-K frequent values
    pub top_k_values: Option<Vec<(String, u64)>>,
}

impl ColumnStatistics {
    /// Create new column statistics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the null count.
    pub fn with_null_count(mut self, count: u64) -> Self {
        self.null_count = count;
        self
    }

    /// Set the distinct count.
    pub fn with_distinct_count(mut self, count: u64) -> Self {
        self.distinct_count = Some(count);
        self
    }

    /// Set min/max values.
    pub fn with_min_max(mut self, min: impl Into<String>, max: impl Into<String>) -> Self {
        self.min_value = Some(min.into());
        self.max_value = Some(max.into());
        self
    }

    /// Set the histogram.
    pub fn with_histogram(mut self, histogram: Histogram) -> Self {
        self.histogram = Some(histogram);
        self
    }

    /// Mark column as sorted.
    pub fn sorted(mut self, ascending: bool) -> Self {
        self.is_sorted = true;
        self.is_ascending = Some(ascending);
        self
    }

    /// Set top-K frequent values.
    pub fn with_top_k(mut self, values: Vec<(String, u64)>) -> Self {
        self.top_k_values = Some(values);
        self
    }

    /// Calculate the null ratio.
    pub fn null_ratio(&self, total_rows: u64) -> f64 {
        if total_rows > 0 {
            self.null_count as f64 / total_rows as f64
        } else {
            0.0
        }
    }

    /// Estimate selectivity for an equality predicate.
    pub fn selectivity_eq(&self, total_rows: u64) -> f64 {
        if let Some(distinct) = self.distinct_count {
            if distinct > 0 {
                return 1.0 / distinct as f64;
            }
        }
        // Default assumption: 10% selectivity
        0.1_f64.min(1.0 / (total_rows as f64).sqrt())
    }

    /// Estimate selectivity for a range predicate.
    pub fn selectivity_range(&self, _low: &str, _high: &str) -> f64 {
        // TODO: Use histogram if available
        // Default assumption: 25% selectivity
        0.25
    }

    /// Merge with another column statistics object.
    pub fn merge(&mut self, other: &ColumnStatistics) {
        self.null_count += other.null_count;

        // Take max of distinct counts
        if let Some(other_distinct) = other.distinct_count {
            self.distinct_count = Some(
                self.distinct_count
                    .map_or(other_distinct, |d| d.max(other_distinct)),
            );
        }

        // Extend min/max range
        if let Some(ref other_min) = other.min_value {
            if self.min_value.is_none() || self.min_value.as_ref().is_some_and(|m| other_min < m) {
                self.min_value = Some(other_min.clone());
            }
        }
        if let Some(ref other_max) = other.max_value {
            if self.max_value.is_none() || self.max_value.as_ref().is_some_and(|m| other_max > m) {
                self.max_value = Some(other_max.clone());
            }
        }

        // Sorted property is lost after merge
        self.is_sorted = false;
    }
}

/// Histogram for value distribution.
///
/// Used for accurate selectivity estimation in range queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Histogram {
    /// Histogram type
    pub histogram_type: HistogramType,

    /// Bucket boundaries (N+1 values for N buckets)
    pub boundaries: Vec<String>,

    /// Count of values in each bucket (N values for N buckets)
    pub counts: Vec<u64>,

    /// Number of distinct values in each bucket (optional)
    pub distinct_counts: Option<Vec<u64>>,
}

impl Histogram {
    /// Create an equi-width histogram.
    pub fn equi_width(boundaries: Vec<String>, counts: Vec<u64>) -> Self {
        Self {
            histogram_type: HistogramType::EquiWidth,
            boundaries,
            counts,
            distinct_counts: None,
        }
    }

    /// Create an equi-depth histogram.
    pub fn equi_depth(boundaries: Vec<String>, counts: Vec<u64>) -> Self {
        Self {
            histogram_type: HistogramType::EquiDepth,
            boundaries,
            counts,
            distinct_counts: None,
        }
    }

    /// Get the number of buckets.
    pub fn num_buckets(&self) -> usize {
        self.counts.len()
    }

    /// Get total count across all buckets.
    pub fn total_count(&self) -> u64 {
        self.counts.iter().sum()
    }

    /// Estimate selectivity for a range [low, high].
    pub fn selectivity_range(&self, low: &str, high: &str) -> f64 {
        let total = self.total_count();
        if total == 0 {
            return 0.0;
        }

        let mut matching = 0u64;
        for (i, count) in self.counts.iter().enumerate() {
            let bucket_low = &self.boundaries[i];
            let bucket_high = &self.boundaries[i + 1];

            // Check if bucket overlaps with range
            if bucket_high.as_str() >= low && bucket_low.as_str() <= high {
                // Simplified: count entire bucket if any overlap
                matching += count;
            }
        }

        matching as f64 / total as f64
    }
}

/// Histogram type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistogramType {
    /// Buckets have equal width
    EquiWidth,
    /// Buckets have equal depth (number of values)
    EquiDepth,
    /// Singleton histogram (one bucket per distinct value)
    Singleton,
}

/// Statistics for data readers.
///
/// Provides runtime statistics about a read operation including
/// progress and resource usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Statistics {
    /// Number of rows read
    pub rows_read: u64,

    /// Number of bytes read
    pub bytes_read: u64,

    /// Number of batches produced
    pub batches_produced: u64,

    /// Elapsed time in microseconds
    pub elapsed_us: u64,

    /// Number of rows filtered out
    pub rows_filtered: u64,

    /// Number of partitions scanned
    pub partitions_scanned: u64,

    /// Number of partitions pruned
    pub partitions_pruned: u64,

    /// Cache hit count
    pub cache_hits: u64,

    /// Cache miss count
    pub cache_misses: u64,

    /// Whether all data has been read
    pub is_complete: bool,
}

impl Statistics {
    /// Create new statistics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate throughput in rows per second.
    pub fn rows_per_second(&self) -> f64 {
        if self.elapsed_us > 0 {
            self.rows_read as f64 / (self.elapsed_us as f64 / 1_000_000.0)
        } else {
            0.0
        }
    }

    /// Calculate throughput in bytes per second.
    pub fn bytes_per_second(&self) -> f64 {
        if self.elapsed_us > 0 {
            self.bytes_read as f64 / (self.elapsed_us as f64 / 1_000_000.0)
        } else {
            0.0
        }
    }

    /// Calculate filter efficiency (0.0 to 1.0).
    pub fn filter_efficiency(&self) -> f64 {
        let total = self.rows_read + self.rows_filtered;
        if total > 0 {
            self.rows_filtered as f64 / total as f64
        } else {
            0.0
        }
    }

    /// Calculate partition pruning efficiency (0.0 to 1.0).
    pub fn partition_pruning_efficiency(&self) -> f64 {
        let total = self.partitions_scanned + self.partitions_pruned;
        if total > 0 {
            self.partitions_pruned as f64 / total as f64
        } else {
            0.0
        }
    }

    /// Calculate cache hit ratio (0.0 to 1.0).
    pub fn cache_hit_ratio(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total > 0 {
            self.cache_hits as f64 / total as f64
        } else {
            0.0
        }
    }
}

/// Result of a completed write operation.
///
/// Returned by `DataWriter::commit()` to provide details about
/// the committed data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WriteResult {
    /// Number of rows written
    pub rows_written: u64,

    /// Total bytes written
    pub bytes_written: u64,

    /// Files created during the write
    pub files_created: Vec<String>,

    /// Partitions written to
    pub partitions_written: Option<Vec<String>>,

    /// Write latency in microseconds
    pub latency_us: Option<u64>,

    /// Transaction ID (if applicable)
    pub transaction_id: Option<String>,

    /// Commit timestamp
    pub commit_timestamp: Option<i64>,

    /// Version after write (for versioned tables)
    pub version: Option<u64>,
}

impl WriteResult {
    /// Create a new write result.
    pub fn new(rows_written: u64, bytes_written: u64) -> Self {
        Self {
            rows_written,
            bytes_written,
            ..Default::default()
        }
    }

    /// Add a created file.
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.files_created.push(file.into());
        self
    }

    /// Set the files created.
    pub fn with_files(mut self, files: Vec<String>) -> Self {
        self.files_created = files;
        self
    }

    /// Set the partitions written.
    pub fn with_partitions(mut self, partitions: Vec<String>) -> Self {
        self.partitions_written = Some(partitions);
        self
    }

    /// Set the latency.
    pub fn with_latency(mut self, latency_us: u64) -> Self {
        self.latency_us = Some(latency_us);
        self
    }

    /// Set the transaction ID.
    pub fn with_transaction_id(mut self, tx_id: impl Into<String>) -> Self {
        self.transaction_id = Some(tx_id.into());
        self
    }

    /// Set the version.
    pub fn with_version(mut self, version: u64) -> Self {
        self.version = Some(version);
        self
    }

    /// Calculate throughput in rows per second.
    pub fn rows_per_second(&self) -> Option<f64> {
        self.latency_us.map(|us| {
            if us > 0 {
                self.rows_written as f64 / (us as f64 / 1_000_000.0)
            } else {
                0.0
            }
        })
    }

    /// Calculate throughput in MB per second.
    pub fn mb_per_second(&self) -> Option<f64> {
        self.latency_us.map(|us| {
            if us > 0 {
                (self.bytes_written as f64 / 1_048_576.0) / (us as f64 / 1_000_000.0)
            } else {
                0.0
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field};

    #[test]
    fn test_table_info_creation() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("value", DataType::Float64, true),
        ]));

        let info = TableInfo::new("test_table", schema)
            .with_partitioning(vec!["date".to_string()])
            .with_property("format", "parquet");

        assert_eq!(info.name, "test_table");
        assert_eq!(info.num_columns(), 2);
        assert!(info.has_column("id"));
        assert!(!info.has_column("nonexistent"));
        assert!(info.is_partitioned());
        assert_eq!(info.get_property("format"), Some("parquet"));
    }

    #[test]
    fn test_table_statistics() {
        let stats = TableStatistics::new(10000, 1_048_576)
            .with_file_count(10)
            .with_column_stats(
                "id",
                ColumnStatistics::new()
                    .with_distinct_count(10000)
                    .with_min_max("0001", "9999"),
            );

        assert_eq!(stats.row_count, 10000);
        assert_eq!(stats.size_bytes, 1_048_576);
        assert_eq!(stats.file_count, Some(10));
        assert!(stats.is_exact);

        let avg_row_size = stats.avg_row_size();
        assert!((avg_row_size - 104.8576).abs() < 0.001);
    }

    #[test]
    fn test_column_statistics_selectivity() {
        let col_stats = ColumnStatistics::new()
            .with_distinct_count(100)
            .with_null_count(10);

        // Equality selectivity should be ~1%
        let selectivity = col_stats.selectivity_eq(1000);
        assert!((selectivity - 0.01).abs() < 0.001);

        // Null ratio should be 1%
        let null_ratio = col_stats.null_ratio(1000);
        assert!((null_ratio - 0.01).abs() < 0.001);
    }

    #[test]
    fn test_histogram() {
        // Use zero-padded strings for proper string comparison
        let histogram = Histogram::equi_width(
            vec![
                "000".to_string(),
                "025".to_string(),
                "050".to_string(),
                "075".to_string(),
                "100".to_string(),
            ],
            vec![100, 200, 300, 400],
        );

        assert_eq!(histogram.num_buckets(), 4);
        assert_eq!(histogram.total_count(), 1000);

        // Range covering buckets 2, 3, and 4 with zero-padded values
        let selectivity = histogram.selectivity_range("040", "090");
        // Buckets [050,075] and [075,100] overlap → 300 + 400 = 700 out of 1000 = 0.7
        assert!(selectivity >= 0.5, "selectivity was {}", selectivity);
    }

    #[test]
    fn test_statistics_merge() {
        let mut stats1 = TableStatistics::new(1000, 10000).with_column_stats(
            "id",
            ColumnStatistics::new()
                .with_distinct_count(1000)
                .with_min_max("0001", "5000"),
        );

        let stats2 = TableStatistics::new(1000, 10000)
            .with_column_stats(
                "id",
                ColumnStatistics::new()
                    .with_distinct_count(500)
                    .with_min_max("5001", "9999"),
            )
            .as_estimated();

        stats1.merge(&stats2);

        assert_eq!(stats1.row_count, 2000);
        assert_eq!(stats1.size_bytes, 20000);
        assert!(!stats1.is_exact);

        let id_stats = stats1.get_column_stats("id").expect("column stats");
        assert_eq!(id_stats.min_value, Some("0001".to_string()));
        assert_eq!(id_stats.max_value, Some("9999".to_string()));
    }

    #[test]
    fn test_write_result() {
        let result = WriteResult::new(1000, 102400)
            .with_files(vec!["part-00000.parquet".to_string()])
            .with_latency(1_000_000) // 1 second
            .with_version(42);

        assert_eq!(result.rows_written, 1000);
        assert_eq!(result.files_created.len(), 1);
        assert_eq!(result.version, Some(42));

        let rows_per_sec = result.rows_per_second().expect("latency set");
        assert!((rows_per_sec - 1000.0).abs() < 0.001);
    }

    #[test]
    fn test_read_statistics() {
        let stats = Statistics {
            rows_read: 10000,
            bytes_read: 1_048_576,
            elapsed_us: 1_000_000, // 1 second
            rows_filtered: 5000,
            partitions_scanned: 5,
            partitions_pruned: 15,
            cache_hits: 80,
            cache_misses: 20,
            ..Default::default()
        };

        assert!((stats.rows_per_second() - 10000.0).abs() < 0.001);
        assert!((stats.filter_efficiency() - 0.333).abs() < 0.01);
        assert!((stats.partition_pruning_efficiency() - 0.75).abs() < 0.001);
        assert!((stats.cache_hit_ratio() - 0.8).abs() < 0.001);
    }
}
