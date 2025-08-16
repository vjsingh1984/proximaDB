// Columnar Quantization Adapter
// Bridges the unified quantization engine with columnar storage formats

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use crate::core::{VectorRecord, hardware_capabilities::HardwareCapabilities};
use crate::compute::quantization::unified::UnifiedQuantizationEngine;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use super::{QuantizationConfig, QuantizationLevel};

/// Adapts unified quantization for columnar storage
pub struct ColumnarQuantizationAdapter {
    /// Unified quantization engine
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    
    /// Hardware capabilities
    hardware: Arc<HardwareCapabilities>,
    
    /// Configuration
    config: QuantizationConfig,
}

impl ColumnarQuantizationAdapter {
    /// Create new columnar quantization adapter
    pub fn new(
        quantization_engine: Arc<UnifiedQuantizationEngine>,
        hardware: Arc<HardwareCapabilities>,
        config: QuantizationConfig,
    ) -> Self {
        Self {
            quantization_engine,
            hardware,
            config,
        }
    }
    
    /// Quantize vectors for columnar storage
    pub async fn quantize_vectors_for_storage(
        &self,
        vectors: &[VectorRecord],
        level: QuantizationLevel,
    ) -> Result<QuantizedVectorBatch> {
        info!("Quantizing {} vectors for columnar storage at level: {:?}", vectors.len(), level);
        
        match level {
            QuantizationLevel::None => self.no_quantization(vectors).await,
            QuantizationLevel::Binary => self.binary_quantization(vectors).await,
            QuantizationLevel::Int8 => self.int8_quantization(vectors).await,
            QuantizationLevel::ProductQuantization => self.pq_quantization(vectors).await,
            QuantizationLevel::Progressive => self.progressive_quantization(vectors).await,
        }
    }
    
    /// No quantization - pass through original vectors
    async fn no_quantization(&self, vectors: &[VectorRecord]) -> Result<QuantizedVectorBatch> {
        let fp32_vectors: Vec<Vec<f32>> = vectors.iter()
            .map(|v| v.vector.clone())
            .collect();
            
        Ok(QuantizedVectorBatch {
            original_vectors: fp32_vectors,
            binary_vectors: None,
            int8_vectors: None,
            pq_vectors: None,
            quantization_metadata: QuantizationMetadata {
                level: QuantizationLevel::None,
                dimension: vectors.first().map(|v| v.vector.len()).unwrap_or(0),
                compression_ratio: 1.0,
                reconstruction_error: 0.0,
            },
        })
    }
    
    /// Binary quantization for columnar storage
    async fn binary_quantization(&self, vectors: &[VectorRecord]) -> Result<QuantizedVectorBatch> {
        debug!("Applying binary quantization to {} vectors", vectors.len());
        
        let fp32_vectors: Vec<Vec<f32>> = vectors.iter()
            .map(|v| v.vector.clone())
            .collect();
        
        // Use unified quantization engine for binary quantization
        let mut binary_vectors = Vec::new();
        for vector in &fp32_vectors {
            let binary = self.quantization_engine.quantize_binary(vector)?;
            binary_vectors.push(binary);
        }
        
        let dimension = fp32_vectors.first().map(|v| v.len()).unwrap_or(0);
        let compression_ratio = (dimension * 32) as f32 / dimension as f32; // 32:1 compression
        
        Ok(QuantizedVectorBatch {
            original_vectors: fp32_vectors,
            binary_vectors: Some(binary_vectors),
            int8_vectors: None,
            pq_vectors: None,
            quantization_metadata: QuantizationMetadata {
                level: QuantizationLevel::Binary,
                dimension,
                compression_ratio,
                reconstruction_error: 0.1, // Approximate
            },
        })
    }
    
    /// INT8 quantization for columnar storage
    async fn int8_quantization(&self, vectors: &[VectorRecord]) -> Result<QuantizedVectorBatch> {
        debug!("Applying INT8 quantization to {} vectors", vectors.len());
        
        let fp32_vectors: Vec<Vec<f32>> = vectors.iter()
            .map(|v| v.vector.clone())
            .collect();
        
        // Use unified quantization engine for INT8 quantization
        let mut int8_vectors = Vec::new();
        for vector in &fp32_vectors {
            let int8 = self.quantization_engine.quantize_int8(vector)?;
            int8_vectors.push(int8);
        }
        
        let dimension = fp32_vectors.first().map(|v| v.len()).unwrap_or(0);
        let compression_ratio = 4.0; // 32-bit to 8-bit
        
        Ok(QuantizedVectorBatch {
            original_vectors: fp32_vectors,
            binary_vectors: None,
            int8_vectors: Some(int8_vectors),
            pq_vectors: None,
            quantization_metadata: QuantizationMetadata {
                level: QuantizationLevel::Int8,
                dimension,
                compression_ratio,
                reconstruction_error: 0.05, // Approximate
            },
        })
    }
    
