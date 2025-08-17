// Progressive columnar search implementation for optimized NOVA engine
// Multi-stage search pipeline: Binary → INT8 → PQ → FP32 with streaming support

use anyhow::{anyhow, Result};
use arrow_array::RecordBatch;
use std::collections::BinaryHeap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, instrument, warn};

use crate::core::VectorRecord;
use crate::compute::distance_computation::DistanceMetric;
use super::hierarchical_stats::{SuperBlock, EnhancedRowGroupStats, ZoneMap};
use super::streaming_processor::{
    StreamingRowGroupProcessor, StreamingContext, RowGroupProcessingResult,
    RowGroupCandidate, ProcessingStage, StreamingConfig,
};
use super::quantized_columns::{BinarySketch, Int8Vector, PQCode, DistanceTable};

/// Configuration for progressive columnar search
#[derive(Debug, Clone)]
pub struct ProgressiveSearchConfig {
    /// Stage configurations
    pub binary_config: StageConfig,
    pub int8_config: StageConfig,
    pub pq_config: StageConfig,
    pub full_precision_config: StageConfig,
    
    /// Streaming configuration
    pub streaming_config: StreamingConfig,
    
    /// Search optimization settings
    pub cost_based_ordering: bool,
    pub adaptive_thresholds: bool,
    pub enable_superblock_pruning: bool,
    
    /// Quality vs Performance trade-offs
    pub quality_target: f32,        // 0.0 (speed) to 1.0 (quality)
    pub latency_budget_ms: Option<u64>,
    pub memory_budget_bytes: Option<usize>,
}

/// Configuration for a single search stage
#[derive(Debug, Clone)]
pub struct StageConfig {
    /// Maximum candidates to pass to next stage
    pub max_candidates: usize,
    
    /// Distance threshold for filtering
    pub distance_threshold: Option<f32>,
    
    /// Memory limit for this stage
    pub memory_limit: usize,
    
    /// Enable parallel processing
    pub enable_parallelism: bool,
    
    /// Timeout for stage completion
    pub timeout_ms: u64,
}

/// Result of progressive search
#[derive(Debug)]
pub struct ProgressiveSearchResult {
    /// Final results
    pub results: Vec<VectorRecord>,
    
    /// Performance metrics per stage
    pub stage_metrics: Vec<StageMetrics>,
    
    /// Overall search metrics
    pub total_time_ms: u64,
    pub total_candidates_processed: usize,
    pub total_candidates_filtered: usize,
    pub memory_peak_usage: usize,
    pub row_groups_scanned: usize,
    pub superblocks_pruned: usize,
}

/// Metrics for a single search stage
#[derive(Debug, Clone)]
pub struct StageMetrics {
    pub stage: ProcessingStage,
    pub duration_ms: u64,
    pub candidates_in: usize,
    pub candidates_out: usize,
    pub memory_used: usize,
    pub row_groups_processed: usize,
    pub effectiveness: f32, // Filtering effectiveness (0.0-1.0)
}

/// Progressive search candidate with stage information
#[derive(Debug, Clone)]
pub struct ProgressiveCandidate {
    pub row_group_id: u32,
    pub row_offset: u32,
    pub similarity: f32,
    pub stage: ProcessingStage,
    pub vector_id: Option<String>,
    pub record: Option<VectorRecord>,
}

impl PartialEq for ProgressiveCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance
    }
}

impl Eq for ProgressiveCandidate {}

impl PartialOrd for ProgressiveCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // Reverse for min-heap (best candidates first)
        other.distance.partial_cmp(&self.distance)
    }
}

impl Ord for ProgressiveCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// Main progressive columnar search engine
pub struct ProgressiveColumnarSearch {
    config: ProgressiveSearchConfig,
    streaming_processor: StreamingRowGroupProcessor,
    distance_metric: DistanceMetric,
}

impl ProgressiveColumnarSearch {
    /// Create a new progressive search engine
    pub fn new(config: ProgressiveSearchConfig, distance_metric: DistanceMetric) -> Self {
        let streaming_processor = StreamingRowGroupProcessor::new(config.streaming_config.clone());
        
        Self {
            config,
            streaming_processor,
            distance_metric,
        }
    }
    
