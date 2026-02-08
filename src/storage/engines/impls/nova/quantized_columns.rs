//! Quantized columns for NOVA - Progressive columnar storage with multi-level quantization
//!
//! This module delegates all quantization operations to the unified quantization module
//! in compute/quantization, eliminating duplication and ensuring consistency across engines.

use anyhow::{Result, anyhow};
use arrow::array::ArrayRef;
use arrow_array::RecordBatch;
use arrow_array::array::{BinaryArray, Float32Array, Int8Array};
use arrow_schema::{DataType, Field};
use std::sync::Arc;

// Use unified quantization from compute module
use crate::compute::quantization::unified::{Codebook, UnifiedQuantizationEngine};

/// Metadata for quantized columns in Parquet
#[derive(Debug, Clone, Default)]
pub struct QuantizedColumnMetadata {
    /// Binary column info
    pub binary_column: Option<BinaryColumnInfo>,

    /// INT8 column info  
    pub int8_column: Option<Int8ColumnInfo>,

    /// PQ column info
    pub pq_column: Option<PQColumnInfo>,

    /// Original vector dimension
    pub dimension: usize,

    /// Statistics
    pub stats: QuantizationStats,
}

#[derive(Debug, Clone, Default)]
pub struct BinaryColumnInfo {
    pub column_name: String,
    pub bits_per_vector: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Int8ColumnInfo {
    pub column_name: String,
    pub global_scale: f32,
    pub global_zero_point: i8,
}

#[derive(Debug, Clone, Default)]
pub struct PQColumnInfo {
    pub column_name: String,
    pub num_segments: usize,
    pub bits_per_code: u8,
    pub codebook_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct QuantizationStats {
    pub num_vectors: usize,
    pub compression_ratio: f32,
    pub avg_reconstruction_error: Option<f32>,
}

/// Container for all quantized columns - delegates to unified engine
pub struct QuantizedColumns {
    /// Reference to unified quantization engine
    quantization_engine: Arc<UnifiedQuantizationEngine>,

    /// Binary quantized data
    pub binary_data: Option<Vec<Vec<u8>>>,

    /// INT8 quantized data with metadata
    pub int8_data: Option<Int8QuantizedData>,

    /// PQ quantized data with metadata
    pub pq_data: Option<PQQuantizedData>,

    /// Metadata
    pub metadata: QuantizedColumnMetadata,
}

/// INT8 quantized data container
#[derive(Debug, Clone)]
pub struct Int8QuantizedData {
    pub vectors: Vec<Vec<u8>>,
    pub scales: Vec<f32>,
    pub zero_points: Vec<i8>,
}

/// PQ quantized data container
#[derive(Debug, Clone)]
pub struct PQQuantizedData {
    pub codes: Vec<Vec<u8>>,
    pub codebook_id: String,
    pub num_segments: usize,
    pub bits_per_code: u8,
}

impl QuantizedColumns {
    /// Create new quantized columns using unified engine
    pub fn new(quantization_engine: Arc<UnifiedQuantizationEngine>) -> Self {
        Self {
            quantization_engine,
            binary_data: None,
            int8_data: None,
            pq_data: None,
            metadata: QuantizedColumnMetadata::default(),
        }
    }

    /// Build binary quantized column using unified engine
    pub async fn build_binary_column(&mut self, vectors: &[Vec<f32>]) -> Result<()> {
        if vectors.is_empty() {
            return Ok(());
        }

        let mut binary_vectors = Vec::with_capacity(vectors.len());

        for vector in vectors {
            // Use unified engine's binary quantization
            let binary = self.quantization_engine.quantize_to_binary(vector)?;
            binary_vectors.push(binary);
        }

        self.binary_data = Some(binary_vectors);
        self.metadata.binary_column = Some(BinaryColumnInfo {
            column_name: "binary_sketch".to_string(),
            bits_per_vector: vectors[0].len(),
        });

        Ok(())
    }

