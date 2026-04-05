use crate::storage::cache::base::BaseCacheImpl;
use crate::storage::cache::traits::{BaseCache, CacheValue};
use anyhow::Result;
use std::collections::HashMap;

/// Index node that can be cached
#[derive(Debug, Clone)]
pub struct IndexNode {
    pub id: String,
    pub level: u32,
    pub children: Vec<String>,
    pub data: Vec<u8>,
}

// String already implements CacheKey elsewhere
impl CacheValue for IndexNode {
    fn size_bytes(&self) -> usize {
        self.data.len() + self.children.len() * 32 + 64 // Approximate size
    }
}

/// Specialized cache for index structures with hot path optimization
pub struct IndexNodeCache {
    base: BaseCacheImpl<String, IndexNode>,
}

impl IndexNodeCache {
    pub fn new(max_memory_mb: usize) -> Self {
        Self {
            base: BaseCacheImpl::new(max_memory_mb * 1024 * 1024),
        }
    }

    /// Delegate put_with_hooks to base cache
    pub async fn put_with_hooks(&self, key: String, value: IndexNode) {
        BaseCache::put_with_hooks(&self.base, key, value).await;
    }

    /// Delegate get_with_hooks to base cache
    pub async fn get_with_hooks(&self, key: &String) -> Option<IndexNode> {
        BaseCache::get_with_hooks(&self.base, key).await
    }

    /// Prefetch index path for a vector
    pub async fn prefetch_vector_index_path(&self, _vector_id: &str) {
        // Deferred: Implement prefetching logic based on index structure
        // This would traverse the index tree and cache hot nodes
    }

    /// Invalidate a cached index node
    pub async fn invalidate(&self, key: &str) -> bool {
        BaseCache::invalidate(&self.base, &key.to_string()).await
    }

    /// Get cache metrics
    pub fn metrics(&self) -> &crate::storage::traits::UnifiedMetricsCollector {
        self.base.metrics()
    }
}

// ========================================================================================
// SSTable Index Cache Operations - Extending IndexNodeCache for SST Engine Integration
// ========================================================================================

/// SSTable index entry with block metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SstIndexEntry {
    pub key: String,
    pub block_offset: u64,
    pub block_size: usize,
    pub min_key: String,
    pub max_key: String,
    pub vector_count: usize,
    pub bloom_filter_offset: Option<u64>,
}

/// Complete SSTable index structure
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SstableIndex {
    pub file_path: String,
    pub entries: Vec<SstIndexEntry>,
    pub total_blocks: usize,
    pub total_vectors: usize,
    pub metadata_stats: HashMap<String, MetadataStats>,
}

/// Metadata statistics for predicate pushdown
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetadataStats {
    pub min_value: serde_json::Value,
    pub max_value: serde_json::Value,
    pub null_count: usize,
    pub distinct_count: usize,
}

impl CacheValue for SstableIndex {
    fn size_bytes(&self) -> usize {
        // Estimate: entries * avg_entry_size + metadata overhead
        self.entries.len() * 128 + 256
    }
}

impl IndexNodeCache {
    /// Cache an SSTable index
    pub async fn cache_sstable_index(&self, file_path: &str, index: SstableIndex) -> Result<()> {
        // Convert SSTable index to IndexNode for storage
        let index_data = bincode::serialize(&index)?;

        let node = IndexNode {
            id: format!("sst_index_{}", file_path),
            level: 0, // SSTable indices are flat
            children: index.entries.iter().map(|e| format!("{:?}", e)).collect(),
            data: index_data,
        };

        self.put_with_hooks(node.id.clone(), node).await;
        Ok(())
    }

    /// Retrieve an SSTable index from cache
    pub async fn get_sstable_index(&self, file_path: &str) -> Option<SstableIndex> {
        let key = format!("sst_index_{}", file_path);
        let node = self.get_with_hooks(&key).await?;

        // Deserialize from node data
        bincode::deserialize(&node.data).ok()
    }

    /// Cache multiple SSTable indices as a batch
    pub async fn cache_sstable_indices_batch(
        &self,
        indices: Vec<(String, SstableIndex)>,
    ) -> Result<()> {
        for (file_path, index) in indices {
            self.cache_sstable_index(&file_path, index).await?;
        }
        Ok(())
    }

    /// Get indices for multiple SSTable files
    pub async fn get_sstable_indices(
        &self,
        file_paths: &[String],
    ) -> HashMap<String, SstableIndex> {
        let mut results = HashMap::new();

        for file_path in file_paths {
            if let Some(index) = self.get_sstable_index(file_path).await {
                results.insert(file_path.clone(), index);
            }
        }

        results
    }

    /// Find blocks that might contain a specific key
    pub async fn find_blocks_for_key(
        &self,
        file_path: &str,
        search_key: &str,
    ) -> Option<Vec<SstIndexEntry>> {
        let index = self.get_sstable_index(file_path).await?;

        // Binary search or range scan to find relevant blocks
        let mut matching_blocks = Vec::new();
        for entry in index.entries {
            if entry.min_key.as_str() <= search_key && entry.max_key.as_str() >= search_key {
                matching_blocks.push(entry);
            }
        }

        if matching_blocks.is_empty() {
            None
        } else {
            Some(matching_blocks)
        }
    }

    /// Cache hot index entries separately for faster access
    pub async fn cache_hot_entries(&self, file_path: &str, hot_keys: Vec<String>) -> Result<()> {
        if let Some(index) = self.get_sstable_index(file_path).await {
            for key in hot_keys {
                // Find and cache individual entries
                for entry in &index.entries {
                    if entry.key == key {
                        let entry_key = format!("sst_entry_{}_{}", file_path, key);
                        let entry_data = bincode::serialize(&entry)?;

                        let node = IndexNode {
                            id: entry_key.clone(),
                            level: 1, // Individual entries are level 1
                            children: vec![],
                            data: entry_data,
                        };

                        self.put_with_hooks(entry_key, node).await;
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}
