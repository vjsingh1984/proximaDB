//! Query execution readers for HELIX engine
//!
//! This module provides efficient reading and searching of HELIX SSTables
//! with Hilbert-based pruning and FastLanes decoding.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, trace, warn, error};
use futures::future::join_all;

use crate::compute::distance_computation::DistanceMetric;
use crate::core::search::results::OptimizedSearchRecord;
use crate::core::metadata_types::TypedMetadata;
use crate::core::VectorRecord;
use crate::storage::persistence::filesystem::FileSystem;

use super::SStableMetadata;
// Filter evaluator now uses unified module from core
use crate::storage::engines::core::formats::fastlanes_blocks::bloom_filter::{factory::BloomFilterFactory, SerializedBloomFilter};

/// Check if SSTable might contain specific vector IDs using bloom filter
pub async fn check_bloom_filter(
    sstable: &SStableMetadata,
    vector_ids: &[String],
) -> Result<Vec<bool>> {
    if let Some(ref bloom_data) = sstable.bloom_filter {
        // Create SerializedBloomFilter structure from raw data
        // We need to parse the serialized format properly
        // For now, use the factory directly with appropriate config
        let config = crate::core::bloom::BloomFilterConfig {
            bits_per_key: 10,
            expected_items: 10000,
            enabled: true,
            ..Default::default()
        };
        
        let serialized = SerializedBloomFilter {
            strategy_type: crate::core::bloom::BloomStrategy::ByteAligned,
            version: SerializedBloomFilter::CURRENT_VERSION,
            config: config.clone(),
            data: bloom_data.clone(),
            metadata: HashMap::new(),
        };
        let bloom = BloomFilterFactory::from_serialized(&serialized)?;
        
        // Check each ID
        let results: Vec<bool> = vector_ids.iter()
            .map(|id| bloom.might_contain(id.as_bytes()))
            .collect();
        Ok(results)
    } else {
        // No bloom filter, assume all might be present
        Ok(vec![true; vector_ids.len()])
    }
}

/// Search an SSTable for nearest vectors with bloom filter optimization
pub async fn search_sstable(
    filesystem: &Arc<dyn FileSystem>,
    sstable: &SStableMetadata,
    query_vector: &[f32],
    k: usize,
    distance_metric: &DistanceMetric,
    filter: Option<Arc<dyn Fn(&HashMap<String, String>) -> bool + Send + Sync>>,
    candidate_ids: Option<&[String]>,  // Optional IDs to check via bloom filter
) -> Result<Vec<OptimizedSearchRecord>> {
    // Check bloom filter if candidate IDs provided
    if let Some(ids) = candidate_ids {
        if !ids.is_empty() && sstable.bloom_filter.is_some() {
            let bloom_results = check_bloom_filter(sstable, ids).await?;
            
            // Skip this SSTable if none of the candidate IDs might be present
            if !bloom_results.iter().any(|&present| present) {
                debug!("Skipping SSTable due to bloom filter pruning");
                return Ok(Vec::new());
            }
        }
    }
    
    debug!(
        "Searching SSTable at level {} with {} vectors",
        sstable.level, sstable.num_vectors
    );

    // Read file data
    let file_data = filesystem.read(&sstable.path.to_string_lossy()).await
        .context("Failed to read SSTable file")?;
    let mut cursor = std::io::Cursor::new(file_data);
    
    // Skip magic and version
    cursor.set_position(8);
    
    // Read number of blocks
    let mut num_blocks_bytes = [0u8; 4];
    std::io::Read::read_exact(&mut cursor, &mut num_blocks_bytes)?;
    let num_blocks = u32::from_le_bytes(num_blocks_bytes);
    
    let mut blocks = Vec::new();
    
    // Read blocks
    for _ in 0..num_blocks {
        // Read block size
        let mut size_bytes = [0u8; 4];
        std::io::Read::read_exact(&mut cursor, &mut size_bytes)?;
        let block_size = u32::from_le_bytes(size_bytes) as usize;
        
        // Read block data
        let mut block_data = vec![0u8; block_size];
        std::io::Read::read_exact(&mut cursor, &mut block_data)?;
        
        // Deserialize block
        use crate::storage::engines::core::formats::fastlanes_blocks::FastLanesDataBlock;
        let block = FastLanesDataBlock::deserialize(&block_data)?;
        blocks.push(block);
    }
    
    let mut results = Vec::new();
    let current_time = chrono::Utc::now().timestamp() as u64;
    
    for block in blocks {
        // Check if block should be pruned based on statistics
        if should_prune_block(&block.metadata, query_vector) {
            continue;
        }
        
        // Search within block
        for record in block.records {
            // Filter out expired records (tombstone support via expires_at)
            if let Some(expires_at) = record.expires_at {
                if expires_at as u64 <= current_time {
                    // Record is expired, skip it
                    debug!("Skipping expired record: {} (expired at {})", record.id, expires_at);
                    continue;
                }
            }
            
            // Apply filter if provided
            if let Some(f) = filter.as_ref() {
                // Convert metadata to HashMap<String, String> for filter
                let metadata_map: HashMap<String, String> = record.metadata.iter()
                    .filter_map(|(key, value)| {
                        if let Some(value) = &value {
                            let value_str = match value {
                                crate::proto::proximadb_v1::metadata_item::Value::StringValue(s) => s.clone(),
                                crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n) => n.to_string(),
                                crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b) => b.to_string(),
                            };
                            Some((key.clone(), value_str))
                        } else {
                            None
                        }
                    })
                    .collect();
                
                if !f(&metadata_map) {
                    continue;
                }
            }
            
            // Calculate distance (simple euclidean for now)
            let distance = query_vector.iter()
                .zip(record.vector.iter())
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>()
                .sqrt();
            
            results.push(OptimizedSearchRecord::new(record.id.clone(), 1.0 / (1.0 + distance))
                .with_similarity(distance)
                .add_vector(record.vector)
                .with_metadata(TypedMetadata::new()) // TODO: Convert record.metadata properly
                .with_version_info(record.version.unwrap_or(0), record.timestamp as u32));
        }
    }
    
    // Sort by distance and take top-k
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    results.truncate(k);
    
    Ok(results)
}