    /// Build INT8 quantized column using unified engine
    pub async fn build_int8_column(&mut self, vectors: &[Vec<f32>]) -> Result<()> {
        if vectors.is_empty() {
            return Ok(());
        }

        let mut int8_vectors = Vec::with_capacity(vectors.len());
        let mut scales = Vec::with_capacity(vectors.len());
        let mut zero_points = Vec::with_capacity(vectors.len());

        for vector in vectors {
            // Use unified engine's INT8 quantization
            let (quantized, min_val, max_val) = self.quantization_engine.quantize_to_u8(vector)?;

            // Calculate scale and zero point from min/max
            let range = max_val - min_val;
            let scale = if range > 0.0 { range / 255.0 } else { 1.0 };
            let zero_point = (min_val / scale).round() as i8;

            int8_vectors.push(quantized);
            scales.push(scale);
            zero_points.push(zero_point);
        }

        // Store global scale/zero_point as first values
        let global_scale = scales.first().copied().unwrap_or(1.0);
        let global_zero_point = zero_points.first().copied().unwrap_or(0);

        self.int8_data = Some(Int8QuantizedData {
            vectors: int8_vectors,
            scales,
            zero_points,
        });

        self.metadata.int8_column = Some(Int8ColumnInfo {
            column_name: "int8_vectors".to_string(),
            global_scale,
            global_zero_point,
        });

        Ok(())
    }

    /// Build PQ quantized column using unified engine
    pub async fn build_pq_column(
        &mut self,
        vectors: &[Vec<f32>],
        num_segments: usize,
        bits_per_code: u8,
    ) -> Result<()> {
        if vectors.is_empty() {
            return Ok(());
        }

        // Generate codebook ID
        let codebook_id = format!("nova_pq_{}_{}", num_segments, bits_per_code);

        // Train PQ codebook using unified engine
        self.quantization_engine
            .train_pq_codebook(vectors, num_segments, bits_per_code, &codebook_id)
            .await?;

        // Quantize vectors using trained codebook
        let mut pq_codes = Vec::with_capacity(vectors.len());

        for vector in vectors {
            let codes = self.quantization_engine.quantize_to_pq(
                vector,
                num_segments,
                bits_per_code as u32,
            )?;
            pq_codes.push(codes);
        }

        self.pq_data = Some(PQQuantizedData {
            codes: pq_codes,
            codebook_id: codebook_id.clone(),
            num_segments,
            bits_per_code,
        });

        self.metadata.pq_column = Some(PQColumnInfo {
            column_name: "pq_codes".to_string(),
            num_segments,
            bits_per_code,
            codebook_id,
        });

        Ok(())
    }

    /// Convert to Arrow RecordBatch for storage
    pub fn to_record_batch(&self) -> Result<RecordBatch> {
        let mut fields = Vec::new();
        let mut arrays: Vec<ArrayRef> = Vec::new();

        // Add binary column if present
        if let Some(ref binary_data) = self.binary_data {
            fields.push(Field::new("binary_sketch", DataType::Binary, false));
            let binary_array: BinaryArray =
                binary_data.iter().map(|v| Some(v.as_slice())).collect();
            arrays.push(Arc::new(binary_array));
        }

        // Add INT8 column if present
        if let Some(ref int8_data) = self.int8_data {
            fields.push(Field::new("int8_vectors", DataType::Binary, false));
            let int8_array: BinaryArray = int8_data
                .vectors
                .iter()
                .map(|v| Some(v.as_slice()))
                .collect();
            arrays.push(Arc::new(int8_array));

            // Add scales
            fields.push(Field::new("int8_scales", DataType::Float32, false));
            let scales_array = Float32Array::from(int8_data.scales.clone());
            arrays.push(Arc::new(scales_array));

            // Add zero points
            fields.push(Field::new("int8_zero_points", DataType::Int8, false));
            let zero_points_array = Int8Array::from(int8_data.zero_points.clone());
            arrays.push(Arc::new(zero_points_array));
        }

        // Add PQ column if present
        if let Some(ref pq_data) = self.pq_data {
            fields.push(Field::new("pq_codes", DataType::Binary, false));
            let pq_array: BinaryArray = pq_data.codes.iter().map(|v| Some(v.as_slice())).collect();
            arrays.push(Arc::new(pq_array));
        }

        if fields.is_empty() {
            anyhow::bail!("No quantized columns to convert");
        }

        let schema = Arc::new(arrow_schema::Schema::new(fields));
        RecordBatch::try_new(schema, arrays)
            .map_err(|e| anyhow!("Failed to create RecordBatch: {}", e))
    }
}

/// Builder for creating quantized columns with configuration
pub struct QuantizedColumnBuilder {
    vectors: Vec<Vec<f32>>,
    dimension: usize,
    config: QuantizationConfig,
    quantization_engine: Arc<UnifiedQuantizationEngine>,
}

#[derive(Debug, Clone)]
pub struct QuantizationConfig {
    pub enable_binary: bool,
    pub enable_int8: bool,
    pub enable_pq: bool,
    pub pq_segments: i32,
    pub pq_bits: i32,
}

impl Default for QuantizationConfig {
    fn default() -> Self {
        Self {
            enable_binary: true,
            enable_int8: true,
            enable_pq: false,
            pq_segments: 8,
            pq_bits: 8,
        }
    }
}

impl QuantizedColumnBuilder {
    /// Create new builder with unified quantization engine
    pub fn new(
        vectors: Vec<Vec<f32>>,
        config: QuantizationConfig,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
    ) -> Self {
        let dimension = vectors.first().map_or(0, |v| v.len());
        Self {
            vectors,
            dimension,
            config,
            quantization_engine,
        }
    }

