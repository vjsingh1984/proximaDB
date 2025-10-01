//! Schema Building for Parquet Writers
//!
//! This module provides functionality to build Arrow schemas for Parquet files,
//! including support for vector columns, quantization, and metadata.

use anyhow::Result;
use arrow::datatypes::{DataType, Field, Fields, Schema};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::{WriterProperties, WriterPropertiesBuilder};
use parquet::basic::{Compression, Encoding};
use std::sync::Arc;

use crate::proto::proximadb_v1::{FilterableColumnSpec, QuantizationConfig};
use crate::core::compression::CompressionAlgorithm;
use crate::storage::engines::core::formats::columnar::constants::*;
use super::writer_config::ParquetWriterConfig;

/// Schema builder for Parquet files
pub struct ParquetSchemaBuilder {
    dimension: usize,
    config: ParquetWriterConfig,
    filterable_columns: Option<Vec<FilterableColumnSpec>>,
}

impl ParquetSchemaBuilder {
    /// Create new schema builder
    pub fn new(dimension: usize, config: ParquetWriterConfig) -> Self {
        Self {
            dimension,
            config,
            filterable_columns: None,
        }
    }

    /// Set filterable columns
    pub fn with_filterable_columns(mut self, columns: Vec<FilterableColumnSpec>) -> Self {
        self.filterable_columns = Some(columns);
        self
    }

    /// Build the complete Arrow schema
    pub fn build_schema(&self) -> Result<Arc<Schema>> {
        let mut fields = Vec::new();

        // ID column (ALWAYS REQUIRED for customer APIs)
        fields.push(Field::new(FIELD_ID, DataType::Utf8, false));

        // Row group offset and row index (for ID-less storage)
        fields.push(Field::new(FIELD_ROW_GROUP_OFFSET, DataType::UInt32, false));
        fields.push(Field::new(FIELD_ROW_INDEX, DataType::UInt32, false));

        // Vector data (FP32 fixed-size list - more efficient since dimension is known)
        let vector_field = Field::new(
            FIELD_VECTOR_FP32,
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                self.dimension as i32,
            ),
            false,
        );
        fields.push(vector_field);

        // Quantized vectors based on configuration
        if self.config.quantization.enable_binary {
            fields.push(Field::new(
                FIELD_Q_BINARY,
                DataType::Binary,
                true,
            ));
        }

        if self.config.quantization.enable_int8 {
            fields.push(Field::new(
                FIELD_Q_INT8,
                DataType::Binary,
                true,
            ));
            fields.push(Field::new(
                FIELD_QP_INT8_SCALE,
                DataType::Float32,
                true,
            ));
            fields.push(Field::new(
                FIELD_QP_INT8_MIN,
                DataType::Float32,
                true,
            ));
            fields.push(Field::new(
                FIELD_QP_INT8_MAX,
                DataType::Float32,
                true,
            ));
        }

        if self.config.quantization.enable_pq {
            // Determine PQ field based on configured bits
            let pq_bits = if self.config.quantization.pq_bits > 0 {
                self.config.quantization.pq_bits
            } else {
                8 // Default to PQ8 if not specified
            };
            let pq_field_name = match pq_bits {
                4 => FIELD_Q_PQ4,
                8 => FIELD_Q_PQ8,
                16 => FIELD_Q_PQ16,
                32 => FIELD_Q_PQ32,
                _ => FIELD_Q_PQ8, // Default to PQ8
            };

            fields.push(Field::new(
                pq_field_name,
                DataType::Binary,
                true,
            ));
            // Note: PQ codebook is stored as file-level metadata or sidecar, not per-row
        }

        // Temporal fields - using constants
        fields.push(Field::new(FIELD_TIMESTAMP, DataType::Int64, false));
        fields.push(Field::new(FIELD_UPDATED_AT, DataType::Int64, true)); // optional
        fields.push(Field::new(FIELD_EXPIRES_AT, DataType::Int64, true)); // optional
        fields.push(Field::new(FIELD_VERSION, DataType::UInt32, true)); // optional
        fields.push(Field::new(FIELD_SOURCE, DataType::Utf8, true)); // optional

        // Filterable metadata columns (if specified)
        if let Some(ref columns) = self.filterable_columns {
            for col_spec in columns {
                let data_type_str = match col_spec.data_type() {
                    crate::proto::proximadb_v1::FilterableDataType::FilterableString => "STRING",
                    crate::proto::proximadb_v1::FilterableDataType::FilterableInteger => "INTEGER",
                    crate::proto::proximadb_v1::FilterableDataType::FilterableFloat => "FLOAT",
                    crate::proto::proximadb_v1::FilterableDataType::FilterableBoolean => "BOOLEAN",
                    crate::proto::proximadb_v1::FilterableDataType::FilterableDatetime => "TIMESTAMP",
                    _ => "STRING", // Default to string for arrays and unknown types
                };
                let data_type = Self::sql_type_to_arrow_type(data_type_str);
                fields.push(Field::new(
                    &col_spec.name,
                    data_type,
                    true, // nullable
                ));
            }
        }

        // Extra metadata (for non-filterable metadata)
        let map_field = Field::new(
            FIELD_EXTRA_META,
            DataType::Map(
                Arc::new(Field::new(
                    "entries",
                    DataType::Struct(Fields::from(vec![
                        Field::new("key", DataType::Utf8, false),
                        Field::new("value", DataType::Utf8, true),
                    ])),
                    false,
                )),
                false,
            ),
            true,
        );
        fields.push(map_field);

