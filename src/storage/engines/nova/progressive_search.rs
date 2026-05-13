// Progressive columnar search implementation for optimized NOVA engine
// Multi-stage search pipeline: Binary → INT8 → PQ → FP32 with streaming support

use super::hierarchical_stats::{EnhancedRowGroupStats, SuperBlock};
use super::streaming_processor::{
    ProcessingStage, RowGroupProcessingResult, StreamingConfig, StreamingContext,
    StreamingRowGroupProcessor,
};
use crate::compute::distance_computation::{DistanceMetric, engine::UnifiedDistanceCompute};
use crate::compute::quantization::quantization_engine::UnifiedQuantizationEngine;
use crate::core::search::bounded_queue::BoundedPriorityQueue;
use crate::proto::proximadb_v1::VectorRecord;
use anyhow::Result;
use std::collections::BinaryHeap;
use std::sync::Arc;
use tracing::{debug, info, instrument};
// Import types from refactored quantized_columns module

// Create compatibility types for progressive search
/// Binary sketch representation for fast approximate distance computation
///
/// Stores a compressed binary representation of vectors for Hamming distance
/// calculations in the first stage of progressive search.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct BinarySketch {
    /// Bit-packed binary representation
    bits: Vec<u8>,
}

/// INT8 quantized vector representation
///
/// Stores vectors quantized to 8-bit integers with scale and zero point
/// for efficient distance computation with reduced memory footprint.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct Int8Vector {
    /// Quantized values
    pub(crate) values: Vec<i8>,
    /// Scale factor for dequantization
    pub(crate) scale: f32,
    /// Zero point for quantization
    zero_point: i8,
}

/// Product Quantization (PQ) code representation
///
/// Stores PQ codes for highly compressed vector representation,
/// enabling fast approximate distance computation via lookup tables.
#[derive(Debug, Clone)]
struct PQCode {
    /// PQ codes (one per segment)
    codes: Vec<u8>,
}

/// Distance lookup table for Product Quantization
///
/// Pre-computed distance table for efficient PQ distance computation.
/// Maps (segment, centroid) pairs to distance values.
#[derive(Debug, Clone)]
struct DistanceTable {
    /// 2D table: segment -> centroid -> distance
    table: Vec<Vec<f32>>,
}

/// Simple quantization adapter that wraps UnifiedQuantizationEngine
///
/// Provides the specific methods needed for progressive search, bridging
/// the unified quantization engine with NOVA's multi-stage search pipeline.
struct QuantizationAdapter {
    /// Underlying unified quantization engine
    engine: Arc<UnifiedQuantizationEngine>,
}

impl QuantizationAdapter {
    /// Quantize vector to binary format
    async fn quantize_to_binary(&self, vector: &[f32]) -> Result<Vec<u8>> {
        // Use the quantization engine's binary quantization method
        let binary = self.engine.quantize_to_binary(vector)?;
        Ok(binary)
    }

    /// Compute hamming distance between binary vectors
    async fn compute_hamming_distance(&self, v1: &[u8], v2: &[u8]) -> Result<f32> {
        let distance = v1
            .iter()
            .zip(v2.iter())
            .map(|(a, b)| (a ^ b).count_ones() as f32)
            .sum();
        Ok(distance)
    }

    /// Quantize vector to INT8 format
    async fn quantize_to_int8(&self, vector: &[f32]) -> Result<Vec<i8>> {
        let quantized = self.engine.quantize_to_int8(vector)?;
        Ok(quantized.iter().map(|&b| b as i8).collect())
    }

    /// Compute INT8 distance
    async fn compute_int8_distance(&self, v1: &[i8], v2: &[i8]) -> Result<f32> {
        let distance: f32 = v1
            .iter()
            .zip(v2.iter())
            .map(|(a, b)| {
                let diff = (*a as f32) - (*b as f32);
                diff * diff
            })
            .sum();
        Ok(distance.sqrt())
    }

    /// Compute PQ distance table
    async fn compute_pq_distance_table(
        &self,
        vector: &[f32],
        segments: u8,
        bits: u8,
    ) -> Result<DistanceTable> {
        DistanceTable::compute_for_query(vector, segments, bits)
    }

