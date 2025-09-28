//! Column name constants for columnar storage formats
//!
//! This module defines all column names used in Parquet and other columnar formats
//! to ensure consistency across the codebase.

// === Core Identity Columns ===
/// Unique identifier column for vectors
pub const FIELD_ID: &str = "id";

/// Collection identifier column
pub const FIELD_COLLECTION_ID: &str = "collection_id";

// === Vector Data Columns ===
/// Full precision vector data (FP32)
pub const FIELD_VECTOR_FP32: &str = "vector";

/// Binary quantized vector data
pub const FIELD_VECTOR_BINARY: &str = "vector_binary";

/// INT8 quantized vector data
pub const FIELD_VECTOR_INT8: &str = "vector_int8";

/// Product quantization vector data
pub const FIELD_VECTOR_PQ: &str = "vector_pq";

// === Quantization Metadata ===
/// INT8 quantization scales
pub const FIELD_INT8_SCALES: &str = "int8_scales";

/// INT8 quantization zero points
pub const FIELD_INT8_ZERO_POINTS: &str = "int8_zero_points";

/// INT8 quantization scale (legacy single value)
pub const FIELD_INT8_SCALE: &str = "int8_scale";

/// INT8 quantization zero point (legacy single value)
pub const FIELD_INT8_ZERO_POINT: &str = "int8_zero_point";

/// PQ codebook data
pub const FIELD_PQ_CODEBOOK: &str = "pq_codebook";

// === Row Group Management ===
/// Row group offset for ID-less storage
pub const FIELD_ROW_GROUP_OFFSET: &str = "row_group_offset";

/// Row index within row group
pub const FIELD_ROW_INDEX: &str = "row_index";

// === Temporal Columns ===
/// Creation timestamp
pub const FIELD_TIMESTAMP: &str = "timestamp";

/// Update timestamp
pub const FIELD_UPDATED_AT: &str = "updated_at";

/// Expiration timestamp
pub const FIELD_EXPIRES_AT: &str = "expires_at";

/// Version number for optimistic concurrency
pub const FIELD_VERSION: &str = "version";

// === Metadata Columns ===
/// Extra metadata as Map type
pub const FIELD_EXTRA_META: &str = "extra_meta";

/// Source identifier
pub const FIELD_SOURCE: &str = "source";

/// Soft delete flag
pub const FIELD_IS_DELETED: &str = "is_deleted";

// === Schema Information ===
/// Schema version for forward compatibility
pub const FIELD_SCHEMA_VERSION: &str = "schema_version";

// === File Format Extensions ===
/// Parquet file extension
pub const PARQUET_EXTENSION: &str = ".parquet";

/// VIPER-specific file extension
pub const VIPER_FILE_EXTENSION: &str = ".viper.parquet";

// === Default Values ===
/// Default row group size
pub const DEFAULT_ROW_GROUP_SIZE: usize = 10000;

/// Default page size
pub const DEFAULT_PAGE_SIZE: usize = 1024;

/// Default write batch size
pub const DEFAULT_WRITE_BATCH_SIZE: usize = 1000;

// === Column Groups ===
/// All quantization-related columns
pub const QUANTIZATION_COLUMNS: &[&str] = &[
    FIELD_VECTOR_BINARY,
    FIELD_VECTOR_INT8,
    FIELD_VECTOR_PQ,
    FIELD_INT8_SCALES,
    FIELD_INT8_ZERO_POINTS,
    FIELD_PQ_CODEBOOK,
];

/// All temporal columns
pub const TEMPORAL_COLUMNS: &[&str] = &[
    FIELD_TIMESTAMP,
    FIELD_UPDATED_AT,
    FIELD_EXPIRES_AT,
];

/// Core required columns for vector storage
pub const REQUIRED_COLUMNS: &[&str] = &[
    FIELD_ID,
    FIELD_VECTOR_FP32,
    FIELD_TIMESTAMP,
];