// Progressive similarity search for SST
// Multi-level refinement: Binary → INT8 → PQ → Full precision

use anyhow::{anyhow, Result};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info};

use crate::core::VectorRecord;
use crate::compute::distance_computation::DistanceMetric;
use super::{SstFile, MetadataFilter, SuperBlock, DataBlock};
use super::quantization_blocks::{BinarySketch, Int8Vector, PQCode, DistanceTable};

/// Configuration for progressive search
#[derive(Debug, Clone)]
pub struct ProgressiveSearchConfig {
    /// Expansion factor for each level
    pub binary_expansion: usize,      // e.g., 10x top_k
    pub int8_expansion: usize,        // e.g., 5x top_k
    pub pq_expansion: usize,          // e.g., 2x top_k
    
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
    superblock_idx:u32,
    block_idx: u32,
    vector_idx: u32,
    similarity: f32,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // Reverse order for min-heap
        other.distance.partial_cmp(&self.distance)
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}

/// Main progressive search function
pub async fn search_progressive(
    sst: &SstFile,
    query: &[f32],
    top_k: usize,
    filter: Option<MetadataFilter>,
) -> Result<Vec<VectorRecord>> {
    let config = ProgressiveSearchConfig::default();
    
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
    ).await?;
    
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
    ).await?;
    
    debug!("Phase 2 complete: {} INT8 candidates", int8_candidates.len());
    
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
    ).await?;
    
    debug!("Phase 3 complete: {} PQ candidates", pq_candidates.len());
    
    // Phase 4: Full precision reranking
    let final_results = phase4_full_precision(
        sst,
        query,
        pq_candidates,
        top_k,
        filter,
        config.max_concurrent_blocks,
    ).await?;
    
    info!("Progressive search complete: {} results", final_results.len());
    
    Ok(final_results)
}