    /// Build all configured quantized columns
    pub async fn build(self) -> Result<QuantizedColumns> {
        let mut columns = QuantizedColumns::new(self.quantization_engine.clone());
        columns.metadata.dimension = self.dimension;
        columns.metadata.stats.num_vectors = self.vectors.len();

        // Build binary column if enabled
        if self.config.enable_binary {
            let _ = columns.build_binary_column(&self.vectors).await?;
        }

        // Build INT8 column if enabled
        if self.config.enable_int8 {
            let _ = columns.build_int8_column(&self.vectors).await?;
        }

        // Build PQ column if enabled
        if self.config.enable_pq {
            let _ = columns
                .build_pq_column(
                    &self.vectors,
                    self.config.pq_segments as usize,
                    self.config.pq_bits as u8,
                )
                .await?;
        }

        // Calculate compression ratio
        let original_size = self.vectors.len() * self.dimension * std::mem::size_of::<f32>();
        let compressed_size = Self::calculate_compressed_size(&columns);
        columns.metadata.stats.compression_ratio = original_size as f32 / compressed_size as f32;

        Ok(columns)
    }

    fn calculate_compressed_size(columns: &QuantizedColumns) -> usize {
        let mut size = 0;

        if let Some(ref binary) = columns.binary_data {
            size += binary.iter().map(|v| v.len()).sum::<usize>();
        }

        if let Some(ref int8) = columns.int8_data {
            size += int8.vectors.iter().map(|v| v.len()).sum::<usize>();
            size += int8.scales.len() * std::mem::size_of::<f32>();
            size += int8.zero_points.len();
        }

        if let Some(ref pq) = columns.pq_data {
            size += pq.codes.iter().map(|v| v.len()).sum::<usize>();
        }

        size
    }
}

/// Helper to compute Hamming distance between binary vectors
pub fn hamming_distance(a: &[u8], b: &[u8]) -> u32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x ^ y).count_ones())
        .sum()
}

/// Helper to compute asymmetric distance for PQ
pub fn asymmetric_distance_pq(
    _query: &[f32],
    _pq_code: &[u8],
    _codebook: &Codebook,
) -> Result<f32> {
    // This will use the unified engine's PQ distance computation
    // For now, return placeholder
    Ok(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_binary_quantization() {
        // Test will use unified quantization engine
        // Placeholder for now
    }

    #[tokio::test]
    async fn test_int8_quantization() {
        // Test will use unified quantization engine
        // Placeholder for now
    }

    #[tokio::test]
    async fn test_pq_quantization() {
        // Test will use unified quantization engine
        // Placeholder for now
    }
}