/// Find a specific vector by ID
pub async fn find_vector_by_id(
    filesystem: &Arc<dyn FileSystem>,
    sstable: &SStableMetadata,
    vector_id: &str,
) -> Result<Option<VectorRecord>> {
    // Check bloom filter if available
    if let Some(ref bloom_data) = sstable.bloom_filter {
        // Deserialize and check bloom filter
        // If not present, return early
        // (Implementation would use actual bloom filter)
    }
    
    // Read file data
    let file_data = filesystem.read(&sstable.path.to_string_lossy()).await?;
    let mut cursor = std::io::Cursor::new(file_data);
    
    // Skip magic and version
    cursor.set_position(8);
    
    // Read number of blocks
    let mut num_blocks_bytes = [0u8; 4];
    std::io::Read::read_exact(&mut cursor, &mut num_blocks_bytes)?;
    let num_blocks = u32::from_le_bytes(num_blocks_bytes);
    
    // Read blocks
    for _ in 0..num_blocks {
        // Read block size
        let mut size_bytes = [0u8; 4];
        std::io::Read::read_exact(&mut cursor, &mut size_bytes)?;
        let block_size = u32::from_le_bytes(size_bytes) as usize;
        
        // Read block data
        let mut block_data = vec![0u8; block_size];
        std::io::Read::read_exact(&mut cursor, &mut block_data)?;
        
        // Deserialize block
        use crate::storage::engines::core::formats::fastlanes_blocks::FastLanesDataBlock;
        let block = FastLanesDataBlock::deserialize(&block_data)?;
        
        let current_time = chrono::Utc::now().timestamp() as u64;
        for record in block.records {
            if record.id == vector_id {
                // Check if record is expired (tombstone support)
                if let Some(expires_at) = record.expires_at {
                    if expires_at as u64 <= current_time {
                        // Record is expired, treat as deleted
                        return Ok(None);
                    }
                }
                return Ok(Some(record));
            }
        }
    }
    
    Ok(None)
}

/// Check if a block should be pruned based on statistics
fn should_prune_block(
    metadata: &crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesBlockMetadata,
    _query_vector: &[f32],
) -> bool {
    // Simple pruning based on Hilbert range
    // In production, would use more sophisticated pruning
    
    // For now, don't prune any blocks
    false
}