    /// Product quantization for columnar storage
    async fn pq_quantization(&self, vectors: &[VectorRecord]) -> Result<QuantizedVectorBatch> {
        debug!("Applying PQ quantization to {} vectors", vectors.len());
        
        let fp32_vectors: Vec<Vec<f32>> = vectors.iter()
            .map(|v| v.vector.clone())
            .collect();
        
        // Use unified quantization engine for PQ quantization
        let mut pq_vectors = Vec::new();
        for vector in &fp32_vectors {
            let pq = self.quantization_engine.quantize_pq(
                vector,
                self.config.pq_segments,
                self.config.pq_bits,
            )?;
            pq_vectors.push(pq);
        }
        
        let dimension = fp32_vectors.first().map(|v| v.len()).unwrap_or(0);
        let compression_ratio = (dimension * 32) as f32 / (self.config.pq_segments * self.config.pq_bits) as f32;
        
        Ok(QuantizedVectorBatch {
            original_vectors: fp32_vectors,
            binary_vectors: None,
            int8_vectors: None,
            pq_vectors: Some(pq_vectors),
            quantization_metadata: QuantizationMetadata {
                level: QuantizationLevel::ProductQuantization,
                dimension,
                compression_ratio,
                reconstruction_error: 0.02, // Approximate
            },
        })
    }
    
    /// Progressive quantization with multiple levels
    async fn progressive_quantization(&self, vectors: &[VectorRecord]) -> Result<QuantizedVectorBatch> {
        info!("Applying progressive quantization to {} vectors", vectors.len());
        
        let fp32_vectors: Vec<Vec<f32>> = vectors.iter()
            .map(|v| v.vector.clone())
            .collect();
        
        // Apply all quantization levels for progressive search
        let mut binary_vectors = Vec::new();
        let mut int8_vectors = Vec::new();
        let mut pq_vectors = Vec::new();
        
        for vector in &fp32_vectors {
            // Binary quantization
            if self.config.enable_binary {
                let binary = self.quantization_engine.quantize_binary(vector)?;
                binary_vectors.push(binary);
            }
            
            // INT8 quantization
            if self.config.enable_int8 {
                let int8 = self.quantization_engine.quantize_int8(vector)?;
                int8_vectors.push(int8);
            }
            
            // PQ quantization
            if self.config.enable_pq {
                let pq = self.quantization_engine.quantize_pq(
                    vector,
                    self.config.pq_segments,
                    self.config.pq_bits,
                )?;
                pq_vectors.push(pq);
            }
        }
        
        let dimension = fp32_vectors.first().map(|v| v.len()).unwrap_or(0);
        
        Ok(QuantizedVectorBatch {
            original_vectors: fp32_vectors,
            binary_vectors: if binary_vectors.is_empty() { None } else { Some(binary_vectors) },
            int8_vectors: if int8_vectors.is_empty() { None } else { Some(int8_vectors) },
            pq_vectors: if pq_vectors.is_empty() { None } else { Some(pq_vectors) },
            quantization_metadata: QuantizationMetadata {
                level: QuantizationLevel::Progressive,
                dimension,
                compression_ratio: 16.0, // Average compression
                reconstruction_error: 0.03, // Average error
            },
        })
    }
    
    /// Convert quantized batch to columnar format
    pub fn to_columnar_format(&self, batch: &QuantizedVectorBatch) -> Result<ColumnarQuantizedData> {
        debug!("Converting quantized batch to columnar format");
        
        let num_vectors = batch.original_vectors.len();
        let dimension = batch.quantization_metadata.dimension;
        
        // Organize data in columnar format for efficient Parquet storage
        let mut columnar_data = ColumnarQuantizedData {
            num_vectors,
            dimension,
            fp32_column: batch.original_vectors.clone(),
            binary_column: None,
            int8_column: None,
            pq_column: None,
            metadata: batch.quantization_metadata.clone(),
        };
        
        // Add quantized columns if present
        if let Some(ref binary_vectors) = batch.binary_vectors {
            columnar_data.binary_column = Some(binary_vectors.clone());
        }
        
        if let Some(ref int8_vectors) = batch.int8_vectors {
            columnar_data.int8_column = Some(int8_vectors.clone());
        }
        
        if let Some(ref pq_vectors) = batch.pq_vectors {
            columnar_data.pq_column = Some(pq_vectors.clone());
        }
        
        Ok(columnar_data)
    }
    