    /// Execute progressive search with streaming optimization
    #[instrument(skip(self, query_vector, superblocks, enhanced_stats))]
    pub async fn search_progressive(
        &self,
        query_vector: &[f32],
        top_k: usize,
        superblocks: &[SuperBlock],
        enhanced_stats: &[EnhancedRowGroupStats],
        parquet_metadata: &parquet::file::metadata::ParquetMetaData,
    ) -> Result<ProgressiveSearchResult> {
        let start_time = std::time::Instant::now();
        info!(
            "Starting progressive search: query_dim={}, top_k={}, superblocks={}, row_groups={}",
            query_vector.len(),
            top_k,
            superblocks.len(),
            enhanced_stats.len()
        );
        
        let mut stage_metrics = Vec::new();
        let mut total_candidates_processed = 0;
        let mut total_candidates_filtered = 0;
        let mut memory_peak_usage = 0;
        let mut superblocks_pruned = 0;
        
        // Phase 1: SuperBlock pruning
        let relevant_superblocks = if self.config.enable_superblock_pruning {
            self.prune_superblocks(query_vector, superblocks, &mut superblocks_pruned).await?
        } else {
            superblocks.to_vec()
        };
        
        info!("SuperBlock pruning: {} → {} superblocks", superblocks.len(), relevant_superblocks.len());
        
        // Phase 2: Row group ordering and streaming processing
        let streaming_context = StreamingContext {
            query_vector: query_vector.to_vec(),
            top_k,
            distance_threshold: None,
            superblocks: relevant_superblocks,
            enhanced_stats: enhanced_stats.to_vec(),
        };
        
        let row_group_results = self.streaming_processor
            .process_row_groups_streaming(streaming_context, parquet_metadata)
            .await?;
        
        // Phase 3: Progressive refinement stages
        let mut candidates = self.collect_initial_candidates(&row_group_results).await?;
        
        // Binary stage
        let (candidates, binary_metrics) = self.execute_binary_stage(
            query_vector,
            candidates,
            &mut total_candidates_processed,
            &mut total_candidates_filtered,
            &mut memory_peak_usage,
        ).await?;
        stage_metrics.push(binary_metrics);
        
        if candidates.is_empty() {
            return Ok(self.build_empty_result(stage_metrics, start_time));
        }
        
        // INT8 stage
        let (candidates, int8_metrics) = self.execute_int8_stage(
            query_vector,
            candidates,
            &mut total_candidates_processed,
            &mut total_candidates_filtered,
            &mut memory_peak_usage,
        ).await?;
        stage_metrics.push(int8_metrics);
        
        if candidates.is_empty() {
            return Ok(self.build_empty_result(stage_metrics, start_time));
        }
        
        // PQ stage
        let (candidates, pq_metrics) = self.execute_pq_stage(
            query_vector,
            candidates,
            &mut total_candidates_processed,
            &mut total_candidates_filtered,
            &mut memory_peak_usage,
        ).await?;
        stage_metrics.push(pq_metrics);
        
        if candidates.is_empty() {
            return Ok(self.build_empty_result(stage_metrics, start_time));
        }
        
        // Full precision stage
        let (final_results, fp_metrics) = self.execute_full_precision_stage(
            query_vector,
            candidates,
            top_k,
            &mut total_candidates_processed,
            &mut total_candidates_filtered,
            &mut memory_peak_usage,
        ).await?;
        stage_metrics.push(fp_metrics);
        
        let total_time_ms = start_time.elapsed().as_millis() as u64;
        
        info!(
            "Progressive search completed: {} results in {}ms, processed {} candidates",
            final_results.len(),
            total_time_ms,
            total_candidates_processed
        );
        
        Ok(ProgressiveSearchResult {
            results: final_results,
            stage_metrics,
            total_time_ms,
            total_candidates_processed,
            total_candidates_filtered,
            memory_peak_usage,
            row_groups_scanned: row_group_results.len(),
            superblocks_pruned,
        })
    }
    
