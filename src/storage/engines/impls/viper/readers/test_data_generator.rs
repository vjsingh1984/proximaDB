//! Test Data Generator for Parquet Reader Tests
//!
//! Generates various types of Parquet files for testing different reader scenarios

use anyhow::Result;
use arrow_array::{
    Array, BooleanArray, FixedSizeListArray, Float32Array, Int64Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::fs::File;
use std::sync::Arc;
use tempfile::TempDir;

/// Test data generator for Parquet files
pub struct ParquetTestDataGenerator {
    temp_dir: TempDir,
    rng: ChaCha8Rng,
}

/// Configuration for test data generation
#[derive(Debug, Clone)]
pub struct TestDataConfig {
    pub num_rows: usize,
    pub vector_dim: usize,
    pub include_metadata: bool,
    pub include_quantized: bool,
    pub quantization_types: Vec<QuantizationType>,
    pub metadata_cardinality: usize,
    pub include_timestamps: bool,
    pub null_percentage: f32,
}

#[derive(Debug, Clone)]
pub enum QuantizationType {
    PQ4,
    PQ8,
    Binary,
}

/// Generated test file information
#[derive(Debug, Clone)]
pub struct TestFileInfo {
    pub file_path: String,
    pub schema: Arc<Schema>,
    pub num_rows: usize,
    pub vector_ids: Vec<String>,
    pub metadata_values: Vec<TestMetadata>,
    pub config: TestDataConfig,
}

#[derive(Debug, Clone)]
pub struct TestMetadata {
    pub category: String,
    pub year: i64,
    pub similarity: f32,
    pub tags: Vec<String>,
    pub active: bool,
}

impl Default for TestDataConfig {
    fn default() -> Self {
        Self {
            num_rows: 1000,
            vector_dim: 128,
            include_metadata: true,
            include_quantized: false,
            quantization_types: vec![QuantizationType::PQ8],
            metadata_cardinality: 10,
            include_timestamps: true,
            null_percentage: 0.1,
        }
    }
}

impl ParquetTestDataGenerator {
    /// Create new test data generator
    pub fn new() -> Result<Self> {
        let temp_dir = TempDir::new()?;
        let rng = ChaCha8Rng::seed_from_u64(42); // Fixed seed for reproducible tests

        Ok(Self { temp_dir, rng })
    }

    /// Generate a basic vector dataset
    pub fn generate_basic_vectors(&mut self, config: TestDataConfig) -> Result<TestFileInfo> {
        let schema = self.create_basic_schema(&config)?;
        let record_batch = self.create_basic_record_batch(&schema, &config)?;

        let file_path = self.temp_dir.path().join("basic_vectors.parquet");
        self.write_parquet_file(&file_path, &schema, vec![record_batch])?;

        let (vector_ids, metadata_values) = self.extract_test_metadata(&config);

        Ok(TestFileInfo {
            file_path: file_path.to_string_lossy().to_string(),
            schema,
            num_rows: config.num_rows,
            vector_ids,
            metadata_values,
            config,
        })
    }

    /// Generate dataset with quantized columns
    pub fn generate_quantized_vectors(&mut self, config: TestDataConfig) -> Result<TestFileInfo> {
        let mut config = config;
        config.include_quantized = true;

        let schema = self.create_quantized_schema(&config)?;
        let record_batch = self.create_quantized_record_batch(&schema, &config)?;

        let file_path = self.temp_dir.path().join("quantized_vectors.parquet");
        self.write_parquet_file(&file_path, &schema, vec![record_batch])?;

        let (vector_ids, metadata_values) = self.extract_test_metadata(&config);

        Ok(TestFileInfo {
            file_path: file_path.to_string_lossy().to_string(),
            schema,
            num_rows: config.num_rows,
            vector_ids,
            metadata_values,
            config,
        })
    }

    /// Generate dataset with rich metadata for filtering tests
    pub fn generate_filterable_vectors(&mut self, config: TestDataConfig) -> Result<TestFileInfo> {
        let mut config = config;
        config.include_metadata = true;
        config.metadata_cardinality = 20;

        let schema = self.create_filterable_schema(&config)?;
        let record_batch = self.create_filterable_record_batch(&schema, &config)?;

        let file_path = self.temp_dir.path().join("filterable_vectors.parquet");
        self.write_parquet_file(&file_path, &schema, vec![record_batch])?;

        let (vector_ids, metadata_values) = self.extract_test_metadata(&config);

        Ok(TestFileInfo {
            file_path: file_path.to_string_lossy().to_string(),
            schema,
            num_rows: config.num_rows,
            vector_ids,
            metadata_values,
            config,
        })
    }

    /// Generate large multi-row-group dataset
    pub fn generate_large_dataset(&mut self, config: TestDataConfig) -> Result<TestFileInfo> {
        let mut config = config;
        config.num_rows = 10000; // Ensure multiple row groups

        let schema = self.create_basic_schema(&config)?;

        // Create multiple record batches for row groups
        let mut record_batches = Vec::new();
        let batch_size = 2000;

        for _batch_idx in 0..(config.num_rows / batch_size) {
            let batch_config = TestDataConfig {
                num_rows: batch_size,
                ..config.clone()
            };

            let record_batch = self.create_basic_record_batch(&schema, &batch_config)?;
            record_batches.push(record_batch);
        }

        let file_path = self.temp_dir.path().join("large_vectors.parquet");
        self.write_parquet_file(&file_path, &schema, record_batches)?;

        let (vector_ids, metadata_values) = self.extract_test_metadata(&config);

        Ok(TestFileInfo {
            file_path: file_path.to_string_lossy().to_string(),
            schema,
            num_rows: config.num_rows,
            vector_ids,
            metadata_values,
            config,
        })
    }

    /// Generate empty dataset for edge case testing
    pub fn generate_empty_dataset(&mut self) -> Result<TestFileInfo> {
        let config = TestDataConfig {
            num_rows: 0,
            ..Default::default()
        };

        let schema = self.create_basic_schema(&config)?;
        let record_batch = self.create_empty_record_batch(&schema)?;

        let file_path = self.temp_dir.path().join("empty_vectors.parquet");
        self.write_parquet_file(&file_path, &schema, vec![record_batch])?;

        Ok(TestFileInfo {
            file_path: file_path.to_string_lossy().to_string(),
            schema,
            num_rows: 0,
            vector_ids: Vec::new(),
            metadata_values: Vec::new(),
            config,
        })
    }

    /// Create basic schema with ID, vector, and optional metadata
    fn create_basic_schema(&self, config: &TestDataConfig) -> Result<Arc<Schema>> {
        let mut fields = vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    config.vector_dim as i32,
                ),
                false,
            ),
        ];

        if config.include_metadata {
            fields.push(Field::new("metadata_info", DataType::Utf8, true));
        }

        if config.include_timestamps {
            fields.push(Field::new("timestamp", DataType::Int64, false));
            fields.push(Field::new("created_at", DataType::Int64, false));
            fields.push(Field::new("updated_at", DataType::Int64, false));
        }

        Ok(Arc::new(Schema::new(fields)))
    }

    /// Create schema with quantized columns
    fn create_quantized_schema(&self, config: &TestDataConfig) -> Result<Arc<Schema>> {
        let mut fields = vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    config.vector_dim as i32,
                ),
                false,
            ),
        ];

        // Add quantized columns
        for quant_type in &config.quantization_types {
            match quant_type {
                QuantizationType::PQ4 => {
                    fields.push(Field::new(
                        "vector_pq4",
                        DataType::FixedSizeList(
                            Arc::new(Field::new("item", DataType::UInt8, false)),
                            (config.vector_dim / 2) as i32, // PQ4 uses 4 bits per component
                        ),
                        false,
                    ));
                }
                QuantizationType::PQ8 => {
                    fields.push(Field::new(
                        "vector_pq8",
                        DataType::FixedSizeList(
                            Arc::new(Field::new("item", DataType::UInt8, false)),
                            config.vector_dim as i32, // PQ8 uses 8 bits per component
                        ),
                        false,
                    ));
                }
                QuantizationType::Binary => {
                    fields.push(Field::new(
                        "vector_binary",
                        DataType::FixedSizeList(
                            Arc::new(Field::new("item", DataType::UInt8, false)),
                            (config.vector_dim / 8) as i32, // Binary uses 1 bit per component
                        ),
                        false,
                    ));
                }
            }
        }

        if config.include_metadata {
            fields.push(Field::new("metadata_info", DataType::Utf8, true));
        }

        Ok(Arc::new(Schema::new(fields)))
    }

    /// Create schema optimized for metadata filtering
    fn create_filterable_schema(&self, config: &TestDataConfig) -> Result<Arc<Schema>> {
        let mut fields = vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    config.vector_dim as i32,
                ),
                false,
            ),
            // Separate filterable columns for efficient filtering
            Field::new("category", DataType::Utf8, true),
            Field::new("year", DataType::Int64, true),
            Field::new("score", DataType::Float32, true),
            Field::new("active", DataType::Boolean, true),
            Field::new(
                "tags",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
                true,
            ),
        ];

        if config.include_metadata {
            fields.push(Field::new("metadata_info", DataType::Utf8, true));
        }

        Ok(Arc::new(Schema::new(fields)))
    }

    /// Create basic record batch
    fn create_basic_record_batch(
        &mut self,
        schema: &Schema,
        config: &TestDataConfig,
    ) -> Result<RecordBatch> {
        let mut arrays: Vec<Arc<dyn Array>> = Vec::new();

        // ID column
        let ids: Vec<String> = (0..config.num_rows)
            .map(|i| format!("vec_{:06}", i))
            .collect();
        arrays.push(Arc::new(StringArray::from(ids)));

        // Vector column
        let vectors = self.generate_vectors(config.num_rows, config.vector_dim);
        arrays.push(Arc::new(vectors));

        // Metadata column (if included)
        if config.include_metadata {
            let metadata_json: Vec<Option<String>> = (0..config.num_rows)
                .map(|i| {
                    if self.rng.gen_range(0.0..1.0) < config.null_percentage {
                        None
                    } else {
                        Some(format!(
                            r#"{{"category":"cat_{}","year":{},"score":{:.2}}}"#,
                            i % config.metadata_cardinality,
                            2020 + (i % 5),
                            self.rng.gen_range(0.0..1.0)
                        ))
                    }
                })
                .collect();
            arrays.push(Arc::new(StringArray::from(metadata_json)));
        }

        // Timestamp columns (if included)
        if config.include_timestamps {
            let now = chrono::Utc::now().timestamp_millis();
            let timestamps: Vec<i64> = (0..config.num_rows)
                .map(|_| now + self.rng.gen_range(-86400000..86400000)) // ±1 day
                .collect();
            arrays.push(Arc::new(Int64Array::from(timestamps.clone())));
            arrays.push(Arc::new(Int64Array::from(timestamps.clone())));
            arrays.push(Arc::new(Int64Array::from(timestamps)));
        }

        RecordBatch::try_new(Arc::new(schema.clone()), arrays)
            .map_err(|e| anyhow::anyhow!("Failed to create record batch: {}", e))
    }

    /// Create record batch with quantized vectors
    fn create_quantized_record_batch(
        &mut self,
        schema: &Schema,
        config: &TestDataConfig,
    ) -> Result<RecordBatch> {
        let mut arrays: Vec<Arc<dyn Array>> = Vec::new();

        // ID column
        let ids: Vec<String> = (0..config.num_rows)
            .map(|i| format!("vec_{:06}", i))
            .collect();
        arrays.push(Arc::new(StringArray::from(ids)));

        // Original vector column
        let vectors = self.generate_vectors(config.num_rows, config.vector_dim);
        arrays.push(Arc::new(vectors));

        // Quantized columns
        for _quant_type in &config.quantization_types {
            // Skip quantized vectors for now - needs separate implementation
            // TODO: Generate quantized vectors properly
        }

        // Metadata column (if included)
        if config.include_metadata {
            let metadata_json: Vec<Option<String>> = (0..config.num_rows)
                .map(|i| {
                    Some(format!(
                        r#"{{"category":"cat_{}","quantized":true,"method":"{}"}}"#,
                        i % config.metadata_cardinality,
                        match &config.quantization_types[0] {
                            QuantizationType::PQ4 => "PQ4",
                            QuantizationType::PQ8 => "PQ8",
                            QuantizationType::Binary => "Binary",
                        }
                    ))
                })
                .collect();
            arrays.push(Arc::new(StringArray::from(metadata_json)));
        }

        RecordBatch::try_new(Arc::new(schema.clone()), arrays)
            .map_err(|e| anyhow::anyhow!("Failed to create quantized record batch: {}", e))
    }

    /// Create record batch optimized for filtering
    fn create_filterable_record_batch(
        &mut self,
        schema: &Schema,
        config: &TestDataConfig,
    ) -> Result<RecordBatch> {
        let mut arrays: Vec<Arc<dyn Array>> = Vec::new();

        // ID column
        let ids: Vec<String> = (0..config.num_rows)
            .map(|i| format!("vec_{:06}", i))
            .collect();
        arrays.push(Arc::new(StringArray::from(ids)));

        // Vector column
        let vectors = self.generate_vectors(config.num_rows, config.vector_dim);
        arrays.push(Arc::new(vectors));

        // Filterable columns
        let categories = vec!["technology", "science", "art", "music", "sports"];
        let category_values: Vec<Option<String>> = (0..config.num_rows)
            .map(|i| {
                if self.rng.gen_range(0.0..1.0) < config.null_percentage {
                    None
                } else {
                    Some(categories[i % categories.len()].to_string())
                }
            })
            .collect();
        arrays.push(Arc::new(StringArray::from(category_values)));

        let years: Vec<Option<i64>> = (0..config.num_rows)
            .map(|i| {
                if self.rng.gen_range(0.0..1.0) < config.null_percentage {
                    None
                } else {
                    Some(2020 + (i % 5) as i64)
                }
            })
            .collect();
        arrays.push(Arc::new(Int64Array::from(years)));

        let scores: Vec<Option<f32>> = (0..config.num_rows)
            .map(|_| {
                if self.rng.gen_range(0.0..1.0) < config.null_percentage {
                    None
                } else {
                    Some(self.rng.gen_range(0.0..1.0))
                }
            })
            .collect();
        arrays.push(Arc::new(Float32Array::from(scores)));

        let active_values: Vec<Option<bool>> = (0..config.num_rows)
            .map(|_| {
                if self.rng.gen_range(0.0..1.0) < config.null_percentage {
                    None
                } else {
                    Some(self.rng.gen_bool(0.7))
                }
            })
            .collect();
        arrays.push(Arc::new(BooleanArray::from(active_values)));

        // Tags column (list of strings)
        let all_tags = vec!["AI", "ML", "NLP", "CV", "robotics", "data", "analysis"];
        let mut tags_builder =
            arrow_array::builder::ListBuilder::new(arrow_array::builder::StringBuilder::new());

        for _ in 0..config.num_rows {
            if self.rng.gen_range(0.0..1.0) < config.null_percentage {
                tags_builder.append_null();
            } else {
                let num_tags = self.rng.gen_range(1..4);
                for _ in 0..num_tags {
                    let tag = all_tags[self.rng.gen_range(0..all_tags.len())];
                    tags_builder.values().append_value(tag);
                }
                tags_builder.append(true);
            }
        }
        arrays.push(Arc::new(tags_builder.finish()));

        // Metadata column (if included)
        if config.include_metadata {
            let metadata_json: Vec<Option<String>> = (0..config.num_rows)
                .map(|i| {
                    Some(format!(
                        r#"{{"filterable":true,"row_index":{},"generated_at":"{}"}}"#,
                        i,
                        chrono::Utc::now().to_rfc3339()
                    ))
                })
                .collect();
            arrays.push(Arc::new(StringArray::from(metadata_json)));
        }

        RecordBatch::try_new(Arc::new(schema.clone()), arrays)
            .map_err(|e| anyhow::anyhow!("Failed to create filterable record batch: {}", e))
    }

    /// Create empty record batch
    fn create_empty_record_batch(&self, schema: &Schema) -> Result<RecordBatch> {
        let mut arrays: Vec<Arc<dyn Array>> = Vec::new();

        for field in schema.fields() {
            let array = arrow_array::new_empty_array(field.data_type());
            arrays.push(array);
        }

        RecordBatch::try_new(Arc::new(schema.clone()), arrays)
            .map_err(|e| anyhow::anyhow!("Failed to create empty record batch: {}", e))
    }

    /// Generate random vectors
    fn generate_vectors(&mut self, num_vectors: usize, dim: usize) -> FixedSizeListArray {
        let mut vector_builder = arrow_array::builder::FixedSizeListBuilder::new(
            arrow_array::builder::Float32Builder::new(),
            dim as i32,
        );

        for _ in 0..num_vectors {
            for _ in 0..dim {
                vector_builder
                    .values()
                    .append_value(self.rng.gen_range(-1.0..1.0));
            }
            vector_builder.append(true);
        }

        vector_builder.finish()
    }

    /// Generate quantized vectors array
    fn generate_quantized_vectors_array(
        &mut self,
        num_vectors: usize,
        dim: usize,
        quant_type: &QuantizationType,
    ) -> FixedSizeListArray {
        let quantized_dim = match quant_type {
            QuantizationType::PQ4 => dim / 2,
            QuantizationType::PQ8 => dim,
            QuantizationType::Binary => dim / 8,
        };

        let mut vector_builder = arrow_array::builder::FixedSizeListBuilder::new(
            arrow_array::builder::UInt8Builder::new(),
            quantized_dim as i32,
        );

        for _ in 0..num_vectors {
            for _ in 0..quantized_dim {
                let value = match quant_type {
                    QuantizationType::PQ4 => self.rng.gen_range(0..16),
                    QuantizationType::PQ8 => self.rng.gen_range(0..256),
                    QuantizationType::Binary => self.rng.gen_range(0..256),
                };
                vector_builder.values().append_value(value as u8);
            }
            vector_builder.append(true);
        }

        vector_builder.finish()
    }

    /// Write Parquet file
    fn write_parquet_file(
        &self,
        file_path: &std::path::Path,
        schema: &Schema,
        record_batches: Vec<RecordBatch>,
    ) -> Result<()> {
        let file = File::create(file_path)?;
        let mut writer = ArrowWriter::try_new(file, Arc::new(schema.clone()), None)?;

        for batch in record_batches {
            writer.write(&batch)?;
        }

        writer.close()?;
        Ok(())
    }

    /// Extract test metadata for verification
    fn extract_test_metadata(&self, config: &TestDataConfig) -> (Vec<String>, Vec<TestMetadata>) {
        let vector_ids: Vec<String> = (0..config.num_rows)
            .map(|i| format!("vec_{:06}", i))
            .collect();

        let metadata_values: Vec<TestMetadata> = (0..config.num_rows)
            .map(|i| TestMetadata {
                category: format!("cat_{}", i % config.metadata_cardinality),
                year: 2020 + (i % 5) as i64,
                similarity: (i as f32) / (config.num_rows as f32),
                tags: vec!["tag1".to_string(), "tag2".to_string()],
                active: i % 2 == 0,
            })
            .collect();

        (vector_ids, metadata_values)
    }

    /// Get temp directory path
    pub fn temp_dir_path(&self) -> &std::path::Path {
        self.temp_dir.path()
    }
}
