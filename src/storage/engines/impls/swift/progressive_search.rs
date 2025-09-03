// Progressive similarity search for SST
// Multi-level refinement: Binary → INT8 → PQ → Full precision

use anyhow::{Result, anyhow};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info};

use super::{MetadataFilter, SwiftFile};
use crate::compute::distance_computation::{
    DistanceMetric, SimilarityResult, UnifiedDistanceCompute,
};
use crate::compute::quantization::storage_engine::{
    StorageQuantizationConfig, StorageQuantizationEngine, StorageQuantizedData,
};
use crate::compute::quantization::unified::UnifiedQuantizationLevel;
use crate::core::VectorRecord;
use crate::storage::engines::core::formats::fastlanes_blocks::{FastLanesDataBlock, SuperBlock};

/// Helper function to compute L2 distance squared for INT8 vectors
fn compute_l2_distance_squared_i8(a: &[i8], b: &[i8]) -> Result<f32> {
    if a.len() != b.len() {
        return Err(anyhow!("Vector dimensions don't match"));
    }

    let distance: f32 = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let diff = (*x as f32) - (*y as f32);
            diff * diff
        })
        .sum();

    Ok(distance)
}

/// Helper function to compare JSON values
fn compare_json_values(
    a: &serde_json::Value,
    b: &serde_json::Value,
    expected: std::cmp::Ordering,
) -> Option<bool> {
    use serde_json::Value;

    match (a, b) {
        (Value::Number(n1), Value::Number(n2)) => {
            if let (Some(f1), Some(f2)) = (n1.as_f64(), n2.as_f64()) {
                Some(match expected {
                    Ordering::Greater => f1 >= f2,
                    Ordering::Less => f1 <= f2,
                    Ordering::Equal => (f1 - f2).abs() < f64::EPSILON,
                })
            } else if let (Some(i1), Some(i2)) = (n1.as_i64(), n2.as_i64()) {
                Some(match expected {
                    Ordering::Greater => i1 >= i2,
                    Ordering::Less => i1 <= i2,
                    Ordering::Equal => i1 == i2,
                })
            } else {
                None
            }
        }
        (Value::String(s1), Value::String(s2)) => Some(match expected {
            Ordering::Greater => s1 >= s2,
            Ordering::Less => s1 <= s2,
            Ordering::Equal => s1 == s2,
        }),
        (Value::Bool(b1), Value::Bool(b2)) => {
            Some(match expected {
                Ordering::Equal => b1 == b2,
                _ => false, // Booleans don't have ordering
            })
        }
        _ => None, // Can't compare other types
    }
}

// Temporary local definition until unified quantization types are available
#[derive(Debug, Clone)]
struct BinarySketch {
    bits: Vec<u8>,
    dimension: usize,
}

impl BinarySketch {
    fn hamming_distance(&self, other: &BinarySketch) -> u32 {
        self.bits
            .iter()
            .zip(other.bits.iter())
            .map(|(a, b)| (*a ^ *b).count_ones())
            .sum()
    }
}
// Quantization types from unified compute module
use crate::compute::quantization::unified::{
    BinaryQuantization, ProductQuantization, ScalarQuantization,
};

// Distance table type for PQ search
type DistanceTable = Vec<Vec<f32>>;

/// Configuration for progressive search
#[derive(Debug, Clone)]
pub struct ProgressiveSearchConfig {
    /// Expansion factor for each level
    pub binary_expansion: usize, // e.g., 10x top_k
    pub int8_expansion: usize, // e.g., 5x top_k
    pub pq_expansion: usize,   // e.g., 2x top_k

    /// Distance thresholds for early termination
    pub binary_threshold: f32,
    pub int8_threshold: f32,
    pub pq_threshold: f32,

    /// Parallelism settings
    pub max_concurrent_blocks: usize,

    /// Cache settings
    pub cache_distance_tables: bool,
}

impl Default for ProgressiveSearchConfig {
    fn default() -> Self {
        Self {
            binary_expansion: 10,
            int8_expansion: 5,
            pq_expansion: 2,
            binary_threshold: 100.0,
            int8_threshold: 50.0,
            pq_threshold: 10.0,
            max_concurrent_blocks: 10,
            cache_distance_tables: true,
        }
    }
}