    /// Compute PQ distance
    async fn compute_pq_distance(&self, table: &DistanceTable, code: &[u8]) -> Result<f32> {
        let pq_code = PQCode {
            codes: code.to_vec(),
        };
        Ok(table.lookup_distance(&pq_code))
    }

    /// Load binary sketch (stub for now)
    async fn load_binary_sketch(&self, _row_group_id: u32, _row_offset: u32) -> Result<Vec<u8>> {
        Ok(vec![0u8; 96]) // Placeholder
    }

    /// Load INT8 vector (stub for now)
    async fn load_int8_vector(&self, _row_group_id: u32, _row_offset: u32) -> Result<Vec<i8>> {
        Ok(vec![0i8; 768]) // Placeholder
    }

    /// Load PQ code (stub for now)
    async fn load_pq_code(&self, _row_group_id: u32, _row_offset: u32) -> Result<Vec<u8>> {
        Ok(vec![0u8; 32]) // Placeholder
    }
}
/// Configuration for progressive columnar search
///
/// Configures the multi-stage progressive search pipeline with parameters
/// for each quantization stage and overall search behavior.
#[derive(Debug, Clone)]
pub struct ProgressiveSearchConfig {
    /// Binary quantization stage configuration
    pub binary_config: StageConfig,
    /// INT8 quantization stage configuration
    pub int8_config: StageConfig,
    /// Product Quantization stage configuration
    pub pq_config: StageConfig,
    /// Full precision final stage configuration
    pub full_precision_config: StageConfig,

    /// Streaming configuration for memory-efficient processing
    pub streaming_config: StreamingConfig,
    /// Enable cost-based row group ordering
    pub cost_based_ordering: bool,
    /// Enable adaptive distance thresholds
    pub adaptive_thresholds: bool,
    /// Enable superblock-level pruning
    pub enable_superblock_pruning: bool,
    /// Quality vs performance trade-off (0.0 = fastest, 1.0 = best quality)
    pub quality_target: f32,
    /// Optional latency budget in milliseconds
    pub latency_budget_ms: Option<u64>,
    /// Optional memory budget in bytes
    pub memory_budget_bytes: Option<usize>,
}
/// Configuration for a single search stage
///
/// Defines the behavior and constraints for one stage in the progressive
/// search pipeline (binary, INT8, PQ, or full precision).
#[derive(Debug, Clone, Default)]
pub struct StageConfig {
    /// Maximum candidates to pass to next stage
    pub max_candidates: usize,
    /// Distance threshold for filtering (candidates above threshold are rejected)
    pub distance_threshold: Option<f32>,
    /// Memory limit for this stage in bytes
    pub memory_limit: usize,
    /// Enable parallel processing within the stage
    pub enable_parallelism: bool,
    /// Timeout for stage completion in milliseconds
    pub timeout_ms: u64,
}

/// Result of progressive search
///
/// Contains the final search results along with detailed performance
/// metrics for each stage of the progressive search pipeline.
#[derive(Debug)]
pub struct ProgressiveSearchResult {
    /// Final top-k results
    pub results: Vec<VectorRecord>,
    /// Performance metrics per stage
    pub stage_metrics: Vec<StageMetrics>,
    /// Total search time in milliseconds
    pub total_time_ms: u64,
    /// Total candidates processed across all stages
    pub total_candidates_processed: usize,
    /// Total candidates filtered out across all stages
    pub total_candidates_filtered: usize,
    /// Peak memory usage during search (bytes)
    pub memory_peak_usage: usize,
    /// Number of row groups scanned
    pub row_groups_scanned: usize,
    /// Number of superblocks pruned
    pub superblocks_pruned: usize,
}

/// Metrics for a single search stage
#[derive(Debug, Clone)]
pub struct StageMetrics {
    /// Processing stage type
    pub stage: ProcessingStage,
    /// Stage duration in milliseconds
    pub duration_ms: u64,
    /// Number of candidates entering the stage
    pub candidates_in: usize,
    /// Number of candidates exiting the stage
    pub candidates_out: usize,
    /// Memory used during this stage (bytes)
    pub memory_used: usize,
    /// Number of row groups processed
    pub row_groups_processed: usize,
    /// Filtering effectiveness (0.0 = no filtering, 1.0 = perfect filtering)
    pub effectiveness: f32,
}

