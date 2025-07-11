//! Two-Stage Search with Quantized Filtering and FP32 Verification
//!
//! This module implements an efficient two-stage search strategy:
//! 1. Stage 1: Fast candidate selection using quantized vectors
//! 2. Stage 2: Accurate refinement using FP32 vectors
//!
//! This approach provides configurable accuracy/speed tradeoffs while
//! maintaining high recall rates.

use anyhow::{anyhow, Context, Result};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::compute::{
    UnifiedDistanceCompute, UnifiedQuantizationEngine, UnifiedQuantizationLevel,
    DistanceMetric,
};
use crate::core::{String, SearchResult, search::SearchParams};
use super::column_projection::ColumnProjection;
use super::ViperEngine;

// Type alias to avoid ambiguity with quantization module
type ComputeQuantizedVector = crate::compute::unified_quantization::QuantizedVector;

// Arrow imports for Parquet reading
use arrow_array::{Array, Float32Array, ListArray, StringArray, Int64Array, Float64Array, BooleanArray, UInt8Array};

/// Configuration for two-stage search
#[derive(Debug, Clone)]
pub struct TwoStageSearchConfig {
    /// Multiplier for candidate selection (e.g., 3x means select 3*k candidates)
    pub candidate_multiplier: f32,
    
    /// Minimum candidates to select (even for small k)
    pub min_candidates: usize,
    
    /// Maximum candidates to select (to bound memory usage)
    pub max_candidates: usize,
    
    /// Enable parallel processing of candidates
    pub enable_parallel: bool,
    
    /// Batch size for parallel processing
    pub parallel_batch_size: usize,
    
    /// Accuracy threshold for early termination
    pub early_termination_threshold: Option<f32>,
}

impl Default for TwoStageSearchConfig {
    fn default() -> Self {
        Self {
            candidate_multiplier: 3.0,
            min_candidates: 100,
            max_candidates: 10000,
            enable_parallel: true,
            parallel_batch_size: 1000,
            early_termination_threshold: None,
        }
    }
}

/// Two-stage search engine
#[derive(Debug)]
pub struct TwoStageSearchEngine {
    /// Distance computation engine
    distance_compute: Arc<UnifiedDistanceCompute>,
    
    /// Quantization engine (handles both quantization and quantized distance)
    quantization_engine: Arc<UnifiedQuantizationEngine>,
    
    /// Configuration
    config: TwoStageSearchConfig,
}

/// Intermediate result from Stage 1
#[derive(Debug, Clone)]
pub struct CandidateResult {
    /// Vector ID
    pub id: String,
    
    /// Approximate distance from quantized vectors
    pub approx_distance: f32,
    
    /// Quantization level used
    pub quantization_level: UnifiedQuantizationLevel,
    
    /// File/location information for Stage 2 retrieval
    pub location: VectorLocation,
}

/// Location information for retrieving FP32 vectors
#[derive(Debug, Clone)]
pub struct VectorLocation {
    /// Parquet file path
    pub file_path: String,
    
    /// Row group index
    pub row_group: usize,
    
    /// Row index within group
    pub row_index: usize,
}

impl TwoStageSearchEngine {
    pub fn new(
        distance_compute: Arc<UnifiedDistanceCompute>,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
        config: TwoStageSearchConfig,
    ) -> Self {
        Self {
            distance_compute,
            quantization_engine,
            config,
        }
    }
    
    /// Execute two-stage search
    pub async fn search(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
        query_vector: &[f32],
        search_params: &SearchParams,
        column_projection: &ColumnProjection,
        distance_metric: &DistanceMetric,
    ) -> Result<Vec<SearchResult>> {
        let k = search_params.top_k.unwrap_or(10);
        let start_time = std::time::Instant::now();
        
        info!("🔍 Two-stage search: Starting for collection {} with k={}", collection_id, k);
        
        // Stage 1: Quantized candidate selection
        let candidates = self.stage1_quantized_search(
            viper_engine,
            collection_id,
            query_vector,
            k,
            search_params,
            column_projection,
            distance_metric,
        ).await?;
        
        let stage1_duration = start_time.elapsed();
        info!("🔍 Stage 1 complete: Found {} candidates in {:?}", candidates.len(), stage1_duration);
        
        // Stage 2: FP32 refinement
        let final_results = self.stage2_fp32_refinement(
            viper_engine,
            query_vector,
            candidates,
            k,
            distance_metric,
        ).await?;
        
        let total_duration = start_time.elapsed();
        info!("✅ Two-stage search complete: {} results in {:?} (Stage1: {:?})", 
              final_results.len(), total_duration, stage1_duration);
        
        Ok(final_results)
    }
    
