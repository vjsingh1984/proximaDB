// Quantized columns for VIPER - Efficient columnar storage with multi-level quantization
// Clean implementation for Parquet-based progressive search
// MIGRATION: Integrating with universal quantization adapter

use anyhow::{anyhow, Result};
use arrow_array::array::{ArrayRef, BinaryArray, Float32Array, Int8Array, UInt8Array};
use arrow_schema::{DataType, Field};
use arrow_array::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::{WriterProperties, WriterVersion};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// MIGRATION: Import universal quantization types
use crate::storage::engines::common::{
    UniversalQuantizationAdapter,
    UniversalQuantizationConfig,
    quantization_common::{
    },
};

// Define quantization types locally for now
#[derive(Debug, Clone)]
pub struct BinarySketch {
    pub bits: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Int8Vector {
    pub values: Vec<i8>,
    pub scale: f32,
    pub zero_point: i8,
}

#[derive(Debug, Clone)]
pub struct PQCode {
    pub codes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct DistanceTable {
    pub table: Vec<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct Codebook {
    pub centroids: Vec<Vec<f32>>,
}

// Additional structures for columnar storage
// NOTE: QuantizedColumns struct moved below to avoid duplication

#[derive(Debug, Clone)]
pub struct BinaryColumn {
    pub data: Vec<Vec<u8>>,
    pub bits_per_vector: usize,
}

#[derive(Debug, Clone)]
pub struct Int8Column {
    pub vectors: Vec<Int8Vector>,
}

#[derive(Debug, Clone)]
pub struct PQColumn {
    pub codes: Vec<PQCode>,
}

/// Metadata for quantized columns in Parquet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizedColumnMetadata {
    /// Binary column info
    pub binary_column: Option<BinaryColumnInfo>,
    
    /// INT8 column info
    pub int8_column: Option<Int8ColumnInfo>,
    
    /// PQ column info
    pub pq_column: Option<PQColumnInfo>,
    
    /// Statistics for query optimization
    pub quantization_stats: QuantizationStatistics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryColumnInfo {
    pub column_name: String,
    pub bits_per_vector: usize,
    pub threshold: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Int8ColumnInfo {
    pub column_name: String,
    pub scale_column: String,
    pub zero_point_column: String,
    pub global_scale: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PQColumnInfo {
    pub column_name: String,
    pub num_segments: u8,
    pub bits_per_segment: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationStatistics {
    pub avg_reconstruction_error: f32,
    pub max_reconstruction_error: f32,
    pub compression_ratio: f32,
    pub quantization_time_ms: u64,
}

/// Builder for creating quantized columns
pub struct QuantizedColumnBuilder {
    dimension: usize,
    vectors: Vec<Vec<f32>>,
    config: QuantizationConfig,
}

#[derive(Debug, Clone)]
pub struct QuantizationConfig {
    pub enable_binary: bool,
    pub enable_int8: bool,
    pub enable_pq: bool,
    pub pq_segments: u8,
    pub pq_bits: u8,
    pub binary_threshold: f32,
}

impl QuantizedColumnBuilder {
    pub fn new(dimension: usize, config: QuantizationConfig) -> Self {
        Self {
            dimension,
            vectors: Vec::new(),
            config,
        }
    }
    
    /// Add vectors to be quantized
    pub fn add_vectors(&mut self, vectors: Vec<Vec<f32>>) {
        self.vectors.extend(vectors);
    }
    
    /// Build all quantized columns using universal adapter
    pub fn build_with_adapter(
        &self,
        adapter: &UniversalQuantizationAdapter,
        config: &UniversalQuantizationConfig,
    ) -> Result<QuantizedColumns> {
        let start_time = std::time::Instant::now();
        
        // Use universal adapter for quantization
        let result = adapter.quantize_progressive(&self.vectors, config)?;
        
        let mut columns = QuantizedColumns {
            binary_column: None,
            int8_column: None,
            pq_column: None,
            original_dimension: self.dimension,
            num_vectors: self.vectors.len(),
        };
        
        // Extract columnar quantized representations from result
        if !result.quantized_vectors.is_empty() {
            // Collect binary sketches if present
            let binary_data: Vec<Vec<u8>> = result.quantized_vectors.iter()
                .filter_map(|qv| qv.binary_sketch.clone())
                .collect();
            if !binary_data.is_empty() {
                columns.binary_column = Some(BinaryColumn {
                    data: binary_data,
                    bits_per_vector: self.dimension,
                });
            }
            
            // Collect INT8 vectors if present
            let int8_data: Vec<Int8Vector> = result.quantized_vectors.iter()
                .filter_map(|qv| qv.int8_vector.as_ref().map(|v| Int8Vector {
                    values: v.values.clone(),
                    scale: v.scale,
                    zero_point: v.zero_point as i8,
                }))
                .collect();
            if !int8_data.is_empty() {
                columns.int8_column = Some(Int8Column {
                    vectors: int8_data,
                });
            }
            
            // Collect PQ codes if present
            let pq_data: Vec<PQCode> = result.quantized_vectors.iter()
                .filter_map(|qv| qv.pq_code.as_ref().map(|pq| PQCode {
                    codes: pq.codes.clone(),
                }))
                .collect();
            if !pq_data.is_empty() {
                columns.pq_column = Some(PQColumn {
                    codes: pq_data,
                    // codebooks removed -  Vec::new(), // Would be populated from adapter
                });
            }
        }
        
        // Calculate statistics
        let quantization_time_ms = start_time.elapsed().as_millis() as u64;
        
        Ok(columns)
    }
    
    /// Legacy build method (deprecated, use build_with_adapter)
    pub fn build(&self) -> Result<QuantizedColumns> {
        let start_time = std::time::Instant::now();
        
        let mut columns = QuantizedColumns {
            binary_column: None,
            int8_column: None,
            pq_column: None,
            original_dimension: self.dimension,
            num_vectors: self.vectors.len(),
        };
        
        // Build binary column
        if self.config.enable_binary {
            columns.binary_column = Some(self.build_binary_column()?);
        }
        
        // Build INT8 column
        if self.config.enable_int8 {
            columns.int8_column = Some(self.build_int8_column()?);
        }
        
        // Build PQ column
        if self.config.enable_pq {
            columns.pq_column = Some(self.build_pq_column()?);
        }
        
        // Calculate statistics
        let quantization_time_ms = start_time.elapsed().as_millis() as u64;
        
        Ok(columns)
    }
    
    /// Build binary quantized column
    fn build_binary_column(&self) -> Result<BinaryQuantizedColumn> {
        let mut sketches = Vec::new();
        let bits_per_vector = (self.dimension + 63) / 64 * 64; // Round up to multiple of 64
        
        for vector in &self.vectors {
            let sketch = BinarySketch::from_vector(vector, self.config.binary_threshold);
            sketches.push(sketch);
        }
        
        Ok(BinaryQuantizedColumn {
            sketches,
            bits_per_vector,
            threshold: self.config.binary_threshold,
        })
    }
    
    /// Build INT8 quantized column
    fn build_int8_column(&self) -> Result<Int8QuantizedColumn> {
        // Calculate global scale for all vectors
        let mut global_min = f32::INFINITY;
        let mut global_max = f32::NEG_INFINITY;
        
        for vector in &self.vectors {
            for &value in vector {
                global_min = global_min.min(value);
                global_max = global_max.max(value);
            }
        }
        
        let global_scale = (global_max - global_min) / 255.0;
        let global_zero_point = (-global_min / global_scale).round() as i8;
        
        // Quantize all vectors
        let mut quantized_vectors = Vec::new();
        let mut scales = Vec::new();
        let mut zero_points = Vec::new();
        
        for vector in &self.vectors {
            let int8_vec = Int8Vector::from_vector(vector);
            scales.push(int8_vec.scale);
            zero_points.push(int8_vec.zero_point);
            quantized_vectors.push(int8_vec);
        }
        
        Ok(Int8QuantizedColumn {
            vectors: quantized_vectors,
            scales,
            zero_points,
            global_scale: Some(global_scale),
            global_zero_point: Some(global_zero_point),
        })
    }
    
    /// Build Product Quantization column
    fn build_pq_column(&self) -> Result<PQQuantizedColumn> {
        let num_segments = self.config.pq_segments as usize;
        let segment_dim = self.dimension / num_segments;
        
        // Train codebooks on sample vectors
        let sample_size = self.vectors.len().min(10000);
        let sample_vectors: Vec<_> = self.vectors.iter().take(sample_size).cloned().collect();
        
        let mut codebooks = Vec::new();
        for seg_id in 0..num_segments {
            let start = seg_id * segment_dim;
            let end = start + segment_dim;
            
            // Extract segment vectors
            let segment_vectors: Vec<Vec<f32>> = sample_vectors.iter()
                .map(|v| v[start..end].to_vec())
                .collect();
            
            // Train codebook for this segment
            let codebook = train_codebook(seg_id as u8, &segment_vectors, 256)?;
            codebooks.push(codebook);
        }
        
        // Encode all vectors
        let mut pq_codes = Vec::new();
        for vector in &self.vectors {
            let code = PQCode::encode(vector, &codebooks);
            pq_codes.push(code);
        }
        
        Ok(PQQuantizedColumn {
            codes: pq_codes,
            codebooks,
            num_segments: self.config.pq_segments,
            bits_per_segment: self.config.pq_bits,
        })
    }
}

/// Container for all quantized columns
#[derive(Debug)]
pub struct QuantizedColumns {
    pub binary_column: Option<BinaryQuantizedColumn>,
    pub int8_column: Option<Int8QuantizedColumn>,
    pub pq_column: Option<PQQuantizedColumn>,
    pub original_dimension: usize,
    pub num_vectors: usize,
}

#[derive(Debug)]
pub struct BinaryQuantizedColumn {
    pub sketches: Vec<BinarySketch>,
    pub bits_per_vector: usize,
    pub threshold: f32,
}

#[derive(Debug)]
pub struct Int8QuantizedColumn {
    pub vectors: Vec<Int8Vector>,
    pub scales: Vec<f32>,
    pub zero_points: Vec<i8>,
    pub global_scale: Option<f32>,
    pub global_zero_point: Option<i8>,
}

#[derive(Debug)]
pub struct PQQuantizedColumn {
    pub codes: Vec<PQCode>,
    pub num_segments: u8,
    pub bits_per_segment: u8,
}

impl QuantizedColumns {
    /// Convert to Arrow arrays for Parquet writing
    pub fn to_arrow_arrays(&self) -> Result<Vec<(String, ArrayRef)>> {
        let mut arrays = Vec::new();
        
        // Binary column
        if let Some(binary) = &self.binary_column {
            let binary_data = self.binary_to_arrow(binary)?;
            arrays.push(("vector_binary".to_string(), binary_data));
        }
        
        // INT8 columns
        if let Some(int8) = &self.int8_column {
            let (int8_data, scales, zero_points) = self.int8_to_arrow(int8)?;
            arrays.push(("vector_int8".to_string(), int8_data));
            arrays.push(("int8_scale".to_string(), scales));
            arrays.push(("int8_zero_point".to_string(), zero_points));
        }
        
        // PQ column
        if let Some(pq) = &self.pq_column {
            let pq_data = self.pq_to_arrow(pq)?;
            arrays.push(("vector_pq".to_string(), pq_data));
        }
        
        Ok(arrays)
    }
    
    fn binary_to_arrow(&self, binary: &BinaryQuantizedColumn) -> Result<ArrayRef> {
        let mut binary_data = Vec::new();
        
        for sketch in &binary.sketches {
            // Convert bits to bytes
            let mut bytes = Vec::new();
            for word in &sketch.bits {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            binary_data.push(bytes);
        }
        
        Ok(Arc::new(BinaryArray::from(binary_data)))
    }
    
    fn int8_to_arrow(&self, int8: &Int8QuantizedColumn) -> Result<(ArrayRef, ArrayRef, ArrayRef)> {
        let mut int8_data = Vec::new();
        let mut scales = Vec::new();
        let mut zero_points = Vec::new();
        
        for (vec, scale, zp) in int8.vectors.iter()
            .zip(int8.scales.iter())
            .zip(int8.zero_points.iter())
            .map(|((v, s), z)| (v, s, z))
        {
            int8_data.push(vec.values.clone());
            scales.push(*scale);
            zero_points.push(*zp);
        }
        
        // Convert to Arrow arrays
        let int8_array = Arc::new(BinaryArray::from(
            int8_data.into_iter().map(|v| {
                v.into_iter().map(|x| x as u8).collect::<Vec<u8>>()
            }).collect::<Vec<_>>()
        ));
        
        let scale_array = Arc::new(Float32Array::from(scales));
        let zp_array = Arc::new(Int8Array::from(zero_points));
        
        Ok((int8_array, scale_array, zp_array))
    }
    
    fn pq_to_arrow(&self, pq: &PQQuantizedColumn) -> Result<ArrayRef> {
        let mut pq_data = Vec::new();
        
        for code in &pq.codes {
            pq_data.push(code.codes.clone());
        }
        
        Ok(Arc::new(BinaryArray::from(pq_data)))
    }
    
    /// Calculate compression ratio
    pub fn compression_ratio(&self) -> f32 {
        let original_size = self.num_vectors * self.original_dimension * 4; // f32
        let mut compressed_size = 0;
        
        if let Some(binary) = &self.binary_column {
            compressed_size += self.num_vectors * (binary.bits_per_vector / 8);
        }
        
        if let Some(int8) = &self.int8_column {
            compressed_size += self.num_vectors * self.original_dimension; // i8
            compressed_size += self.num_vectors * 5; // scale + zero_point
        }
        
        if let Some(pq) = &self.pq_column {
            compressed_size += self.num_vectors * pq.num_segments as usize;
        }
        
        original_size as f32 / compressed_size.max(1) as f32
    }
}

/// Helper function to train a codebook (simplified k-means)
fn train_codebook(segment_id: u8, vectors: &[Vec<f32>], n_centroids: usize) -> Result<Codebook> {
    if vectors.is_empty() {
        return Err(anyhow!("Cannot train codebook with empty vectors"));
    }
    
    let dimension = vectors[0].len();
    let mut centroids = Vec::new();
    
    // Initialize with random vectors
    use rand::seq::SliceRandom;
    let mut rng = rand::thread_rng();
    let mut indices: Vec<usize> = (0..vectors.len()).collect();
    indices.shuffle(&mut rng);
    
    for i in 0..n_centroids.min(vectors.len()) {
        centroids.push(vectors[indices[i]].clone());
    }
    
    // Simple k-means iterations
    for _ in 0..10 {
        let mut clusters: Vec<Vec<Vec<f32>>> = vec![Vec::new(); n_centroids];
        
        // Assignment step
        for vector in vectors {
            let mut best_idx = 0;
            let mut best_dist = f32::INFINITY;
            
            for (idx, centroid) in centroids.iter().enumerate() {
                let dist = euclidean_distance(vector, centroid);
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = idx;
                }
            }
            
            clusters[best_idx].push(vector.clone());
        }
        
        // Update step
        for (idx, cluster) in clusters.iter().enumerate() {
            if !cluster.is_empty() {
                let mut new_centroid = vec![0.0; dimension];
                for vector in cluster {
                    for (i, &v) in vector.iter().enumerate() {
                        new_centroid[i] += v;
                    }
                }
                for v in &mut new_centroid {
                    *v /= cluster.len() as f32;
                }
                centroids[idx] = new_centroid;
            }
        }
    }
    
    Ok(Codebook {
        segment_id,
        dimension,
        centroids,
    })
}

fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_quantized_column_builder() {
        let config = QuantizationConfig {
            enable_binary: true,
            enable_int8: true,
            enable_pq: false, // Disable PQ for faster test
            pq_segments: 8,
            pq_bits: 8,
            binary_threshold: 0.0,
        };
        
        let mut builder = QuantizedColumnBuilder::new(128, config);
        
        // Add test vectors
        let vectors: Vec<Vec<f32>> = (0..100)
            .map(|i| {
                (0..128).map(|j| (i as f32 + j as f32) / 128.0).collect()
            })
            .collect();
        
        builder.add_vectors(vectors);
        
        // Build columns
        let columns = builder.build().unwrap();
        
        assert!(columns.binary_column.is_some());
        assert!(columns.int8_column.is_some());
        assert_eq!(columns.num_vectors, 100);
        assert_eq!(columns.original_dimension, 128);
        
        // Check compression ratio
        let ratio = columns.compression_ratio();
        assert!(ratio > 1.0); // Should have some compression
    }
    
    #[test]
    fn test_arrow_conversion() {
        let config = QuantizationConfig {
            enable_binary: true,
            enable_int8: false,
            enable_pq: false,
            pq_segments: 8,
            pq_bits: 8,
            binary_threshold: 0.0,
        };
        
        let mut builder = QuantizedColumnBuilder::new(64, config);
        
        let vectors: Vec<Vec<f32>> = (0..10)
            .map(|i| vec![i as f32 / 10.0; 64])
            .collect();
        
        builder.add_vectors(vectors);
        
        let columns = builder.build().unwrap();
        let arrays = columns.to_arrow_arrays().unwrap();
        
        assert!(!arrays.is_empty());
        assert_eq!(arrays[0].0, "vector_binary");
    }
}