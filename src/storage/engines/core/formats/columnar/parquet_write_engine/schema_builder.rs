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
        fields.push(Field::new("row_group_offset", DataType::UInt32, false));
        fields.push(Field::new("row_index", DataType::UInt32, false));

        // Vector data (FP32 list)
        let vector_field = Field::new(
            FIELD_VECTOR_FP32,
            DataType::List(Arc::new(Field::new("item", DataType::Float32, false))),
            false,
        );
        fields.push(vector_field);

        // Quantized vectors based on configuration
        if self.config.quantization.enable_binary {
            fields.push(Field::new(
                "vector_binary",
                DataType::Binary,
                true,
            ));
        }

        if self.config.quantization.enable_int8 {
            fields.push(Field::new(
                "vector_int8",
                DataType::Binary,
                true,
            ));
            fields.push(Field::new(
                "int8_scales",
                DataType::List(Arc::new(Field::new("item", DataType::Float32, false))),
                true,
            ));
            fields.push(Field::new(
                "int8_zero_points",
                DataType::List(Arc::new(Field::new("item", DataType::Float32, false))),
                true,
            ));
        }

        if self.config.quantization.enable_pq {
            fields.push(Field::new(
                "vector_pq",
                DataType::Binary,
                true,
            ));
            fields.push(Field::new(
                "pq_codebook",
                DataType::Binary,
                true,
            ));
        }

        // Metadata fields
        fields.push(Field::new("timestamp", DataType::Int64, false));

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
            "extra_meta",
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
        builder = builder.set_encoding(Encoding::RLE_DICTIONARY);
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
        let mut config = ParquetWriterConfig::default();
        config.quantization.enable_binary = true;
        config.quantization.enable_int8 = true;
        config.quantization.enable_pq = true;

        let builder = ParquetSchemaBuilder::new(256, config);
        let schema = builder.build_schema().unwrap();

        // Check quantization fields
        assert!(schema.field_with_name("vector_binary").is_ok());
        assert!(schema.field_with_name("vector_int8").is_ok());
        assert!(schema.field_with_name("int8_scales").is_ok());
        assert!(schema.field_with_name("vector_pq").is_ok());
        assert!(schema.field_with_name("pq_codebook").is_ok());
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