    /// Prune SuperBlocks based on query
    async fn prune_superblocks(
        &self,
        query_vector: &[f32],
        superblocks: &[SuperBlock],
        pruned_count: &mut usize,
    ) -> Result<Vec<SuperBlock>> {
        let mut relevant_blocks = Vec::new();
        
        for superblock in superblocks {
            // Use zone map intersection for pruning
            if superblock.can_contain_candidates(
                query_vector,
                self.distance_metric,
                f32::INFINITY, // For now, don't use distance threshold
            ) {
                relevant_blocks.push(superblock.clone());
            } else {
                *pruned_count += 1;
            }
        }
        
        // Sort by estimated search cost
        relevant_blocks.sort_by(|a, b| {
            a.selectivity_hints.search_cost_estimate
                .partial_cmp(&b.selectivity_hints.search_cost_estimate)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        
        Ok(relevant_blocks)
    }
    
    /// Collect initial candidates from row group processing results
    async fn collect_initial_candidates(
        &self,
        row_group_results: &[RowGroupProcessingResult],
    ) -> Result<Vec<ProgressiveCandidate>> {
        let mut candidates = Vec::new();
        
        for result in row_group_results {
            for rg_candidate in &result.candidates {
                candidates.push(ProgressiveCandidate {
                    row_group_id: rg_candidate.row_group_id,
                    row_offset: rg_candidate.row_offset,
                    similarity: rg_candidate.distance,
                    // confidence removed -  0.3, // Low confidence from initial processing
                    stage: ProcessingStage::BloomFilter,
                    vector_id: rg_candidate.vector_id.clone(),
                    record: rg_candidate.record.clone(),
                });
            }
        }
        
        Ok(candidates)
    }
    
    /// Execute binary quantization stage
    async fn execute_binary_stage(
        &self,
        query_vector: &[f32],
        candidates: Vec<ProgressiveCandidate>,
        total_processed: &mut usize,
        total_filtered: &mut usize,
        peak_memory: &mut usize,
    ) -> Result<(Vec<ProgressiveCandidate>, StageMetrics)> {
        let stage_start = std::time::Instant::now();
        let candidates_in = candidates.len();
        *total_processed += candidates_in;
        
        debug!("Binary stage: processing {} candidates", candidates_in);
        
        // Convert query to binary sketch
        let binary_query = BinarySketch::from_vector(query_vector, 0.0);
        
        // Process candidates through binary filtering
        let mut filtered_candidates = Vec::new();
        let max_candidates = self.config.binary_config.max_candidates;
        let mut candidate_heap = BinaryHeap::new();
        
        for candidate in candidates {
            // Simulate loading binary sketch for this candidate
            let binary_sketch = self.load_binary_sketch(
                candidate.row_group_id,
                candidate.row_offset,
            ).await?;
            
            // Compute binary distance
            let binary_distance = binary_query.hamming_distance(&binary_sketch) as f32;
            
            // Check threshold
            if let Some(threshold) = self.config.binary_config.distance_threshold {
                if binary_distance > threshold {
                    continue;
                }
            }
            
            let updated_candidate = ProgressiveCandidate {
                similarity: binary_distance,
                // confidence removed -  0.5,
                stage: ProcessingStage::BinaryFilter,
                ..candidate
            };
            
            candidate_heap.push(updated_candidate);
            
            // Keep only top candidates
            if candidate_heap.len() > max_candidates {
                candidate_heap.pop();
            }
        }
        
        // Convert heap to vector
        while let Some(candidate) = candidate_heap.pop() {
            filtered_candidates.push(candidate);
        }
        filtered_candidates.reverse(); // Best candidates first
        
        let candidates_out = filtered_candidates.len();
        let filtered = candidates_in - candidates_out;
        *total_filtered += filtered;
        
        let duration_ms = stage_start.elapsed().as_millis() as u64;
        let effectiveness = if candidates_in > 0 {
            filtered as f32 / candidates_in as f32
        } else {
            0.0
        };
        
        debug!(
            "Binary stage completed: {} → {} candidates ({}% filtered) in {}ms",
            candidates_in,
            candidates_out,
            (effectiveness * 100.0) as u32,
            duration_ms
        );
        
        let metrics = StageMetrics {
            stage: ProcessingStage::BinaryFilter,
            duration_ms,
            candidates_in,
            candidates_out,
            memory_used: candidates_in * 96, // Approximate binary sketch size
            row_groups_processed: 0, // Calculated elsewhere
            effectiveness,
        };
        
        Ok((filtered_candidates, metrics))
    }
    
    /// Execute INT8 quantization stage
    async fn execute_int8_stage(
        &self,
        query_vector: &[f32],
        candidates: Vec<ProgressiveCandidate>,
        total_processed: &mut usize,
        total_filtered: &mut usize,
        peak_memory: &mut usize,
    ) -> Result<(Vec<ProgressiveCandidate>, StageMetrics)> {
        let stage_start = std::time::Instant::now();
        let candidates_in = candidates.len();
        *total_processed += candidates_in;
        
        debug!("INT8 stage: processing {} candidates", candidates_in);
        
        // Convert query to INT8
        let int8_query = Int8Vector::from_vector(query_vector);
        
        let mut filtered_candidates = Vec::new();
        let max_candidates = self.config.int8_config.max_candidates;
        let mut candidate_heap = BinaryHeap::new();
        
        for candidate in candidates {
            // Simulate loading INT8 vector for this candidate
            let int8_vector = self.load_int8_vector(
                candidate.row_group_id,
                candidate.row_offset,
            ).await?;
            
            // Compute INT8 distance
            let int8_distance = int8_query.l2_distance_squared(&int8_vector);
            
            // Check threshold
            if let Some(threshold) = self.config.int8_config.distance_threshold {
                if int8_distance > threshold {
                    continue;
                }
            }
            
            let updated_candidate = ProgressiveCandidate {
                similarity: int8_distance,
                // confidence removed -  0.7,
                stage: ProcessingStage::Int8Filter,
                ..candidate
            };
            
            candidate_heap.push(updated_candidate);
            
            if candidate_heap.len() > max_candidates {
                candidate_heap.pop();
            }
        }
        
        while let Some(candidate) = candidate_heap.pop() {
            filtered_candidates.push(candidate);
        }
        filtered_candidates.reverse();
        
        let candidates_out = filtered_candidates.len();
        let filtered = candidates_in - candidates_out;
        *total_filtered += filtered;
        
        let duration_ms = stage_start.elapsed().as_millis() as u64;
        let effectiveness = if candidates_in > 0 {
            filtered as f32 / candidates_in as f32
        } else {
            0.0
        };
        
        debug!(
            "INT8 stage completed: {} → {} candidates ({}% filtered) in {}ms",
            candidates_in,
            candidates_out,
            (effectiveness * 100.0) as u32,
            duration_ms
        );
        
        let metrics = StageMetrics {
            stage: ProcessingStage::Int8Filter,
            duration_ms,
            candidates_in,
            candidates_out,
            memory_used: candidates_in * query_vector.len(), // INT8 vector size
            row_groups_processed: 0,
            effectiveness,
        };
        
        Ok((filtered_candidates, metrics))
    }
    
    /// Execute PQ stage
    async fn execute_pq_stage(
        &self,
        query_vector: &[f32],
        candidates: Vec<ProgressiveCandidate>,
        total_processed: &mut usize,
        total_filtered: &mut usize,
        peak_memory: &mut usize,
    ) -> Result<(Vec<ProgressiveCandidate>, StageMetrics)> {
        let stage_start = std::time::Instant::now();
        let candidates_in = candidates.len();
        *total_processed += candidates_in;
        
        debug!("PQ stage: processing {} candidates", candidates_in);
        
        // Simulate PQ distance table computation
        let distance_table = DistanceTable::compute_for_query(query_vector, 32, 8)?;
        
        let mut filtered_candidates = Vec::new();
        let max_candidates = self.config.pq_config.max_candidates;
        let mut candidate_heap = BinaryHeap::new();
        
        for candidate in candidates {
            // Simulate loading PQ code for this candidate
            let pq_code = self.load_pq_code(
                candidate.row_group_id,
                candidate.row_offset,
            ).await?;
            
            // Compute PQ distance using distance table
            let pq_distance = distance_table.lookup_distance(&pq_code);
            
            // Check threshold
            if let Some(threshold) = self.config.pq_config.distance_threshold {
                if pq_distance > threshold {
                    continue;
                }
            }
            
            let updated_candidate = ProgressiveCandidate {
                similarity: pq_distance,
                // confidence removed -  0.9,
                stage: ProcessingStage::PQFilter,
                ..candidate
            };
            
            candidate_heap.push(updated_candidate);
            
            if candidate_heap.len() > max_candidates {
                candidate_heap.pop();
            }
        }
        
        while let Some(candidate) = candidate_heap.pop() {
            filtered_candidates.push(candidate);
        }
        filtered_candidates.reverse();
        
        let candidates_out = filtered_candidates.len();
        let filtered = candidates_in - candidates_out;
        *total_filtered += filtered;
        
        let duration_ms = stage_start.elapsed().as_millis() as u64;
        let effectiveness = if candidates_in > 0 {
            filtered as f32 / candidates_in as f32
        } else {
            0.0
        };
        
        debug!(
            "PQ stage completed: {} → {} candidates ({}% filtered) in {}ms",
            candidates_in,
            candidates_out,
            (effectiveness * 100.0) as u32,
            duration_ms
        );
        
        let metrics = StageMetrics {
            stage: ProcessingStage::PQFilter,
            duration_ms,
            candidates_in,
            candidates_out,
            memory_used: candidates_in * 32, // PQ code size
            row_groups_processed: 0,
            effectiveness,
        };
        
        Ok((filtered_candidates, metrics))
    }
    
    /// Execute full precision stage
    async fn execute_full_precision_stage(
        &self,
        query_vector: &[f32],
        candidates: Vec<ProgressiveCandidate>,
        top_k: usize,
        total_processed: &mut usize,
        total_filtered: &mut usize,
        peak_memory: &mut usize,
    ) -> Result<(Vec<VectorRecord>, StageMetrics)> {
        let stage_start = std::time::Instant::now();
        let candidates_in = candidates.len();
        *total_processed += candidates_in;
        
        debug!("Full precision stage: processing {} candidates", candidates_in);
        
        let mut final_candidates = Vec::new();
        
        for candidate in candidates {
            // Load full precision vector
            let full_vector = self.load_full_vector(
                candidate.row_group_id,
                candidate.row_offset,
            ).await?;
            
            // Compute exact distance
            let exact_distance = self.compute_exact_distance(query_vector, &full_vector);
            
            let record = VectorRecord {
                id: candidate.vector_id,
                vector: full_vector,
                metadata: None,
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
            };
            
            final_candidates.push((record, exact_distance));
        }
        
        // Sort by exact distance and take top-k
        final_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        final_candidates.truncate(top_k);
        
        let results: Vec<VectorRecord> = final_candidates.into_iter()
            .map(|(record, _)| record)
            .collect();
        
        let candidates_out = results.len();
        let filtered = candidates_in - candidates_out;
        *total_filtered += filtered;
        
        let duration_ms = stage_start.elapsed().as_millis() as u64;
        let effectiveness = if candidates_in > 0 {
            filtered as f32 / candidates_in as f32
        } else {
            0.0
        };
        
        debug!(
            "Full precision stage completed: {} → {} results in {}ms",
            candidates_in,
            candidates_out,
            duration_ms
        );
        
        let metrics = StageMetrics {
            stage: ProcessingStage::FullPrecision,
            duration_ms,
            candidates_in,
            candidates_out,
            memory_used: candidates_in * query_vector.len() * 4, // Full vector size
            row_groups_processed: 0,
            effectiveness,
        };
        
        Ok((results, metrics))
    }
    
    // Helper methods for loading data (would be implemented with actual Parquet reading)
    
    async fn load_binary_sketch(&self, _row_group_id: u32, _row_offset: u32) -> Result<BinarySketch> {
        // Simulate loading binary sketch from Parquet
        Ok(BinarySketch {
            bits: vec![0u64; 12], // 768 bits / 64 = 12 words
            dimension: 768,
        })
    }
    
    async fn load_int8_vector(&self, _row_group_id: u32, _row_offset: u32) -> Result<Int8Vector> {
        // Simulate loading INT8 vector from Parquet
        Ok(Int8Vector {
            values: vec![0i8; 768],
            scale: 1.0,
            zero_point: 0,
        })
    }
    
    async fn load_pq_code(&self, _row_group_id: u32, _row_offset: u32) -> Result<PQCode> {
        // Simulate loading PQ code from Parquet
        Ok(PQCode {
            codes: vec![0u8; 32],
            n_subspaces: 32,
        })
    }
    
    async fn load_full_vector(&self, _row_group_id: u32, _row_offset: u32) -> Result<Vec<f32>> {
        // Simulate loading full vector from Parquet
        Ok(vec![0.0f32; 768])
    }
    
    fn compute_exact_distance(&self, query: &[f32], vector: &[f32]) -> f32 {
        match self.distance_metric {
            DistanceMetric::Euclidean => {
                query.iter()
                    .zip(vector.iter())
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f32>()
                    .sqrt()
            }
            DistanceMetric::Cosine => {
                let dot: f32 = query.iter().zip(vector.iter()).map(|(a, b)| a * b).sum();
                let norm_a: f32 = query.iter().map(|x| x.powi(2)).sum::<f32>().sqrt();
                let norm_b: f32 = vector.iter().map(|x| x.powi(2)).sum::<f32>().sqrt();
                1.0 - (dot / (norm_a * norm_b))
            }
            DistanceMetric::DotProduct => {
                -query.iter().zip(vector.iter()).map(|(a, b)| a * b).sum::<f32>()
            }
            _ => {
                // Fallback to Euclidean
                query.iter()
                    .zip(vector.iter())
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f32>()
                    .sqrt()
            }
        }
    }
    
    fn build_empty_result(
        &self,
        stage_metrics: Vec<StageMetrics>,
        start_time: std::time::Instant,
    ) -> ProgressiveSearchResult {
        ProgressiveSearchResult {
            results: Vec::new(),
            stage_metrics,
            total_time_ms: start_time.elapsed().as_millis() as u64,
            total_candidates_processed: 0,
            total_candidates_filtered: 0,
            memory_peak_usage: 0,
            row_groups_scanned: 0,
            superblocks_pruned: 0,
        }
    }
}

impl Default for ProgressiveSearchConfig {
    fn default() -> Self {
        Self {
            binary_config: StageConfig {
                max_candidates: 10000,
                distance_threshold: Some(100.0),
                memory_limit: 64 * 1024 * 1024, // 64MB
                enable_parallelism: true,
                timeout_ms: 5000,
            },
            int8_config: StageConfig {
                max_candidates: 1000,
                distance_threshold: Some(50.0),
                memory_limit: 32 * 1024 * 1024, // 32MB
                enable_parallelism: true,
                timeout_ms: 3000,
            },
            pq_config: StageConfig {
                max_candidates: 200,
                distance_threshold: Some(20.0),
                memory_limit: 16 * 1024 * 1024, // 16MB
                enable_parallelism: true,
                timeout_ms: 2000,
            },
            full_precision_config: StageConfig {
                max_candidates: 50,
                distance_threshold: None,
                memory_limit: 64 * 1024 * 1024, // 64MB
                enable_parallelism: true,
                timeout_ms: 10000,
            },
            streaming_config: StreamingConfig::default(),
            cost_based_ordering: true,
            adaptive_thresholds: true,
            enable_superblock_pruning: true,
            quality_target: 0.8,
            latency_budget_ms: Some(1000),
            memory_budget_bytes: Some(256 * 1024 * 1024), // 256MB
        }
    }
}

// Additional implementations for helper types
impl DistanceTable {
    fn compute_for_query(query: &[f32], segments: u8, bits: u8) -> Result<Self> {
        // Simulate distance table computation
        let centroids_per_segment = 1 << bits; // 2^bits
        let mut table = Vec::new();
        
        for _segment in 0..segments {
            let mut segment_distances = Vec::new();
            for _centroid in 0..centroids_per_segment {
                // Simulate distance computation
                segment_distances.push(rand::random::<f32>() * 10.0);
            }
            table.push(segment_distances);
        }
        
        Ok(Self { table })
    }
    
    fn lookup_distance(&self, pq_code: &PQCode) -> f32 {
        let mut total_distance = 0.0;
        
        for (segment_idx, &code) in pq_code.codes.iter().enumerate() {
            if segment_idx < self.table.len() && (code as usize) < self.table[segment_idx].len() {
                total_distance += self.table[segment_idx][code as usize];
            }
        }
        
        total_distance
    }
}

impl BinarySketch {
    fn from_vector(vector: &[f32], threshold: f32) -> Self {
        let dimension = vector.len();
        let num_words = (dimension + 63) / 64;
        let mut bits = vec![0u64; num_words];
        
        for (i, &value) in vector.iter().enumerate() {
            if value > threshold {
                let word_idx = i / 64;
                let bit_idx = i % 64;
                bits[word_idx] |= 1u64 << bit_idx;
            }
        }
        
        Self { bits, dimension }
    }
    
    fn hamming_distance(&self, other: &Self) -> u32 {
        self.bits.iter()
            .zip(other.bits.iter())
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }
}

impl Int8Vector {
    fn from_vector(vector: &[f32]) -> Self {
        // Find min and max for scaling
        let min_val = vector.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let max_val = vector.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        
        let scale = (max_val - min_val) / 255.0;
        let zero_point = (-min_val / scale).round() as i8;
        
        let values = vector.iter()
            .map(|&x| ((x / scale) + zero_point as f32).round().clamp(-128.0, 127.0) as i8)
            .collect();
        
        Self {
            values,
            scale,
            zero_point,
        }
    }
    
    fn l2_distance_squared(&self, other: &Self) -> f32 {
        self.values.iter()
            .zip(other.values.iter())
            .map(|(&a, &b)| {
                let diff = (a as f32 - b as f32) * self.scale;
                diff * diff
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_binary_sketch() {
        let vector = vec![0.5, -0.3, 0.8, -0.1, 0.0];
        let sketch = BinarySketch::from_vector(&vector, 0.0);
        
        assert_eq!(sketch.dimension, 5);
        // Bits should be set for positive values: 1, 0, 1, 0, 0
        // In first word: bit 0 and bit 2 should be set
        assert_eq!(sketch.bits[0] & 0b101, 0b101);
    }
    
    #[test]
    fn test_int8_vector() {
        let vector = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let int8_vec = Int8Vector::from_vector(&vector);
        
        assert_eq!(int8_vec.values.len(), 5);
        assert!(int8_vec.scale > 0.0);
    }
    
    #[test]
    fn test_progressive_candidate_ordering() {
        let mut heap = BinaryHeap::new();
        
        heap.push(ProgressiveCandidate {
            row_group_id: 0,
            row_offset: 0,
            similarity: 10.0,
            // confidence removed -  0.8,
            stage: ProcessingStage::BinaryFilter,
            vector_id: None,
            record: None,
        });
        
        heap.push(ProgressiveCandidate {
            row_group_id: 0,
            row_offset: 1,
            similarity: 5.0,
            // confidence removed -  0.8,
            stage: ProcessingStage::BinaryFilter,
            vector_id: None,
            record: None,
        });
        
        // Should pop smallest distance first (min-heap behavior)
        assert_eq!(heap.pop().unwrap().distance, 5.0);
        assert_eq!(heap.pop().unwrap().distance, 10.0);
    }
}