    /// Progressive search using quantized columns
    pub async fn progressive_search(
        &self,
        query: &[f32],
        columnar_data: &ColumnarQuantizedData,
        top_k: usize,
    ) -> Result<Vec<ProgressiveSearchResult>> {
        info!("Progressive search with {} candidates", columnar_data.num_vectors);
        
        let mut candidates: Vec<usize> = (0..columnar_data.num_vectors).collect();
        
        // Stage 1: Binary filtering (if available)
        if let Some(ref binary_column) = columnar_data.binary_column {
            candidates = self.binary_filter(query, binary_column, &candidates, top_k * 10).await?;
            debug!("Binary filter: {} candidates remaining", candidates.len());
        }
        
        // Stage 2: INT8 filtering (if available)
        if let Some(ref int8_column) = columnar_data.int8_column {
            candidates = self.int8_filter(query, int8_column, &candidates, top_k * 5).await?;
            debug!("INT8 filter: {} candidates remaining", candidates.len());
        }
        
        // Stage 3: PQ filtering (if available)
        if let Some(ref pq_column) = columnar_data.pq_column {
            candidates = self.pq_filter(query, pq_column, &candidates, top_k * 2).await?;
            debug!("PQ filter: {} candidates remaining", candidates.len());
        }
        
        // Stage 4: Full precision ranking
        let results = self.final_ranking(query, &columnar_data.fp32_column, &candidates, top_k).await?;
        
        Ok(results)
    }
    
    /// Binary filtering stage
    async fn binary_filter(
        &self,
        query: &[f32],
        binary_column: &[Vec<u8>],
        candidates: &[usize],
        max_candidates: usize,
    ) -> Result<Vec<usize>> {
        // Quantize query to binary
        let binary_query = self.quantization_engine.quantize_binary(query)?;
        
        // Compute binary distances using SIMD if available
        let mut scored_candidates = Vec::new();
        
        for &idx in candidates {
            if idx < binary_column.len() {
                let distance = self.hamming_distance(&binary_query, &binary_column[idx]);
                scored_candidates.push((idx, distance));
            }
        }
        
        // Sort by distance and take top candidates
        scored_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        scored_candidates.truncate(max_candidates);
        
        Ok(scored_candidates.into_iter().map(|(idx, _)| idx).collect())
    }
    
    /// INT8 filtering stage
    async fn int8_filter(
        &self,
        query: &[f32],
        int8_column: &[Vec<i8>],
        candidates: &[usize],
        max_candidates: usize,
    ) -> Result<Vec<usize>> {
        // Quantize query to INT8
        let int8_query = self.quantization_engine.quantize_int8(query)?;
        
        // Compute INT8 distances using SIMD if available
        let mut scored_candidates = Vec::new();
        
        for &idx in candidates {
            if idx < int8_column.len() {
                let distance = self.int8_distance(&int8_query, &int8_column[idx]);
                scored_candidates.push((idx, distance));
            }
        }
        
        // Sort by distance and take top candidates
        scored_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        scored_candidates.truncate(max_candidates);
        
        Ok(scored_candidates.into_iter().map(|(idx, _)| idx).collect())
    }
    
    /// PQ filtering stage
    async fn pq_filter(
        &self,
        query: &[f32],
        pq_column: &[Vec<u8>],
        candidates: &[usize],
        max_candidates: usize,
    ) -> Result<Vec<usize>> {
        // Quantize query to PQ
        let pq_query = self.quantization_engine.quantize_pq(
            query,
            self.config.pq_segments,
            self.config.pq_bits,
        )?;
        
        // Compute PQ distances using precomputed tables
        let mut scored_candidates = Vec::new();
        
        for &idx in candidates {
            if idx < pq_column.len() {
                let distance = self.pq_distance(&pq_query, &pq_column[idx]);
                scored_candidates.push((idx, distance));
            }
        }
        
        // Sort by distance and take top candidates
        scored_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        scored_candidates.truncate(max_candidates);
        
        Ok(scored_candidates.into_iter().map(|(idx, _)| idx).collect())
    }
    
