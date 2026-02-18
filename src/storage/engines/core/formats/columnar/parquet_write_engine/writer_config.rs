//! Parquet Writer Configuration
//!
//! This module defines configuration options for Parquet writers,
//! including compression, encoding, and optimization settings.

use crate::proto::proximadb_v1::QuantizationConfig;
use crate::storage::engines::core::formats::columnar::constants::{
    DEFAULT_PAGE_SIZE, DEFAULT_ROW_GROUP_SIZE,
};
use parquet::basic::Compression;

/// Comprehensive configuration for Parquet writers
#[derive(Debug, Clone)]
pub struct ParquetWriterConfig {
    // === Core Settings ===
    /// Row group size for Parquet files
    pub row_group_size: usize,

    /// Page size for fine-grained I/O
    pub page_size: usize,

    /// Write batch size for streaming writers
    pub write_batch_size: usize,

    // === Compression Settings ===
    /// Compression algorithm for data pages
    pub compression: Compression,

    /// Compression level (if applicable)
    pub compression_level: Option<i32>,

    /// Enable dictionary encoding for string columns
    pub enable_dictionary: bool,

    // === Bloom Filter Settings ===
    /// Enable bloom filters for ID columns
    pub enable_bloom_filters: bool,

    /// Bloom filter false positive probability
    pub bloom_filter_fpp: f64,

    /// Bloom filter NDV (number of distinct values)
    pub bloom_filter_ndv: u64,

    // === Optimization Settings ===
    /// Enable statistics for min/max pruning
    pub enable_statistics: bool,

    /// Enable page index for fine-grained pruning
    pub enable_page_index: bool,

    /// Sort columns for better compression
    pub sort_columns: Vec<String>,

    // === ID-less Storage Optimization ===
    /// Enable ID-less storage optimization
    /// Note: This optimization should typically be false to keep customer ID column
    pub id_less_storage: bool,

    // === Metadata Settings ===
    /// Columns that should be stored as dedicated columns (not in extra_meta)
    pub filterable_metadata_columns: Option<Vec<String>>,

    // === Quantization Settings ===
    /// Quantization configuration for vector compression
    pub quantization: QuantizationConfig,

    // === Advanced Settings ===
    /// Maximum number of records per file
    pub max_records_per_file: Option<usize>,

    /// Target file size in bytes
    pub target_file_size_bytes: Option<usize>,

    /// Enable async I/O for better performance
    pub enable_async_io: bool,
}

impl ParquetWriterConfig {
    /// Create a new configuration with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style method to set row group size
    pub fn with_row_group_size(mut self, size: usize) -> Self {
        self.row_group_size = size;
        self
    }

    /// Builder-style method to set compression
    pub fn with_compression(mut self, compression: Compression) -> Self {
        self.compression = compression;
        self
    }

    /// Builder-style method to enable bloom filters
    pub fn with_bloom_filters(mut self, enabled: bool) -> Self {
        self.enable_bloom_filters = enabled;
        self
    }

    /// Builder-style method to set filterable metadata columns
    pub fn with_filterable_columns(mut self, columns: Vec<String>) -> Self {
        self.filterable_metadata_columns = Some(columns);
        self
    }

    /// Builder-style method to set quantization config
    pub fn with_quantization(mut self, quantization: QuantizationConfig) -> Self {
        self.quantization = quantization;
        self
    }

    /// Create optimized config for analytics workloads
    pub fn for_analytics() -> Self {
        Self {
            row_group_size: 50000,
            compression: Compression::ZSTD(parquet::basic::ZstdLevel::try_new(3).unwrap()),
            enable_dictionary: true,
            enable_statistics: true,
            enable_page_index: true,
            ..Default::default()
        }
    }

    /// Create optimized config for real-time workloads
    pub fn for_realtime() -> Self {
        Self {
            row_group_size: 5000,
            compression: Compression::LZ4,
            enable_bloom_filters: true,
            enable_async_io: true,
            ..Default::default()
        }
    }

    /// Create optimized config for archival storage
    pub fn for_archival() -> Self {
        Self {
            row_group_size: 100000,
            compression: Compression::ZSTD(parquet::basic::ZstdLevel::try_new(9).unwrap()),
            enable_dictionary: true,
            enable_statistics: true,
            ..Default::default()
        }
    }

    /// Validate configuration for consistency
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.row_group_size == 0 {
            return Err(anyhow::anyhow!("Row group size must be greater than 0"));
        }

        if self.page_size == 0 {
            return Err(anyhow::anyhow!("Page size must be greater than 0"));
        }

        if self.page_size > self.row_group_size {
            return Err(anyhow::anyhow!(
                "Page size cannot be larger than row group size"
            ));
        }

        if self.bloom_filter_fpp <= 0.0 || self.bloom_filter_fpp >= 1.0 {
            return Err(anyhow::anyhow!(
                "Bloom filter false positive probability must be between 0 and 1"
            ));
        }

        Ok(())
    }
}

impl Default for ParquetWriterConfig {
    fn default() -> Self {
        Self {
            row_group_size: DEFAULT_ROW_GROUP_SIZE,
            page_size: DEFAULT_PAGE_SIZE,
            write_batch_size: 1000,
            compression: Compression::SNAPPY,
            compression_level: None,
            enable_dictionary: true,
            enable_bloom_filters: true, // Enable by default for better filtering
            bloom_filter_fpp: 0.05,
            bloom_filter_ndv: 1000000,
            enable_statistics: true,
            enable_page_index: true, // Enable by default for cloud-optimized access
            sort_columns: Vec::new(),
            id_less_storage: false,
            filterable_metadata_columns: None,
            quantization: QuantizationConfig::default(),
            max_records_per_file: None,
            target_file_size_bytes: None,
            enable_async_io: false,
        }
    }
}