/// Phase 1: Binary sketch filtering
async fn phase1_binary_filtering(
    sst: &SstFile,
    query: &[f32],
    n_candidates: usize,
    filter: &Option<MetadataFilter>,
    threshold: f32,
) -> Result<Vec<Candidate>> {
    let binary_query = BinarySketch::from_vector(query, 0.0);
    let mut candidates = BinaryHeap::new();
    
    // First check superblock-level signatures
    for (sb_idx, superblock) in sst.superblocks.iter().enumerate() {
        // Quick check with superblock signature
        let sb_binary = BinarySketch {
            bits: bytes_to_bits(&superblock.quantized_signature),
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
            for (v_idx, sketch) in block.quantized_section.binary_sketches.iter().enumerate() {
                let distance = binary_query.hamming_distance(sketch) as f32;
                
                if distance <= threshold {
                    candidates.push(Candidate {
                        superblock_idx:sb_idx as u32,
                        block_idx: b_idx as u32,
                        vector_idx: v_idx as u32,
                        distance,
                    });
                    
                    // Keep only top candidates
                    if candidates.len() > n_candidates {
                        candidates.pop();
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
    sst: &SstFile,
    query: &[f32],
    binary_candidates: Vec<Candidate>,
    n_candidates: usize,
    threshold: f32,
) -> Result<Vec<Candidate>> {
    let int8_query = Int8Vector::from_vector(query);
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
            if let Some(int8_vec) = block.quantized_section.int8_vectors.get(&vector_id) {
                let distance = int8_query.l2_distance_squared(int8_vec);
                
                if distance <= threshold {
                    candidates.push(Candidate {
                        superblock_idx:sb_idx,
                        block_idx: b_idx,
                        vector_idx: v_idx,
                        distance,
                    });
                    
                    if candidates.len() > n_candidates {
                        candidates.pop();
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
    sst: &SstFile,
    query: &[f32],
    int8_candidates: Vec<Candidate>,
    n_candidates: usize,
    threshold: f32,
) -> Result<Vec<Candidate>> {
    // Compute distance tables for PQ
    let distance_table = if !sst.header.quantization.pq_codebooks.is_empty() {
        Some(DistanceTable::compute(query, &sst.header.quantization.pq_codebooks))
    } else {
        None
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
            if let Some(pq_code) = block.quantized_section.pq_codes.get(key) {
                let distance = if let Some(ref dt) = distance_table {
                    dt.lookup_distance(pq_code)
                } else {
                    // Fallback to INT8 distance if no PQ
                    block.quantized_section.int8_vectors[v_idx as usize]
                        .l2_distance_squared(&Int8Vector::from_vector(query))
                };
                
                if distance <= threshold {
                    candidates.push(Candidate {
                        superblock_idx:sb_idx,
                        block_idx: b_idx,
                        vector_idx: v_idx,
                        distance,
                    });
                    
                    if candidates.len() > n_candidates {
                        candidates.pop();
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
    sst: &SstFile,
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
            let _permit = sem/* TODO: Fix VectorMemoryPool::acquire() method */.await.unwrap();
            
            // In real implementation, would load block from disk
            // For now, we'll simulate with the in-memory block
            let mut results = Vec::new();
            
            // This would actually load the block
            // let block = sst.load_block(sb_idx, b_idx).await?;
            
            // Compute distances for vectors in this block
            for v_idx in vector_indices {
                // let record = &block.records[v_idx as usize];
                // let distance = compute_distance(&query, &record.vector, distance_metric);
                // results.push((record.clone(), distance));
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
        all_results.retain(|(record, _)| {
            record_matches_filter(record, &f)
        });
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

fn block_matches_filter(block: &DataBlock, filter: &MetadataFilter) -> bool {
    // Check block-level statistics against filter
    for condition in &filter.conditions {
        if !condition_matches_block_stats(condition, &block.metadata_stats) {
            return false;
        }
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
            if let Some(col_stats) = stats.get(key) {
                // Check if range overlaps with block's range
                col_stats.max_value >= *min && col_stats.min_value <= *max
            } else {
                false
            }
        }
        _ => true, // Conservative: don't exclude block for other conditions
    }
}

fn record_matches_filter(record: &VectorRecord, filter: &MetadataFilter) -> bool {
    if let Some(metadata) = &record.metadata {
        for condition in &filter.conditions {
            if !condition_matches_record(condition, metadata) {
                return false;
            }
        }
    }
    true
}

fn condition_matches_record(
    condition: &super::FilterCondition,
    metadata: &std::collections::HashMap<String, serde_json::Value>,
) -> bool {
    use super::FilterCondition;
    
    match condition {
        FilterCondition::Equals(column, value) => {
            metadata.get(key).map_or(false, |v| v == value)
        }
        FilterCondition::Range(column, min, max) => {
            metadata.get(key).map_or(false, |v| v >= min && v <= max)
        }
        FilterCondition::In(column, values) => {
            metadata.get(key).map_or(false, |v| values.contains_hash(v))
        }
        FilterCondition::IsNull(column) => {
            !metadata.contains_key(column) || metadata[column].is_null()
        }
        FilterCondition::IsNotNull(column) => {
            metadata.contains_key(column) && !metadata[column].is_null()
        }
    }
}

fn compute_distance(a: &[f32], b: &[f32], metric: DistanceMetric) -> f32 {
    match metric {
        DistanceMetric::Euclidean => {
            a.iter()
                .zip(b.iter())
                .map(|(x, y)| (x - y).powi(2))
                .sum::<f32>()
                .sqrt()
        }
        DistanceMetric::Cosine => {
            let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
            let norm_a: f32 = a.iter().map(|x| x.powi(2)).sum::<f32>().sqrt();
            let norm_b: f32 = b.iter().map(|x| x.powi(2)).sum::<f32>().sqrt();
            1.0 - (dot / (norm_a * norm_b))
        }
        DistanceMetric::DotProduct => {
            -a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f32>()
        }
        _ => {
            // Fallback to Euclidean
            a.iter()
                .zip(b.iter())
                .map(|(x, y)| (x - y).powi(2))
                .sum::<f32>()
                .sqrt()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_candidate_ordering() {
        let mut heap = BinaryHeap::new();
        
        heap.push(Candidate {
            superblock_idx:0,
            block_idx: 0,
            vector_idx: 0,
            similarity: 10.0,
        });
        
        heap.push(Candidate {
            superblock_idx:0,
            block_idx: 0,
            vector_idx: 1,
            similarity: 5.0,
        });
        
        heap.push(Candidate {
            superblock_idx:0,
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
        
        let euclidean = compute_distance(&a, &b, DistanceMetric::Euclidean);
        assert!((euclidean - 1.414).abs() < 0.01);
        
        let cosine = compute_distance(&a, &b, DistanceMetric::Cosine);
        assert!((cosine - 1.0).abs() < 0.01); // Orthogonal vectors
        
        let dot = compute_distance(&a, &b, DistanceMetric::DotProduct);
        assert_eq!(dot, 0.0); // Orthogonal vectors
    }
}