    /// Stage 1: Fast candidate selection using quantized vectors
    async fn stage1_quantized_search(
        &self,
        viper_engine: &ViperEngine,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        search_params: &SearchParams,
        column_projection: &ColumnProjection,
        distance_metric: &DistanceMetric,
    ) -> Result<Vec<CandidateResult>> {
        // Calculate number of candidates to select
        let num_candidates = self.calculate_candidate_count(k);
        debug!("🔍 Stage 1: Selecting {} candidates for k={}", num_candidates, k);
        
        // Get Parquet files for collection
        let parquet_files = viper_engine.get_parquet_files_for_collection(collection_id).await?;
        
        let mut all_candidates = Vec::new();
        
        // Search each Parquet file using quantized columns only
        for (file_idx, parquet_file_path) in parquet_files.iter().enumerate() {
            debug!("🔍 Stage 1: Searching file {}/{}: {}", 
                   file_idx + 1, parquet_files.len(), parquet_file_path);
            
            let file_candidates = self.search_quantized_file(
                parquet_file_path,
                query_vector,
                num_candidates,
                search_params,
                column_projection,
                distance_metric,
            ).await?;
            
            all_candidates.extend(file_candidates);
        }
        
        // Sort by approximate distance and take top candidates
        all_candidates.sort_by(|a, b| {
            a.approx_distance.partial_cmp(&b.approx_distance)
                .unwrap_or(Ordering::Equal)
        });
        all_candidates.truncate(num_candidates);
        
        Ok(all_candidates)
    }
    
    /// Stage 2: Accurate refinement using FP32 vectors
    async fn stage2_fp32_refinement(
        &self,
        viper_engine: &ViperEngine,
        query_vector: &[f32],
        candidates: Vec<CandidateResult>,
        k: usize,
        distance_metric: &DistanceMetric,
    ) -> Result<Vec<SearchResult>> {
        if candidates.is_empty() {
            return Ok(vec![]);
        }
        
        // Store candidate count for logging
        let candidate_count = candidates.len();
        
        // Group candidates by file for efficient retrieval
        let mut candidates_by_file: HashMap<String, Vec<CandidateResult>> = HashMap::new();
        for candidate in candidates {
            candidates_by_file
                .entry(candidate.location.file_path.clone())
                .or_insert_with(Vec::new)
                .push(candidate);
        }
        
        let mut final_results = Vec::new();
        
        // Process each file
        for (file_path, file_candidates) in candidates_by_file {
            debug!("🔍 Stage 2: Processing {} candidates from {}", 
                   file_candidates.len(), file_path);
            
            // Retrieve FP32 vectors for candidates
            let fp32_vectors = self.retrieve_fp32_vectors(
                viper_engine,
                &file_path,
                &file_candidates,
            ).await?;
            
            // Calculate exact distances
            for (candidate, fp32_vector) in file_candidates.iter().zip(fp32_vectors.iter()) {
                let exact_distance = self.distance_compute.calculate_distance(
                    query_vector,
                    fp32_vector,
                    distance_metric,
                );
                
                final_results.push(SearchResult {
                    id: candidate.id.clone(),
                    vector_id: None,
                    score: exact_distance,
                    distance: Some(exact_distance),
                    rank: None,
                    vector: None,
                    metadata: HashMap::new(), // Would be populated from file
                    collection_id: None,
                    created_at: None,
                    algorithm_used: Some("two_stage_search".to_string()),
                    processing_time_us: None,
                });
            }
        }
        
        // Sort by exact distance and take top k
        final_results.sort_by(|a, b| {
            a.score.partial_cmp(&b.score).unwrap_or(Ordering::Equal)
        });
        final_results.truncate(k);
        
        // Log accuracy statistics
        info!("🎯 Two-stage search completed: {} candidates → {} final results", 
              candidate_count, final_results.len());
        
        Ok(final_results)
    }
    