    /// Final ranking with full precision
    async fn final_ranking(
        &self,
        query: &[f32],
        fp32_column: &[Vec<f32>],
        candidates: &[usize],
        top_k: usize,
    ) -> Result<Vec<ProgressiveSearchResult>> {
        // Use unified distance compute for final ranking
        let distance_compute = UnifiedDistanceCompute::default();
        let mut scored_candidates = Vec::new();
        
        for &idx in candidates {
            if idx < fp32_column.len() {
                let distance = distance_compute.euclidean_distance(query, &fp32_column[idx])?;
                scored_candidates.push(ProgressiveSearchResult {
                    vector_index: idx,
                    final_distance: distance,
                    binary_distance: None,
                    int8_distance: None,
                    pq_distance: None,
                });
            }
        }
        
        // Sort by final distance and take top-k
        scored_candidates.sort_by(|a, b| a.final_distance.partial_cmp(&b.final_distance).unwrap());
        scored_candidates.truncate(top_k);
        
        Ok(scored_candidates)
    }
    
    /// Hamming distance for binary vectors
    fn hamming_distance(&self, a: &[u8], b: &[u8]) -> f32 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (x ^ y).count_ones() as f32)
            .sum()
    }
    
    /// Distance for INT8 vectors
    fn int8_distance(&self, a: &[i8], b: &[i8]) -> f32 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (*x as f32 - *y as f32).powi(2))
            .sum::<f32>()
            .sqrt()
    }
    
    /// Distance for PQ vectors
    fn pq_distance(&self, a: &[u8], b: &[u8]) -> f32 {
        // Simplified PQ distance - in production would use lookup tables
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (*x as f32 - *y as f32).abs())
            .sum()
    }
}

/// Batch of quantized vectors
#[derive(Debug, Clone)]
pub struct QuantizedVectorBatch {
    pub original_vectors: Vec<Vec<f32>>,
    pub binary_vectors: Option<Vec<Vec<u8>>>,
    pub int8_vectors: Option<Vec<Vec<i8>>>,
    pub pq_vectors: Option<Vec<Vec<u8>>>,
    pub quantization_metadata: QuantizationMetadata,
}

/// Quantization metadata
#[derive(Debug, Clone)]
pub struct QuantizationMetadata {
    pub level: QuantizationLevel,
    pub dimension: usize,
    pub compression_ratio: f32,
    pub reconstruction_error: f32,
}

/// Columnar quantized data format
#[derive(Debug, Clone)]
pub struct ColumnarQuantizedData {
    pub num_vectors: usize,
    pub dimension: usize,
    pub fp32_column: Vec<Vec<f32>>,
    pub binary_column: Option<Vec<Vec<u8>>>,
    pub int8_column: Option<Vec<Vec<i8>>>,
    pub pq_column: Option<Vec<Vec<u8>>>,
    pub metadata: QuantizationMetadata,
}

/// Progressive search result
#[derive(Debug, Clone)]
pub struct ProgressiveSearchResult {
    pub vector_index: usize,
    pub final_distance: f32,
    pub binary_distance: Option<f32>,
    pub int8_distance: Option<f32>,
    pub pq_distance: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::quantization::unified::InMemoryCodebookStore;
    
    #[tokio::test]
    async fn test_columnar_quantization_adapter() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let hardware = HardwareCapabilities::get().unwrap();
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute,
            codebook_store,
        ));
        
        let config = QuantizationConfig::default();
        let adapter = ColumnarQuantizationAdapter::new(
            quantization_engine,
            hardware,
            config,
        );
        
        // Create test vectors
        let vectors = vec![
            VectorRecord {
                id: Some("test1".to_string()),
                vector: vec![1.0, 2.0, 3.0, 4.0],
                metadata: None,
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
            },
            VectorRecord {
                id: Some("test2".to_string()),
                vector: vec![5.0, 6.0, 7.0, 8.0],
                metadata: None,
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
            },
        ];
        
        // Test binary quantization
        let batch = adapter.quantize_vectors_for_storage(&vectors, QuantizationLevel::Binary).await.unwrap();
        assert!(batch.binary_vectors.is_some());
        assert_eq!(batch.original_vectors.len(), 2);
        
        // Test columnar format conversion
        let columnar_data = adapter.to_columnar_format(&batch).unwrap();
        assert_eq!(columnar_data.num_vectors, 2);
        assert_eq!(columnar_data.dimension, 4);
    }
}