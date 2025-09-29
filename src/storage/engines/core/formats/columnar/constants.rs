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
/// Full precision vector data (FP32) - Primary column
pub const FIELD_VECTOR_FP32: &str = "vector_fp32";

/// Binary quantized vector data (1 bit per dimension)
pub const FIELD_Q_BINARY: &str = "q_binary";

/// INT8 quantized vector data (8 bits per dimension)
pub const FIELD_Q_INT8: &str = "q_int8";

/// PQ4 quantized vector data (4 bits per code)
pub const FIELD_Q_PQ4: &str = "q_pq4";

/// PQ8 quantized vector data (8 bits per code)
pub const FIELD_Q_PQ8: &str = "q_pq8";

/// PQ16 quantized vector data (16 bits per code)
pub const FIELD_Q_PQ16: &str = "q_pq16";

/// PQ32 quantized vector data (32 bits per code)
pub const FIELD_Q_PQ32: &str = "q_pq32";


// === Codebook Columns ===
/// PQ4 codebooks (subquantizer centroids for 4-bit codes)
pub const FIELD_CB_PQ4: &str = "cb_pq4";

/// PQ8 codebooks (subquantizer centroids for 8-bit codes)
pub const FIELD_CB_PQ8: &str = "cb_pq8";

/// PQ16 codebooks (subquantizer centroids for 16-bit codes)
pub const FIELD_CB_PQ16: &str = "cb_pq16";

/// PQ32 codebooks (subquantizer centroids for 32-bit codes)
pub const FIELD_CB_PQ32: &str = "cb_pq32";

// === Quantization Parameter Columns ===
/// Binary quantization threshold parameter
pub const FIELD_QP_BINARY_THRESHOLD: &str = "qp_binary_threshold";

/// INT8 quantization minimum value parameter
pub const FIELD_QP_INT8_MIN: &str = "qp_int8_min";

/// INT8 quantization maximum value parameter
pub const FIELD_QP_INT8_MAX: &str = "qp_int8_max";

/// INT8 quantization scale factor parameter
pub const FIELD_QP_INT8_SCALE: &str = "qp_int8_scale";

/// PQ number of subquantizers parameter
pub const FIELD_QP_PQ_SUBQUANTIZERS: &str = "qp_pq_subquantizers";

/// PQ number of centroids per subquantizer parameter
pub const FIELD_QP_PQ_CENTROIDS: &str = "qp_pq_centroids";


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
/// Quantized vector columns
pub const QUANTIZED_VECTOR_COLUMNS: &[&str] = &[
    FIELD_Q_BINARY,
    FIELD_Q_INT8,
    FIELD_Q_PQ4,
    FIELD_Q_PQ8,
    FIELD_Q_PQ16,
    FIELD_Q_PQ32,
];

/// Codebook columns
pub const CODEBOOK_COLUMNS: &[&str] = &[
    FIELD_CB_PQ4,
    FIELD_CB_PQ8,
    FIELD_CB_PQ16,
    FIELD_CB_PQ32,
];

/// Quantization parameter columns
pub const QUANTIZATION_PARAMETER_COLUMNS: &[&str] = &[
    FIELD_QP_BINARY_THRESHOLD,
    FIELD_QP_INT8_MIN,
    FIELD_QP_INT8_MAX,
    FIELD_QP_INT8_SCALE,
    FIELD_QP_PQ_SUBQUANTIZERS,
    FIELD_QP_PQ_CENTROIDS,
];

/// All quantization-related columns
pub const QUANTIZATION_COLUMNS: &[&str] = &[
    FIELD_Q_BINARY,
    FIELD_Q_INT8,
    FIELD_Q_PQ4,
    FIELD_Q_PQ8,
    FIELD_Q_PQ16,
    FIELD_Q_PQ32,
    FIELD_CB_PQ4,
    FIELD_CB_PQ8,
    FIELD_CB_PQ16,
    FIELD_CB_PQ32,
    FIELD_QP_BINARY_THRESHOLD,
    FIELD_QP_INT8_MIN,
    FIELD_QP_INT8_MAX,
    FIELD_QP_INT8_SCALE,
    FIELD_QP_PQ_SUBQUANTIZERS,
    FIELD_QP_PQ_CENTROIDS,
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