/// Parallel search across multiple SSTables with thread-safe filter
/// 
/// This function distributes the search across multiple threads, with each
/// thread searching one or more SSTables in parallel for maximum performance.
pub async fn parallel_search(
    filesystem: Arc<dyn FileSystem>,
    sstables: Vec<SStableMetadata>,
    query_vector: Vec<f32>,
    k: usize,
    distance_metric: DistanceMetric,
    filter: Option<Arc<dyn Fn(&HashMap<String, String>) -> bool + Send + Sync>>,
) -> Result<Vec<OptimizedSearchRecord>> {
    if sstables.is_empty() {
        return Ok(Vec::new());
    }
    
    info!("Starting parallel search across {} SSTables", sstables.len());
    let start = std::time::Instant::now();
    
    // Create search tasks for each SSTable
    let search_tasks = sstables.into_iter().enumerate().map(|(idx, sstable)| {
        let fs = filesystem.clone();
        let query = query_vector.clone();
        let metric = distance_metric.clone();
        let filter_clone = filter.clone();
        
        tokio::spawn(async move {
            trace!("Thread {} searching SSTable at level {}", idx, sstable.level);
            let thread_start = std::time::Instant::now();
            
            // Search the SSTable with the thread-safe filter
            let result = search_sstable(
                &fs, 
                &sstable, 
                &query, 
                k, 
                &metric, 
                filter_clone,
                None  // No candidate IDs for now
            ).await;
            
            trace!("Thread {} completed in {:?}", idx, thread_start.elapsed());
            result
        })
    });
    
    // Wait for all search tasks to complete
    let results = join_all(search_tasks).await;
    
    // Merge results from all SSTables
    let mut all_results = Vec::new();
    let mut successful_searches = 0;
    let mut failed_searches = 0;
    
    for (idx, result) in results.into_iter().enumerate() {
        match result {
            Ok(Ok(mut sstable_results)) => {
                trace!("Thread {} returned {} results", idx, sstable_results.len());
                all_results.append(&mut sstable_results);
                successful_searches += 1;
            }
            Ok(Err(e)) => {
                warn!("Thread {} failed to search SSTable: {}", idx, e);
                failed_searches += 1;
            }
            Err(e) => {
                error!("Thread {} panicked: {}", idx, e);
                failed_searches += 1;
            }
        }
    }
    
    // Sort by score (higher is better) and take top-k
    all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    all_results.truncate(k);
    
    info!(
        "Parallel search completed in {:?}: {} successful, {} failed, {} results",
        start.elapsed(),
        successful_searches,
        failed_searches,
        all_results.len()
    );
    
    Ok(all_results)
}

/// Statistics for query execution
#[derive(Debug, Default)]
pub struct QueryStats {
    pub sstables_scanned: usize,
    pub sstables_pruned: usize,
    pub blocks_scanned: usize,
    pub blocks_pruned: usize,
    pub vectors_evaluated: usize,
    pub pruning_ratio: f64,
}

/// Advanced search with statistics
pub async fn search_with_stats(
    filesystem: &Arc<dyn FileSystem>,
    sstables: &[SStableMetadata],
    query_vector: &[f32],
    query_hilbert_key: Option<u64>,
    k: usize,
    distance_metric: &DistanceMetric,
) -> Result<(Vec<OptimizedSearchRecord>, QueryStats)> {
    let mut stats = QueryStats::default();
    let mut results = Vec::new();
    
    stats.sstables_scanned = sstables.len();
    
    for sstable in sstables {
        // Prune based on Hilbert range
        if let (Some(query_key), Some((min_key, max_key))) = 
            (query_hilbert_key, sstable.hilbert_range) {
            
            // Calculate distance to range
            let distance_to_range = if query_key < min_key {
                min_key - query_key
            } else if query_key > max_key {
                query_key - min_key
            } else {
                0
            };
            
            // Skip if too far
            if distance_to_range > 10000 {
                stats.sstables_pruned += 1;
                continue;
            }
        }
        
        // Search SSTable
        let sstable_results = search_sstable(
            filesystem,
            sstable,
            query_vector,
            k,
            distance_metric,
            None,
            None,  // candidate_ids
        ).await?;
        
        stats.vectors_evaluated += sstable_results.len();
        results.extend(sstable_results);
    }
    
    // Calculate pruning ratio
    stats.pruning_ratio = if stats.sstables_scanned > 0 {
        stats.sstables_pruned as f64 / stats.sstables_scanned as f64
    } else {
        0.0
    };
    
    // Sort and take top-k
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    results.truncate(k);
    
    info!(
        "Query completed: scanned {}/{} SSTables, pruned {:.1}%, evaluated {} vectors",
        stats.sstables_scanned - stats.sstables_pruned,
        stats.sstables_scanned,
        stats.pruning_ratio * 100.0,
        stats.vectors_evaluated
    );
    
    Ok((results, stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::filesystem::FilesystemFactory;

    #[tokio::test]
    async fn test_query_stats() {
        let stats = QueryStats {
            sstables_scanned: 10,
            sstables_pruned: 7,
            blocks_scanned: 30,
            blocks_pruned: 20,
            vectors_evaluated: 1000,
            pruning_ratio: 0.7,
        };
        
        assert_eq!(stats.sstables_scanned - stats.sstables_pruned, 3);
        assert_eq!(stats.pruning_ratio, 0.7);
    }
}