/// Candidate at various stages of refinement
#[derive(Debug, Clone)]
struct Candidate {
    superblock_idx: u32,
    block_idx: u32,
    vector_idx: u32,
    similarity: f32,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.similarity == other.similarity
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // Reverse order for min-heap
        other.similarity.partial_cmp(&self.similarity)
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}

/// Main progressive search function
pub async fn search_progressive(
    sst: &SwiftFile,
    query: &[f32],
    top_k: usize,
    filter: Option<MetadataFilter>,
) -> Result<Vec<VectorRecord>> {
    let config = ProgressiveSearchConfig::default();

    // Create quantization engine for binary operations
    let quantization_engine = StorageQuantizationEngine::new_default();

    info!(
        "Starting progressive search for top-{} with query dimension {}",
        top_k,
        query.len()
    );

    // Phase 1: Binary sketch filtering
    let binary_candidates = phase1_binary_filtering(
        sst,
        query,
        top_k * config.binary_expansion,
        &filter,
        config.binary_threshold,
        &quantization_engine,
    )
    .await?;

    debug!(
        "Phase 1 complete: {} binary candidates from {} superblocks",
        binary_candidates.len(),
        sst.superblocks.len()
    );

    if binary_candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Phase 2: INT8 filtering
    let int8_candidates = phase2_int8_filtering(
        sst,
        query,
        binary_candidates,
        top_k * config.int8_expansion,
        config.int8_threshold,
    )
    .await?;

    debug!(
        "Phase 2 complete: {} INT8 candidates",
        int8_candidates.len()
    );

    if int8_candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Phase 3: PQ distance computation
    let pq_candidates = phase3_pq_refinement(
        sst,
        query,
        int8_candidates,
        top_k * config.pq_expansion,
        config.pq_threshold,
    )
    .await?;

    debug!("Phase 3 complete: {} PQ candidates", pq_candidates.len());

    // Phase 4: Full precision reranking
    let final_results = phase4_full_precision(
        sst,
        query,
        pq_candidates,
        top_k,
        filter,
        config.max_concurrent_blocks,
    )
    .await?;

    info!(
        "Progressive search complete: {} results",
        final_results.len()
    );

    Ok(final_results)
}

/// Phase 1: Binary sketch filtering
async fn phase1_binary_filtering(
    sst: &SwiftFile,
    query: &[f32],
    n_candidates: usize,
    filter: &Option<MetadataFilter>,
    threshold: f32,
    quantization_engine: &StorageQuantizationEngine,
) -> Result<Vec<Candidate>> {
    // Use binary quantization approach - create a simple binary representation
    // TODO: Implement proper binary quantization via storage engine
    let binary_query_bits = query
        .iter()
        .map(|&x| if x > 0.0 { 1u8 } else { 0u8 })
        .collect::<Vec<u8>>();
    let binary_query = BinarySketch {
        bits: binary_query_bits,
        dimension: query.len(),
    };
    let mut candidates = BinaryHeap::new();

    // First check superblock-level signatures
    for (sb_idx, superblock) in sst.superblocks.iter().enumerate() {
        // Quick check with superblock signature
        let sb_binary = BinarySketch {
            bits: superblock.quantized_signature.clone(),
            dimension: query.len(),
        };

        let sb_distance = binary_query.hamming_distance(&sb_binary) as f32;
        if sb_distance > threshold * 2.0 {
            continue; // Skip entire superblock
        }

        // Check individual blocks
        for (b_idx, block) in superblock.blocks.iter().enumerate() {
            // Apply metadata filter at block level
            if let Some(f) = filter {
                if !block_matches_filter(block, f) {
                    continue;
                }
            }

            // Check each vector in block using binary sketches
            if let Some(ref sketches) = block.quantized_vectors {
                for (v_idx, sketch) in sketches.iter().enumerate() {
                    let sketch_binary = BinarySketch {
                        bits: sketch.clone(),
                        dimension: query.len(),
                    };
                    let distance = binary_query.hamming_distance(&sketch_binary) as f32;

                    if distance <= threshold {
                        candidates.push(Candidate {
                            superblock_idx: sb_idx as u32,
                            block_idx: b_idx as u32,
                            vector_idx: v_idx as u32,
                            similarity: distance,
                        });

                        // Keep only top candidates
                        if candidates.len() > n_candidates {
                            candidates.pop();
                        }
                    }
                }
            }
        }
    }

    // Convert heap to vector
    let mut result = Vec::new();
    while let Some(candidate) = candidates.pop() {
        result.push(candidate);
    }
    result.reverse(); // Best first

    Ok(result)
}