/// Progressive search candidate with stage information
///
/// Represents a search candidate progressing through the multi-stage
/// search pipeline, with location and similarity information.
pub struct ProgressiveCandidate {
    /// Row group identifier
    pub row_group_id: u32,
    /// Row offset within the row group
    pub row_offset: u32,
    /// Similarity score (higher is better)
    pub similarity: f32,
    /// Optional vector identifier
    pub vector_id: Option<String>,
    /// Optional full vector record (loaded in final stage)
    pub record: Option<VectorRecord>,
}

impl PartialEq for ProgressiveCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.similarity == other.similarity
    }
}

impl Eq for ProgressiveCandidate {}

impl Ord for ProgressiveCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse for min-heap (best candidates first)
        other
            .similarity
            .partial_cmp(&self.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl PartialOrd for ProgressiveCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Main progressive columnar search engine
///
/// Implements a multi-stage search pipeline that progressively refines
/// candidates through binary, INT8, PQ, and full precision stages.
pub struct ProgressiveColumnarSearch {
    /// Search configuration
    config: ProgressiveSearchConfig,
    /// Streaming row group processor
    streaming_processor: StreamingRowGroupProcessor,
    /// Distance metric for similarity computation
    distance_metric: DistanceMetric,
    /// Unified distance computation engine
    distance_compute: Arc<UnifiedDistanceCompute>,
    /// Quantization adapter for multi-stage processing
    quantization_adapter: QuantizationAdapter,
}

impl ProgressiveColumnarSearch {
    /// Create a new progressive search engine
    pub fn new(
        config: ProgressiveSearchConfig,
        distance_metric: DistanceMetric,
        distance_compute: Arc<UnifiedDistanceCompute>,
        quantization_engine: Arc<UnifiedQuantizationEngine>,
    ) -> Self {
        let streaming_processor = StreamingRowGroupProcessor::new(config.streaming_config.clone());
        let quantization_adapter = QuantizationAdapter {
            engine: quantization_engine,
        };

        Self {
            config,
            streaming_processor,
            distance_metric,
            distance_compute,
            quantization_adapter,
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
            self.prune_superblocks(query_vector, superblocks, &mut superblocks_pruned)
                .await?
        } else {
            superblocks.to_vec()
        };
        info!(
            "SuperBlock pruning: {} → {} superblocks",
            superblocks.len(),
            relevant_superblocks.len()
        );
        // Phase 2: Row group ordering and streaming processing
        let streaming_context = StreamingContext {
            query_vector: query_vector.to_vec(),
            top_k,
            distance_threshold: None,
            superblocks: relevant_superblocks,
            enhanced_stats: enhanced_stats.to_vec(),
        };

        let row_group_results = self
            .streaming_processor
            .process_row_groups_streaming(streaming_context, parquet_metadata)
            .await?;
        // Phase 3: Progressive refinement stages
        let candidates = self.collect_initial_candidates(&row_group_results).await?;
        // Binary stage
        let (candidates, binary_metrics) = self
            .execute_binary_stage(
                query_vector,
                candidates,
                &mut total_candidates_processed,
                &mut total_candidates_filtered,
                &mut memory_peak_usage,
            )
            .await?;
        stage_metrics.push(binary_metrics);
        if candidates.is_empty() {
            return Ok(self.build_empty_result(stage_metrics, start_time));
        }

        // INT8 stage
        let (candidates, int8_metrics) = self
            .execute_int8_stage(
                query_vector,
                candidates,
                &mut total_candidates_processed,
                &mut total_candidates_filtered,
                &mut memory_peak_usage,
            )
            .await?;
        stage_metrics.push(int8_metrics);

        // PQ stage
        let (candidates, pq_metrics) = self
            .execute_pq_stage(
                query_vector,
                candidates,
                &mut total_candidates_processed,
                &mut total_candidates_filtered,
                &mut memory_peak_usage,
            )
            .await?;
        stage_metrics.push(pq_metrics);

        // Full precision stage
        let (final_results, fp_metrics) = self
            .execute_full_precision_stage(
                query_vector,
                candidates,
                top_k,
                &mut total_candidates_processed,
                &mut memory_peak_usage,
            )
            .await?;
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
            total_candidates_processed,
            total_candidates_filtered,
            memory_peak_usage,
            row_groups_scanned: row_group_results.len(),
            superblocks_pruned,
            total_time_ms,
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
                match self.distance_metric {
                    DistanceMetric::Cosine => "cosine".to_string(),
                    DistanceMetric::Euclidean => "euclidean".to_string(),
                    DistanceMetric::DotProduct => "dot".to_string(),
                    _ => "euclidean".to_string(),
                },
                f32::INFINITY, // For now, don't use distance threshold
            ) {
                relevant_blocks.push(superblock.clone());
            } else {
                *pruned_count += 1;
            }
        }

        // Sort by estimated search cost
        relevant_blocks.sort_by(|a, b| {
            a.selectivity_hints
                .search_cost_estimate
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
                    similarity: rg_candidate.similarity,
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
        _peak_memory: &mut usize,
    ) -> Result<(Vec<ProgressiveCandidate>, StageMetrics)> {
        let stage_start = std::time::Instant::now();
        let candidates_in = candidates.len();
        *total_processed += candidates_in;
        debug!("Binary stage: processing {} candidates", candidates_in);
        // Convert query to binary sketch using quantization adapter
        let binary_query = self
            .quantization_adapter
            .quantize_to_binary(query_vector)
            .await?;
        // Process candidates through binary filtering
        let mut filtered_candidates = Vec::new();
        let max_candidates = self.config.binary_config.max_candidates;
        let mut candidate_heap = BinaryHeap::new();
        for candidate in candidates {
            // Simulate loading binary sketch for this candidate
            let binary_sketch = self
                .load_binary_sketch(candidate.row_group_id, candidate.row_offset)
                .await?;

            // Compute binary distance using quantization adapter
            let binary_distance = self
                .quantization_adapter
                .compute_hamming_distance(&binary_query, &binary_sketch)
                .await?;
            // Check threshold
            if let Some(threshold) = self.config.binary_config.distance_threshold
                && binary_distance > threshold
            {
                continue;
            }

            let updated_candidate = ProgressiveCandidate {
                row_group_id: candidate.row_group_id,
                row_offset: candidate.row_offset,
                similarity: binary_distance,
                vector_id: None,
                record: None,
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
            memory_used: candidates_in * 96, // Approximate binary sketch size
            row_groups_processed: 0,         // Calculated elsewhere
            effectiveness,
            candidates_in,
            candidates_out,
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
        _peak_memory: &mut usize,
    ) -> Result<(Vec<ProgressiveCandidate>, StageMetrics)> {
        let stage_start = std::time::Instant::now();
        let candidates_in = candidates.len();
        *total_processed += candidates_in;
        debug!("INT8 stage: processing {} candidates", candidates_in);
        // Convert query to INT8 using quantization adapter
        let int8_query = self
            .quantization_adapter
            .quantize_to_int8(query_vector)
            .await?;
        let max_candidates = self.config.int8_config.max_candidates;
        let mut filtered_candidates = Vec::new();
        let mut candidate_heap = BinaryHeap::new();

        for candidate in candidates {
            // Simulate loading INT8 vector for this candidate
            let int8_vector = self
                .load_int8_vector(candidate.row_group_id, candidate.row_offset)
                .await?;

            // Compute INT8 distance using quantization adapter
            let int8_distance = self
                .quantization_adapter
                .compute_int8_distance(&int8_query, &int8_vector)
                .await?;

            // Check threshold
            if let Some(threshold) = self.config.int8_config.distance_threshold
                && int8_distance > threshold
            {
                continue;
            }

            let updated_candidate = ProgressiveCandidate {
                row_group_id: candidate.row_group_id,
                row_offset: candidate.row_offset,
                similarity: int8_distance,
                vector_id: candidate.vector_id.clone(),
                record: None,
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
        _peak_memory: &mut usize,
    ) -> Result<(Vec<ProgressiveCandidate>, StageMetrics)> {
        let stage_start = std::time::Instant::now();
        let candidates_in = candidates.len();
        *total_processed += candidates_in;
        debug!("PQ stage: processing {} candidates", candidates_in);
        // Compute PQ distance table using quantization adapter
        let distance_table = self
            .quantization_adapter
            .compute_pq_distance_table(query_vector, 32, 8)
            .await?;
        let max_candidates = self.config.pq_config.max_candidates;
        let mut filtered_candidates = Vec::new();
        let mut candidate_heap = BinaryHeap::new();

        for candidate in candidates {
            // Simulate loading PQ code for this candidate
            let pq_code = self
                .load_pq_code(candidate.row_group_id, candidate.row_offset)
                .await?;

            // Compute PQ distance using quantization adapter
            let pq_distance = self
                .quantization_adapter
                .compute_pq_distance(&distance_table, &pq_code)
                .await?;

            // Check threshold
            if let Some(threshold) = self.config.pq_config.distance_threshold
                && pq_distance > threshold
            {
                continue;
            }

            let updated_candidate = ProgressiveCandidate {
                row_group_id: candidate.row_group_id,
                row_offset: candidate.row_offset,
                similarity: pq_distance,
                vector_id: candidate.vector_id.clone(),
                record: None,
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
            stage: ProcessingStage::PQ8Filter,
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
        _peak_memory: &mut usize,
    ) -> Result<(Vec<VectorRecord>, StageMetrics)> {
        let stage_start = std::time::Instant::now();
        let candidates_in = candidates.len();
        *total_processed += candidates_in;
        debug!(
            "Full precision stage: processing {} candidates",
            candidates_in
        );
        let mut final_candidates = Vec::new();

        for (i, candidate) in candidates.into_iter().enumerate() {
            // Load full precision vector
            let full_vector = self
                .load_full_vector(candidate.row_group_id, candidate.row_offset)
                .await?;

            // Compute exact distance using universal adapter
            let exact_distance = self.compute_exact_distance(query_vector, &full_vector)?;

            let record = VectorRecord {
                id: candidate
                    .vector_id
                    .clone()
                    .unwrap_or_else(|| format!("unknown_{}", i)),
                vector: full_vector,
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            };
            final_candidates.push((record, exact_distance));
        }

        // Use bounded priority queue for efficient top-k selection
        let mut priority_queue = BoundedPriorityQueue::new(top_k);

        for (record, distance) in final_candidates {
            // Convert distance to score (higher is better)
            let score = 1.0 / (1.0 + distance);

            let search_record = crate::core::search::results::OptimizedSearchRecord {
                id: record.id.clone(),
                vector_id: Some(record.id.clone()),
                score,
                similarity: Some(distance),
                vector: Some(Arc::new(record.vector.clone())),
                metadata: record.metadata.clone(),
                debug_info: None,
                version: None,
                timestamp: None,
                updated_at: None,
                expires_at: None,
                source: None,
                expanded_context: vec![],
                semantic_similarity: None,
                quantization_info: None,
                engine_stats: None,
                index_path: None,
            };

            priority_queue.try_insert(search_record);
        }

        // Get sorted results and convert back to VectorRecord format
        let top_results = priority_queue.into_sorted_vec();

        let results: Vec<VectorRecord> = top_results
            .into_iter()
            .map(|search_record| VectorRecord {
                id: search_record.id,
                vector: search_record
                    .vector
                    .map(|v| (*v).clone())
                    .unwrap_or_default(),
                metadata: search_record.metadata,
                version: None,
                timestamp: Some(0),
                expires_at: None,
                updated_at: None,
                source: None,
            })
            .collect();
        let candidates_out = results.len();
        let duration_ms = stage_start.elapsed().as_millis() as u64;

        debug!(
            "Full precision stage completed: {} → {} results in {}ms",
            candidates_in, candidates_out, duration_ms
        );

        let metrics = StageMetrics {
            stage: ProcessingStage::FullPrecision,
            duration_ms,
            candidates_in,
            candidates_out,
            memory_used: candidates_in * query_vector.len() * 4, // Full vector size
            row_groups_processed: 0,
            effectiveness: 1.0, // Full precision doesn't filter
        };

        Ok((results, metrics))
    }

    // Helper methods for loading data (would be implemented with actual Parquet reading)
    async fn load_binary_sketch(&self, row_group_id: u32, row_offset: u32) -> Result<Vec<u8>> {
        // Load binary sketch from Parquet using quantization adapter
        self.quantization_adapter
            .load_binary_sketch(row_group_id, row_offset)
            .await
    }

    async fn load_int8_vector(&self, row_group_id: u32, row_offset: u32) -> Result<Vec<i8>> {
        // Load INT8 vector from Parquet using quantization adapter
        self.quantization_adapter
            .load_int8_vector(row_group_id, row_offset)
            .await
    }

    async fn load_pq_code(&self, row_group_id: u32, row_offset: u32) -> Result<Vec<u8>> {
        // Load PQ code from Parquet using quantization adapter
        self.quantization_adapter
            .load_pq_code(row_group_id, row_offset)
            .await
    }

    async fn load_full_vector(&self, _row_group_id: u32, _row_offset: u32) -> Result<Vec<f32>> {
        // Simulate loading full vector from Parquet
        Ok(vec![0.0f32; 768])
    }

    fn compute_exact_distance(&self, query: &[f32], vector: &[f32]) -> Result<f32> {
        // Use UnifiedDistanceCompute instead of local implementation
        let result = self
            .distance_compute
            .calculate_distance(query, vector, &self.distance_metric);
        Ok(result.normalized_score)
    }

    fn build_empty_result(
        &self,
        stage_metrics: Vec<StageMetrics>,
        start_time: std::time::Instant,
    ) -> ProgressiveSearchResult {
        ProgressiveSearchResult {
            results: Vec::new(),
            total_time_ms: start_time.elapsed().as_millis() as u64,
            total_candidates_processed: 0,
            total_candidates_filtered: 0,
            memory_peak_usage: 0,
            row_groups_scanned: 0,
            superblocks_pruned: 0,
            stage_metrics,
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
                memory_limit: 8 * 1024 * 1024, // 8MB
                enable_parallelism: false,
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
    fn compute_for_query(_query: &[f32], segments: u8, bits: u8) -> Result<Self> {
        // Simulate distance table computation
        let centroids_per_segment = 1 << bits; // 2^bits
        let mut table = Vec::new();
        for _segment in 0..segments {
            let mut segment_distances = Vec::new();
            for _centroid in 0..centroids_per_segment {
                // Simulate distance computation
                segment_distances.push(0.5f32);
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

#[allow(dead_code)]
impl BinarySketch {
    fn from_vector(vector: &[f32], threshold: f32) -> Self {
        let dimension = vector.len();
        let num_bytes = dimension.div_ceil(8);
        let mut bits = vec![0u8; num_bytes];
        for (i, &value) in vector.iter().enumerate() {
            if value > threshold {
                let byte_idx = i / 8;
                let bit_idx = i % 8;
                bits[byte_idx] |= 1u8 << bit_idx;
            }
        }
        Self { bits }
    }

    fn hamming_distance(&self, other: &Self) -> u32 {
        self.bits
            .iter()
            .zip(other.bits.iter())
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }
}

#[allow(dead_code)]
impl Int8Vector {
    pub(crate) fn from_vector(vector: &[f32]) -> Self {
        // Find min and max for scaling
        let min_val = vector.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let max_val = vector.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        let scale = (max_val - min_val) / 255.0;
        let zero_point = (-min_val / scale).round() as i8;
        let values = vector
            .iter()
            .map(|&x| {
                ((x / scale) + zero_point as f32)
                    .round()
                    .clamp(-128.0, 127.0) as i8
            })
            .collect();

        Self {
            values,
            scale,
            zero_point,
        }
    }

    pub(crate) fn l2_distance_squared(&self, other: &Self) -> f32 {
        self.values
            .iter()
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
        assert_eq!(vector.len(), 5); // Test the input vector dimension instead
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
            vector_id: None,
            record: None,
        });

        heap.push(ProgressiveCandidate {
            row_group_id: 0,
            row_offset: 1,
            similarity: 5.0,
            vector_id: None,
            record: None,
        });

        // Should pop smallest similarity first (min-heap behavior)
        assert_eq!(heap.pop().unwrap().similarity, 5.0);
        assert_eq!(heap.pop().unwrap().similarity, 10.0);
    }
}