        Ok(Arc::new(Schema::new(fields)))
    }

    /// Convert SQL type string to Arrow DataType
    fn sql_type_to_arrow_type(sql_type: &str) -> DataType {
        match sql_type.to_uppercase().as_str() {
            "INT" | "INTEGER" => DataType::Int32,
            "BIGINT" | "LONG" => DataType::Int64,
            "FLOAT" => DataType::Float32,
            "DOUBLE" => DataType::Float64,
            "BOOLEAN" | "BOOL" => DataType::Boolean,
            "TEXT" | "STRING" | "VARCHAR" => DataType::Utf8,
            "TIMESTAMP" => DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, None),
            "DATE" => DataType::Date32,
            "BINARY" | "BLOB" => DataType::Binary,
            _ => DataType::Utf8, // Default to string
        }
    }
}

/// Create writer properties with comprehensive optimizations
pub fn create_writer_properties(config: &ParquetWriterConfig) -> Result<WriterProperties> {
    let mut builder = WriterProperties::builder()
        .set_max_row_group_size(config.row_group_size)
        .set_data_page_size_limit(config.page_size)
        .set_write_batch_size(config.write_batch_size);

    // Set compression
    let compression = match config.compression {
        Compression::UNCOMPRESSED => parquet::basic::Compression::UNCOMPRESSED,
        Compression::SNAPPY => parquet::basic::Compression::SNAPPY,
        Compression::GZIP(_) => parquet::basic::Compression::GZIP(parquet::basic::GzipLevel::default()),
        Compression::LZ4 => parquet::basic::Compression::LZ4,
        Compression::LZ4_RAW => parquet::basic::Compression::LZ4_RAW,
        Compression::BROTLI(_) => parquet::basic::Compression::BROTLI(parquet::basic::BrotliLevel::default()),
        Compression::ZSTD(_) => parquet::basic::Compression::ZSTD(parquet::basic::ZstdLevel::default()),
        Compression::LZO => parquet::basic::Compression::LZO,
    };
    builder = builder.set_compression(compression);

    // Enable dictionary encoding if configured
    if config.enable_dictionary {
        // Dictionary encoding is enabled separately, the encoding here is the fallback
        // Use PLAIN as the fallback encoding when dictionary encoding fails
        builder = builder.set_encoding(Encoding::PLAIN);
        builder = builder.set_dictionary_enabled(true);
    }

    // Enable statistics for column pruning
    if config.enable_statistics {
        builder = builder.set_statistics_enabled(parquet::file::properties::EnabledStatistics::Chunk);
    }

    // Enable bloom filters for ID column
    if config.enable_bloom_filters {
        builder = builder.set_bloom_filter_enabled(true);
        builder = builder.set_bloom_filter_fpp(config.bloom_filter_fpp);
        builder = builder.set_bloom_filter_ndv(config.bloom_filter_ndv);
    }

    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_builder_basic() {
        let config = ParquetWriterConfig::default();
        let builder = ParquetSchemaBuilder::new(128, config);
        let schema = builder.build_schema().unwrap();

        // Check required fields
        assert!(schema.field_with_name(FIELD_ID).is_ok());
        assert!(schema.field_with_name(FIELD_VECTOR_FP32).is_ok());
        assert!(schema.field_with_name("timestamp").is_ok());
        assert!(schema.field_with_name("extra_meta").is_ok());
    }

    #[test]
    fn test_schema_with_quantization() {
        use crate::storage::engines::core::formats::columnar::constants::*;

        let mut config = ParquetWriterConfig::default();
        config.quantization.enable_binary = true;
        config.quantization.enable_int8 = true;
        config.quantization.enable_pq = true;
        config.quantization.pq_bits = 8; // Use PQ8

        let builder = ParquetSchemaBuilder::new(256, config);
        let schema = builder.build_schema().unwrap();

        // Check quantization fields using constants
        assert!(schema.field_with_name(FIELD_Q_BINARY).is_ok());
        assert!(schema.field_with_name(FIELD_Q_INT8).is_ok());
        assert!(schema.field_with_name(FIELD_QP_INT8_SCALE).is_ok());
        assert!(schema.field_with_name(FIELD_QP_INT8_MIN).is_ok());
        assert!(schema.field_with_name(FIELD_QP_INT8_MAX).is_ok());
        assert!(schema.field_with_name(FIELD_Q_PQ8).is_ok());
        // Note: pq_codebook stored as file metadata, not column
    }

    #[test]
    fn test_schema_with_filterable_columns() {
        let config = ParquetWriterConfig::default();
        let columns = vec![
            FilterableColumnSpec {
                name: "category".to_string(),
                data_type: 0, // STRING type
                indexed: false,
                supports_range: false,
                estimated_cardinality: Some(100),
            },
            FilterableColumnSpec {
                name: "score".to_string(),
                data_type: 1, // FLOAT type
                indexed: false,
                supports_range: true,
                estimated_cardinality: Some(50),
            },
        ];

        let builder = ParquetSchemaBuilder::new(64, config)
            .with_filterable_columns(columns);
        let schema = builder.build_schema().unwrap();

        // Check filterable columns
        assert!(schema.field_with_name("category").is_ok());
        assert!(schema.field_with_name("score").is_ok());
    }

    #[test]
    fn test_writer_properties() {
        let mut config = ParquetWriterConfig::default();
        config.compression = Compression::ZSTD(parquet::basic::ZstdLevel::default());
        config.enable_dictionary = true;
        config.enable_statistics = true;
        config.enable_bloom_filters = true;

        let props = create_writer_properties(&config).unwrap();
        // Properties are opaque, but we can verify creation doesn't panic
        assert_eq!(props.max_row_group_size(), config.row_group_size);
    }
}