/// Phase 2: INT8 filtering
async fn phase2_int8_filtering(
    sst: &SwiftFile,
    query: &[f32],
    binary_candidates: Vec<Candidate>,
    n_candidates: usize,
    threshold: f32,
) -> Result<Vec<Candidate>> {
    // Use unified quantization module for INT8 quantization
    let quantization_config = StorageQuantizationConfig::default();
    let quantization_engine = StorageQuantizationEngine::new_with_config(quantization_config);

    // Quantize the query vector to INT8
    let quantized_query = quantization_engine
        .quantize_batch_with_level(&[query.to_vec()], UnifiedQuantizationLevel::int8())
        .await?;

    let int8_query = if let Some(q) = quantized_query.first() {
        if let Some(primary) = &q.primary {
            primary.data.iter().map(|&b| b as i8).collect::<Vec<_>>()
        } else {
            return Err(anyhow!("Failed to quantize query vector"));
        }
    } else {
        return Err(anyhow!("Failed to quantize query vector"));
    };
    let mut candidates = BinaryHeap::new();

    // Group candidates by block for efficient access
    let mut blocks_to_check = std::collections::HashMap::new();
    for candidate in binary_candidates {
        blocks_to_check
            .entry((candidate.superblock_idx, candidate.block_idx))
            .or_insert_with(Vec::new)
            .push(candidate.vector_idx);
    }

    for ((sb_idx, b_idx), vector_indices) in blocks_to_check {
        let block = &sst.superblocks[sb_idx as usize].blocks[b_idx as usize];

        for v_idx in vector_indices {
            if let Some(ref quantized) = block.quantized_vectors {
                if let Some(int8_vec) = quantized.get(v_idx as usize) {
                    // Use distance computation for INT8 vectors
                    // Convert Vec<u8> to &[i8] using unsafe transmute
                    let int8_slice = unsafe {
                        std::slice::from_raw_parts(int8_vec.as_ptr() as *const i8, int8_vec.len())
                    };
                    let distance = compute_l2_distance_squared_i8(&int8_query, int8_slice)?;

                    if distance <= threshold {
                        candidates.push(Candidate {
                            superblock_idx: sb_idx,
                            block_idx: b_idx,
                            vector_idx: v_idx as u32,
                            similarity: distance,
                        });

                        if candidates.len() > n_candidates {
                            candidates.pop();
                        }
                    }
                }
            }
        }
    }

    // Convert to vector
    let mut result = Vec::new();
    while let Some(candidate) = candidates.pop() {
        result.push(candidate);
    }
    result.reverse();

    Ok(result)
}