    /// Search a single Parquet file using quantized columns
    async fn search_quantized_file(
        &self,
        parquet_file_path: &str,
        query_vector: &[f32],
        num_candidates: usize,
        search_params: &SearchParams,
        column_projection: &ColumnProjection,
        distance_metric: &DistanceMetric,
    ) -> Result<Vec<CandidateResult>> {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
        use std::fs::File;
        
        debug!("🔍 Stage 1: Searching quantized columns in {}", parquet_file_path);
        
        // Open Parquet file
        let file = File::open(parquet_file_path)
            .context(format!("Failed to open Parquet file: {}", parquet_file_path))?;
        let file_reader = ParquetRecordBatchReaderBuilder::try_new(file)?;
        let metadata = file_reader.metadata();
        
        // Build column projection for quantized vectors
        let mut projected_columns = vec!["id", "version"];
        
        // Add quantized column based on quantization hint
        let quantized_column = if let Some(ref quantization_hint) = search_params.quantization_hint {
            match &quantization_hint.level_type {
                Some(crate::compute::QuantizationLevelType::Pq(pq)) => {
                    match pq.bits_per_code {
                        8 => "vector_pq8",
                        4 => "vector_pq4",
                        _ => "vector_pq",
                    }
                }
                Some(crate::compute::QuantizationLevelType::Binary(_)) => "vector_binary",
                Some(crate::compute::QuantizationLevelType::Scalar(s)) if s.bits == 8 => "vector_int8",
                _ => "vector", // Fallback to FP32
            }
        } else {
            // Default to PQ8 if available (most common)
            "vector_pq8"
        };
        projected_columns.push(quantized_column);
        
        // Add metadata filter columns if present
        if let Some(filters) = &search_params.filters {
            for key in filters.keys() {
                if !projected_columns.contains(&key.as_str()) {
                    projected_columns.push(key);
                }
            }
        }
        
        // Get metadata to track row groups
        let file_metadata = file_reader.metadata();
        let num_row_groups = file_metadata.num_row_groups();
        
        let mut candidates = Vec::new();
        
        // Quantize query vector for comparison
        // For now, use PQ8 as default quantization level
        let quantization_level = UnifiedQuantizationLevel::pq8(8);
        let quantized_query = self.quantization_engine
            .quantize(query_vector, &quantization_level)
            .await?;
        
        // Process each row group separately for better tracking
        for row_group_idx in 0..num_row_groups {
            // Build reader for specific row group
            let row_group_reader = ParquetRecordBatchReaderBuilder::try_new(File::open(parquet_file_path)?)?
                .with_row_groups(vec![row_group_idx])
                .build()?;
            
            let mut row_offset = 0;
            
            // Process batches within this row group
            for batch_result in row_group_reader {
            let batch = batch_result?;
            
            // Get columns
            let id_array = batch.column_by_name("id")
                .ok_or_else(|| anyhow!("Missing 'id' column"))?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow!("'id' column is not String type"))?;
                
            let version_array = batch.column_by_name("version")
                .ok_or_else(|| anyhow!("Missing 'version' column"))?
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| anyhow!("'version' column is not Int64 type"))?;
            
            // Get quantized vector column
            let quantized_vectors = batch.column_by_name(quantized_column)
                .ok_or_else(|| anyhow!("Missing quantized column '{}'", quantized_column))?;
            
            // Process based on quantization type (infer from column name)
            let distances = match quantized_column {
                "vector_pq8" | "vector_pq4" => {
                    // PQ vectors are stored as List<UInt8>
                    let list_array = quantized_vectors.as_any()
                        .downcast_ref::<ListArray>()
                        .ok_or_else(|| anyhow!("PQ column is not List type"))?;
                    
                    self.calculate_pq_distances(list_array, &quantized_query, distance_metric)?
                }
                "vector_binary" => {
                    // Binary vectors stored as List<UInt8> (packed bits)
                    let list_array = quantized_vectors.as_any()
                        .downcast_ref::<ListArray>()
                        .ok_or_else(|| anyhow!("Binary column is not List type"))?;
                    
                    self.calculate_binary_distances(list_array, &quantized_query, distance_metric)?
                }
                "vector_int8" => {
                    // INT8 vectors stored as List<Int8>
                    let list_array = quantized_vectors.as_any()
                        .downcast_ref::<ListArray>()
                        .ok_or_else(|| anyhow!("INT8 column is not List type"))?;
                    
                    // Quantize query vector to INT8 for comparison
                    let int8_query = self.quantize_to_int8(query_vector);
                    self.calculate_int8_distances(list_array, &int8_query, distance_metric)?
                }
                _ => {
                    // Fallback to FP32
                    let list_array = quantized_vectors.as_any()
                        .downcast_ref::<ListArray>()
                        .ok_or_else(|| anyhow!("Vector column is not List type"))?;
                    
                    self.calculate_fp32_distances(list_array, query_vector, distance_metric)?
                }
            };
            
            // Apply metadata filters if present
            let mut valid_rows = vec![true; batch.num_rows()];
            if let Some(filters) = &search_params.filters {
                self.apply_metadata_filters(&batch, filters, &mut valid_rows)?;
            }
            
            // Collect candidates with distances
            for (row_idx, &distance) in distances.iter().enumerate() {
                if !valid_rows[row_idx] || !id_array.is_valid(row_idx) {
                    continue;
                }
                
                candidates.push(CandidateResult {
                    id: id_array.value(row_idx).to_string(),
                    approx_distance: distance,
                    quantization_level: quantization_level.clone(),
                    location: VectorLocation {
                        file_path: parquet_file_path.to_string(),
                        row_group: row_group_idx,
                        row_index: row_offset + row_idx,
                    },
                });
            }
            
            // Update row offset for next batch
            row_offset += batch.num_rows();
        }
        }
        
        // Sort by distance and take top candidates
        candidates.sort_by(|a, b| a.approx_distance.partial_cmp(&b.approx_distance)
            .unwrap_or(Ordering::Equal));
        candidates.truncate(num_candidates);
        
        debug!("🔍 Stage 1: Found {} candidates from {}", candidates.len(), parquet_file_path);
        Ok(candidates)
    }
    
    /// Retrieve FP32 vectors for given candidates
    async fn retrieve_fp32_vectors(
        &self,
        viper_engine: &ViperEngine,
        file_path: &str,
        candidates: &[CandidateResult],
    ) -> Result<Vec<Vec<f32>>> {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
        use std::fs::File;
        
        debug!("🔍 Stage 2: Retrieving {} FP32 vectors from {}", candidates.len(), file_path);
        
        if candidates.is_empty() {
            return Ok(vec![]);
        }
        
        // Open Parquet file
        let file = File::open(file_path)
            .context(format!("Failed to open Parquet file: {}", file_path))?;
        
        // Group candidates by row group for efficient reading
        let mut candidates_by_row_group: HashMap<usize, Vec<(usize, &CandidateResult)>> = HashMap::new();
        for (idx, candidate) in candidates.iter().enumerate() {
            candidates_by_row_group
                .entry(candidate.location.row_group)
                .or_insert_with(Vec::new)
                .push((idx, candidate));
        }
        
        let mut fp32_vectors = vec![Vec::new(); candidates.len()];
        
        // Process each row group that contains candidates
        for (row_group_idx, group_candidates) in candidates_by_row_group {
            debug!("🔍 Stage 2: Reading row group {} with {} candidates", row_group_idx, group_candidates.len());
            
            // Build reader for specific row group
            let row_group_reader = ParquetRecordBatchReaderBuilder::try_new(File::open(file_path)?)?
                .with_row_groups(vec![row_group_idx])
                .build()?;
            
            // Create lookup for this row group
            let mut row_to_candidate: HashMap<usize, usize> = HashMap::new();
            for (candidate_idx, candidate) in &group_candidates {
                row_to_candidate.insert(candidate.location.row_index, *candidate_idx);
            }
            
            let mut current_row = 0;
            
            // Process batches within this row group
            for batch_result in row_group_reader {
            let batch = batch_result?;
            
            // Get the vector column
            let vector_array = batch.column_by_name("vector")
                .ok_or_else(|| anyhow!("Missing 'vector' column"))?
                .as_any()
                .downcast_ref::<ListArray>()
                .ok_or_else(|| anyhow!("'vector' column is not List type"))?;
            
            // Extract vectors for our candidates
            for row_idx in 0..batch.num_rows() {
                let global_row_idx = current_row + row_idx;
                
                if let Some(&candidate_idx) = row_to_candidate.get(&global_row_idx) {
                    if vector_array.is_valid(row_idx) {
                        // Extract the float array from the list
                        let value_array = vector_array.value(row_idx);
                        let float_array = value_array.as_any()
                            .downcast_ref::<Float32Array>()
                            .ok_or_else(|| anyhow!("Vector values are not Float32 type"))?;
                        
                        // Convert to Vec<f32>
                        let vector: Vec<f32> = (0..float_array.len())
                            .map(|i| float_array.value(i))
                            .collect();
                        
                        fp32_vectors[candidate_idx] = vector;
                    }
                }
            }
            
            current_row += batch.num_rows();
        }
        }
        
        // Verify we got all vectors
        for (idx, vec) in fp32_vectors.iter().enumerate() {
            if vec.is_empty() {
                warn!("🔍 Stage 2: Failed to retrieve vector for candidate {}", candidates[idx].id);
            }
        }
        
        debug!("🔍 Stage 2: Retrieved {} FP32 vectors", fp32_vectors.len());
        Ok(fp32_vectors)
    }
    
    /// Get the configuration for testing purposes
    pub fn config(&self) -> &TwoStageSearchConfig {
        &self.config
    }
    
    /// Calculate number of candidates based on k and configuration
    pub fn calculate_candidate_count(&self, k: usize) -> usize {
        let calculated = (k as f32 * self.config.candidate_multiplier) as usize;
        calculated
            .max(self.config.min_candidates)
            .min(self.config.max_candidates)
    }
    
    /// Calculate distances for Product Quantization vectors
    fn calculate_pq_distances(
        &self,
        vectors: &ListArray,
        quantized_query: &ComputeQuantizedVector,
        distance_metric: &DistanceMetric,
    ) -> Result<Vec<f32>> {
        let mut distances = Vec::with_capacity(vectors.len());
        
        for i in 0..vectors.len() {
            if vectors.is_valid(i) {
                let vector_bytes = vectors.value(i);
                let uint8_array = vector_bytes.as_any()
                    .downcast_ref::<UInt8Array>()
                    .ok_or_else(|| anyhow!("PQ vector values are not UInt8 type"))?;
                
                // Convert to Vec<u8> for quantized distance calculation
                let pq_codes: Vec<u8> = (0..uint8_array.len())
                    .map(|j| uint8_array.value(j))
                    .collect();
                
                // Calculate distance between quantized vectors using proper PQ distance
                // Extract num_subvectors from quantization level
                let num_subvectors = match &quantized_query.quantization_level.level_type {
                    Some(crate::compute::unified_quantization::QuantizationLevelType::Pq(pq)) => {
                        pq.num_subvectors as usize
                    }
                    _ => 8, // Default to 8 if not PQ or not specified
                };
                
                let distance = self.quantization_engine.calculate_pq_distance(
                    &quantized_query.data,
                    &pq_codes,
                    distance_metric,
                    num_subvectors,
                );
                
                distances.push(distance);
            } else {
                distances.push(f32::MAX);
            }
        }
        
        Ok(distances)
    }
    
    /// Calculate distances for Binary quantized vectors
    fn calculate_binary_distances(
        &self,
        vectors: &ListArray,
        quantized_query: &ComputeQuantizedVector,
        distance_metric: &DistanceMetric,
    ) -> Result<Vec<f32>> {
        let mut distances = Vec::with_capacity(vectors.len());
        
        for i in 0..vectors.len() {
            if vectors.is_valid(i) {
                let vector_bytes = vectors.value(i);
                let uint8_array = vector_bytes.as_any()
                    .downcast_ref::<UInt8Array>()
                    .ok_or_else(|| anyhow!("Binary vector values are not UInt8 type"))?;
                
                // Convert to Vec<u8> for binary distance calculation
                let binary_data: Vec<u8> = (0..uint8_array.len())
                    .map(|j| uint8_array.value(j))
                    .collect();
                
                // Calculate Hamming distance for binary vectors
                let distance = self.quantization_engine.calculate_hamming_distance(
                    &quantized_query.data,
                    &binary_data
                ) as f32;
                
                distances.push(distance);
            } else {
                distances.push(f32::MAX);
            }
        }
        
        Ok(distances)
    }
    
    /// Calculate distances for FP32 vectors
    fn calculate_fp32_distances(
        &self,
        vectors: &ListArray,
        query_vector: &[f32],
        distance_metric: &DistanceMetric,
    ) -> Result<Vec<f32>> {
        let mut distances = Vec::with_capacity(vectors.len());
        
        for i in 0..vectors.len() {
            if vectors.is_valid(i) {
                let value_array = vectors.value(i);
                let float_array = value_array.as_any()
                    .downcast_ref::<Float32Array>()
                    .ok_or_else(|| anyhow!("Vector values are not Float32 type"))?;
                
                // Convert to Vec<f32>
                let vector: Vec<f32> = (0..float_array.len())
                    .map(|j| float_array.value(j))
                    .collect();
                
                // Calculate distance
                let distance = self.distance_compute.calculate_distance(
                    query_vector,
                    &vector,
                    distance_metric,
                );
                
                distances.push(distance);
            } else {
                distances.push(f32::MAX);
            }
        }
        
        Ok(distances)
    }
    
    /// Quantize FP32 vector to INT8
    fn quantize_to_int8(&self, vector: &[f32]) -> Vec<i8> {
        // Find min and max for scaling
        let min = vector.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let max = vector.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        let range = max - min;
        
        if range == 0.0 {
            return vec![0i8; vector.len()];
        }
        
        // Scale to INT8 range [-127, 127]
        vector.iter()
            .map(|&v| {
                let normalized = (v - min) / range;
                let scaled = (normalized * 255.0 - 128.0).round();
                scaled.clamp(-127.0, 127.0) as i8
            })
            .collect()
    }
    
    /// Calculate distances for INT8 quantized vectors using unified quantization engine
    fn calculate_int8_distances(
        &self,
        list_array: &ListArray,
        query: &[i8],
        distance_metric: &DistanceMetric,
    ) -> Result<Vec<f32>> {
        use arrow_array::cast::AsArray;
        use crate::compute::unified_quantization::{QuantizedVector, UnifiedQuantizationLevel, QuantizationMetadata};
        
        let mut distances = Vec::with_capacity(list_array.len());
        
        // Create quantized query vector
        let query_quantized = QuantizedVector {
            data: query.iter().map(|&v| v as u8).collect(),
            quantization_level: UnifiedQuantizationLevel::int8(),
            metadata: QuantizationMetadata::default(),
        };
        
        for i in 0..list_array.len() {
            if list_array.is_null(i) {
                distances.push(f32::INFINITY);
                continue;
            }
            
            let values = list_array.value(i);
            let int8_array = values.as_primitive::<arrow_array::types::Int8Type>();
            
            if int8_array.len() != query.len() {
                distances.push(f32::INFINITY);
                continue;
            }
            
            // Create quantized data vector
            let data_quantized = QuantizedVector {
                data: (0..int8_array.len())
                    .map(|j| int8_array.value(j) as u8)
                    .collect(),
                quantization_level: UnifiedQuantizationLevel::int8(),
                metadata: QuantizationMetadata::default(),
            };
            
            // Use unified quantization engine for distance calculation
            let distance = self.quantization_engine.calculate_quantized_distance(
                &query_quantized,
                &data_quantized,
                distance_metric,
            );
            
            distances.push(distance);
        }
        
        Ok(distances)
    }
    
    /// Test helper method for refining candidates with FP32 vectors
    pub async fn refine_with_fp32(
        &self,
        viper_engine: &ViperEngine,
        query_vector: &[f32],
        candidates: Vec<CandidateResult>,
        search_params: &SearchParams,
        distance_metric: &DistanceMetric,
    ) -> Result<Vec<SearchResult>> {
        self.stage2_fp32_refinement(viper_engine, query_vector, candidates, search_params.top_k.unwrap_or(10), distance_metric).await
    }
    
    /// Apply metadata filters to rows
    fn apply_metadata_filters(
        &self,
        batch: &arrow_array::RecordBatch,
        filters: &HashMap<String, serde_json::Value>,
        valid_rows: &mut [bool],
    ) -> Result<()> {
        // Arrow types are already imported at the top
        
        for (key, expected_value) in filters {
            if let Some(column) = batch.column_by_name(key) {
                match expected_value {
                    serde_json::Value::String(expected_str) => {
                        if let Some(str_array) = column.as_any().downcast_ref::<StringArray>() {
                            for (idx, valid) in valid_rows.iter_mut().enumerate() {
                                if *valid && str_array.is_valid(idx) {
                                    *valid = str_array.value(idx) == expected_str;
                                }
                            }
                        }
                    }
                    serde_json::Value::Number(expected_num) => {
                        if let Some(expected_i64) = expected_num.as_i64() {
                            if let Some(int_array) = column.as_any().downcast_ref::<Int64Array>() {
                                for (idx, valid) in valid_rows.iter_mut().enumerate() {
                                    if *valid && int_array.is_valid(idx) {
                                        *valid = int_array.value(idx) == expected_i64;
                                    }
                                }
                            }
                        } else if let Some(expected_f64) = expected_num.as_f64() {
                            if let Some(float_array) = column.as_any().downcast_ref::<Float64Array>() {
                                for (idx, valid) in valid_rows.iter_mut().enumerate() {
                                    if *valid && float_array.is_valid(idx) {
                                        *valid = (float_array.value(idx) - expected_f64).abs() < f64::EPSILON;
                                    }
                                }
                            }
                        }
                    }
                    serde_json::Value::Bool(expected_bool) => {
                        if let Some(bool_array) = column.as_any().downcast_ref::<BooleanArray>() {
                            for (idx, valid) in valid_rows.iter_mut().enumerate() {
                                if *valid && bool_array.is_valid(idx) {
                                    *valid = bool_array.value(idx) == *expected_bool;
                                }
                            }
                        }
                    }
                    _ => {} // Skip complex filter types for now
                }
            }
        }
        
        Ok(())
    }
    
    /// Log accuracy statistics for monitoring
    fn log_accuracy_stats(&self, candidates: &[CandidateResult], final_results: &[SearchResult]) {
        if candidates.is_empty() || final_results.is_empty() {
            return;
        }
        
        // Calculate how many of the final results were in the top candidates
        let final_ids: HashSet<_> = final_results.iter()
            .map(|r| &r.id)
            .collect();
            
        let candidate_ids: HashSet<_> = candidates.iter()
            .take(final_results.len())
            .map(|c| &c.id)
            .collect();
            
        let overlap = final_ids.intersection(&candidate_ids).count();
        let recall = overlap as f32 / final_results.len() as f32;
        
        info!("📊 Two-stage accuracy: recall={:.2}% ({}/{} found in candidates)", 
              recall * 100.0, overlap, final_results.len());
        
        // Log distance correlation
        if let (Some(first_candidate), Some(first_result)) = (candidates.first(), final_results.first()) {
            debug!("📊 Distance correlation: approx={:.4} vs exact={:.4}", 
                   first_candidate.approx_distance, first_result.score);
        }
    }
}

/// Builder for two-stage search configuration
pub struct TwoStageSearchBuilder {
    config: TwoStageSearchConfig,
}

impl TwoStageSearchBuilder {
    pub fn new() -> Self {
        Self {
            config: TwoStageSearchConfig::default(),
        }
    }
    
    pub fn candidate_multiplier(mut self, multiplier: f32) -> Self {
        self.config.candidate_multiplier = multiplier;
        self
    }
    
    pub fn min_candidates(mut self, min: usize) -> Self {
        self.config.min_candidates = min;
        self
    }
    
    pub fn max_candidates(mut self, max: usize) -> Self {
        self.config.max_candidates = max;
        self
    }
    
    pub fn enable_parallel(mut self, enable: bool) -> Self {
        self.config.enable_parallel = enable;
        self
    }
    
    pub fn build(
        self,
        distance_compute: Arc<UnifiedDistanceCompute>,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
    ) -> TwoStageSearchEngine {
        TwoStageSearchEngine::new(distance_compute, quantization_engine, self.config)
    }
}