/// Phase 3: PQ refinement
async fn phase3_pq_refinement(
    sst: &SwiftFile,
    query: &[f32],
    int8_candidates: Vec<Candidate>,
    n_candidates: usize,
    threshold: f32,
) -> Result<Vec<Candidate>> {
    // Create distance computation engine for PQ operations
    // Note: Skip PQ distance table computation for now, use direct computation
    let distance_table: Option<Vec<Vec<f32>>> = if !sst.header.quantization.pq_codebooks.is_empty()
    {
        // TODO: Implement proper PQ distance table computation
        None
    } else {
        None
    };

    // Use unified quantization for INT8 fallback
    let quantization_config = StorageQuantizationConfig::default();
    let quantization_engine = StorageQuantizationEngine::new_with_config(quantization_config);

    let quantized_query = quantization_engine
        .quantize_batch_with_level(&[query.to_vec()], UnifiedQuantizationLevel::int8())
        .await?;

    let int8_query = if let Some(q) = quantized_query.first() {
        if let Some(primary) = &q.primary {
            primary.data.iter().map(|&b| b as i8).collect::<Vec<_>>()
        } else {
            // Fallback to empty vector if quantization fails
            vec![]
        }
    } else {
        // Fallback to empty vector if quantization fails
        vec![]
    };

    let mut candidates = BinaryHeap::new();

    // Group by block for efficiency
    let mut blocks_to_check = std::collections::HashMap::new();
    for candidate in int8_candidates {
        blocks_to_check
            .entry((candidate.superblock_idx, candidate.block_idx))
            .or_insert_with(Vec::new)
            .push(candidate.vector_idx);
    }

    for ((sb_idx, b_idx), vector_indices) in blocks_to_check {
        let block = &sst.superblocks[sb_idx as usize].blocks[b_idx as usize];

        for v_idx in vector_indices {
            if let Some(ref quantized) = block.quantized_vectors {
                if let Some(pq_code) = quantized.get(v_idx as usize) {
                    // TODO: Implement proper PQ distance computation
                    let distance: f32 = pq_code
                        .iter()
                        .zip(query.iter())
                        .map(|(a, b)| {
                            let diff = (*a as f32) - b;
                            diff * diff
                        })
                        .sum::<f32>()
                        .sqrt();

                    if distance <= threshold {
                        candidates.push(Candidate {
                            superblock_idx: sb_idx,
                            block_idx: b_idx,
                            vector_idx: v_idx as u32,
                            similarity: distance,
                        });

                        if candidates.len() > n_candidates {
                            candidates.pop();
                        }
                    }
                }
            }
        }
    }

    // Convert to vector
    let mut result = Vec::new();
    while let Some(candidate) = candidates.pop() {
        result.push(candidate);
    }
    result.reverse();

    Ok(result)
}

/// Phase 4: Full precision reranking
async fn phase4_full_precision(
    sst: &SwiftFile,
    query: &[f32],
    pq_candidates: Vec<Candidate>,
    top_k: usize,
    filter: Option<MetadataFilter>,
    max_concurrent: usize,
) -> Result<Vec<VectorRecord>> {
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let mut handles = Vec::new();

    // Group by block for efficient loading
    let mut blocks_to_load = std::collections::HashMap::new();
    for candidate in pq_candidates {
        blocks_to_load
            .entry((candidate.superblock_idx, candidate.block_idx))
            .or_insert_with(Vec::new)
            .push(candidate.vector_idx);
    }

    // Load blocks in parallel and compute full precision distances
    for ((sb_idx, b_idx), vector_indices) in blocks_to_load {
        let sem = semaphore.clone();
        let query = query.to_vec();
        let filter = filter.clone();
        let distance_metric = sst.header.distance_metric;

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();

            // In real implementation, would load block from disk
            // For now, we'll simulate with the in-memory block
            let mut results = Vec::new();

            // This would actually load the block
            // let block = sst.load_block(sb_idx, b_idx).await?;

            // Compute distances for vectors in this block
            for v_idx in vector_indices {
                // let record = &block.records[v_idx as usize];
                // let compute = UnifiedDistanceCompute::new(distance_metric);
                // let result = compute.calculate_distance(&query, &record.vector, &distance_metric);
                // results.push((record.clone(), result.similarity));
            }

            Ok::<Vec<(VectorRecord, f32)>, anyhow::Error>(results)
        });

        handles.push(handle);
    }

    // Collect all results
    let mut all_results = Vec::new();
    for handle in handles {
        let block_results = handle.await??;
        all_results.extend(block_results);
    }

    // Apply final metadata filter if needed
    if let Some(f) = filter {
        all_results.retain(|(record, _)| record_matches_filter(record, &f));
    }

    // Sort by distance and take top-k
    all_results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    all_results.truncate(top_k);

    Ok(all_results.into_iter().map(|(r, _)| r).collect())
}

// Helper functions

fn bytes_to_bits(bytes: &[u8]) -> Vec<u64> {
    let mut bits = Vec::new();
    for chunk in bytes.chunks(8) {
        let mut word = 0u64;
        for (i, &byte) in chunk.iter().enumerate() {
            word |= (byte as u64) << (i * 8);
        }
        bits.push(word);
    }
    bits
}

fn block_matches_filter(block: &FastLanesDataBlock, filter: &MetadataFilter) -> bool {
    // Check block-level statistics against filter if available
    if let Some(ref stats) = block.metadata_stats {
        // Convert BlockMetadataStats to expected format
        // For now, skip block-level filtering if stats format doesn't match
        // TODO: Properly convert BlockMetadataStats to HashMap<String, ColumnStats>
        return true; // Conservative: include block if we can't check stats
    }
    true
}

fn condition_matches_block_stats(
    condition: &super::FilterCondition,
    stats: &std::collections::HashMap<String, super::ColumnStats>,
) -> bool {
    use super::FilterCondition;

    match condition {
        FilterCondition::Range(column, min, max) => {
            if let Some(col_stats) = stats.get(column) {
                // Check if range overlaps with block's range
                // Use JSON comparison helpers
                compare_json_values(&col_stats.max_value, min, std::cmp::Ordering::Greater)
                    .unwrap_or(false)
                    && compare_json_values(&col_stats.min_value, max, std::cmp::Ordering::Less)
                        .unwrap_or(false)
            } else {
                false
            }
        }
        _ => true, // Conservative: don't exclude block for other conditions
    }
}

fn record_matches_filter(record: &VectorRecord, filter: &MetadataFilter) -> bool {
    // Convert metadata to HashMap for easier lookup
    let metadata_map: std::collections::HashMap<String, serde_json::Value> = record
        .metadata
        .iter()
        .map(|item| (item.key.clone(), metadata_item_to_json(&item.value)))
        .collect();

    for condition in &filter.conditions {
        if !condition_matches_record(condition, &metadata_map) {
            return false;
        }
    }
    true
}

fn metadata_item_to_json(
    value: &Option<crate::proto::proximadb::metadata_item::Value>,
) -> serde_json::Value {
    match value {
        Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)) => {
            serde_json::Value::String(s.clone())
        }
        Some(crate::proto::proximadb::metadata_item::Value::NumberValue(f)) => {
            serde_json::Value::Number(
                serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0)),
            )
        }
        Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b)) => {
            serde_json::Value::Bool(*b)
        }
        None => serde_json::Value::Null,
    }
}

fn condition_matches_record(
    condition: &super::FilterCondition,
    metadata: &std::collections::HashMap<String, serde_json::Value>,
) -> bool {
    use super::FilterCondition;

    match condition {
        FilterCondition::Equals(column, value) => {
            metadata.get(column).map_or(false, |v| v == value)
        }
        FilterCondition::Range(column, min, max) => metadata.get(column).map_or(false, |v| {
            compare_json_values(v, min, std::cmp::Ordering::Greater).unwrap_or(false)
                && compare_json_values(v, max, std::cmp::Ordering::Less).unwrap_or(false)
        }),
        FilterCondition::In(column, values) => {
            metadata.get(column).map_or(false, |v| values.contains(v))
        }
        FilterCondition::IsNull(column) => {
            !metadata.contains_key(column) || metadata[column].is_null()
        }
        FilterCondition::IsNotNull(column) => {
            metadata.contains_key(column) && !metadata[column].is_null()
        }
    }
}

// Removed compute_distance wrapper - directly use UnifiedDistanceCompute in calling code

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_ordering() {
        let mut heap = BinaryHeap::new();

        heap.push(Candidate {
            superblock_idx: 0,
            block_idx: 0,
            vector_idx: 0,
            similarity: 10.0,
        });

        heap.push(Candidate {
            superblock_idx: 0,
            block_idx: 0,
            vector_idx: 1,
            similarity: 5.0,
        });

        heap.push(Candidate {
            superblock_idx: 0,
            block_idx: 0,
            vector_idx: 2,
            similarity: 15.0,
        });

        // Should pop in order: 5.0, 10.0, 15.0
        assert_eq!(heap.pop().unwrap().distance, 5.0);
        assert_eq!(heap.pop().unwrap().distance, 10.0);
        assert_eq!(heap.pop().unwrap().distance, 15.0);
    }

    #[test]
    fn test_distance_computation() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];

        let compute = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
        let euclidean_result = compute.calculate_distance(&a, &b, &DistanceMetric::Euclidean);
        assert!((euclidean_result.similarity - 1.414).abs() < 0.01);

        let cosine_result = compute.calculate_distance(&a, &b, &DistanceMetric::Cosine);
        assert!((cosine_result.similarity - 1.0).abs() < 0.01); // Orthogonal vectors

        let dot_result = compute.calculate_distance(&a, &b, &DistanceMetric::DotProduct);
        assert_eq!(dot_result.similarity, 0.0); // Orthogonal vectors
    }
}
