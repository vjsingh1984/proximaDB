//! # Vector Store Cache Module
//!
//! Specialized cache implementation for high-dimensional vector data with
//! collection-aware partitioning and batch operation optimizations.
//!
//! ## Design Philosophy
//!
//! While this module provides in-memory caching for vectors, it's being phased
//! out in favor of OS page cache + zero-copy I/O for several reasons:
//!
//! 1. **Memory Efficiency**: High-dimensional vectors (768-1536 dims) consume
//!    significant memory. OS page cache manages this better.
//!
//! 2. **Zero-Copy Benefits**: Direct memory mapping eliminates serialization
//!    overhead and reduces CPU usage.
//!
//! 3. **Automatic Eviction**: OS kernel's LRU page eviction is more sophisticated
//!    than application-level cache eviction.
//!
//! ## Migration Path
//!
//! ```rust
//! // Old approach (VectorStore)
//! let cache = VectorStore::new(1024);
//! cache.put("vec_123", vector).await;
//!
//! // New approach (ZeroCopyIOSystem)
//! let zero_copy = ZeroCopyIOSystem::new();
//! zero_copy.mmap_file("vectors/vec_123.bin")?;
//! ```

use crate::proto::proximadb::VectorRecord;
use crate::storage::cache::base::BaseCacheImpl;
use crate::storage::cache::traits::{BaseCache, CacheKey, CacheValue};
use anyhow::Result;
use crate::utils::encoding::{base64_encode, base64_decode};
use serde::{Deserialize, Serialize};

/// DEPRECATED: VectorStore is being phased out in favor of OS page cache + zero-copy system
///
/// # Deprecation Rationale
/// - High-dimensional vectors benefit more from OS page cache than in-memory caching
/// - OS can handle pages optimally based on file access patterns
/// - Zero-copy system provides file-specific metadata caching
/// - Reduces memory pressure for large vector datasets
///
/// Use `ZeroCopyIOSystem` with filename-based cache keys instead.

/// Partitioned key for collection-aware storage
///
/// ## Purpose:
///
/// Enables collection-level isolation in the cache, preventing one collection's
/// vectors from evicting another's. This is critical for multi-tenant scenarios.
///
/// ## Key Format:
///
/// The key combines collection_id and vector_id to create a unique identifier:
/// - `collection_id`: Namespace for isolation
/// - `vector_id`: Unique within the collection
///
/// Example: `{"collection_id": "products", "vector_id": "sku_12345"}`
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionedVectorKey {
    pub collection_id: String,
    pub vector_id: String,
}

impl PartitionedVectorKey {
    pub fn new(collection_id: String, vector_id: String) -> Self {
        Self {
            collection_id,
            vector_id,
        }
    }
}

impl CacheKey for String {}
impl CacheKey for PartitionedVectorKey {}

impl CacheValue for VectorRecord {
    fn size_bytes(&self) -> usize {
        // Calculate actual size based on vector dimensions
        // Each f32 is 4 bytes
        let vector_size = self.vector.len() * 4;

        // Estimate metadata size (can't access private fields)
        // Assume each metadata item is ~50 bytes on average
        let metadata_size = self.metadata.len() * 50;

        // Add size of id string
        let id_size = self.id.len();

        // Total: vector data + metadata + id + struct overhead
        vector_size + metadata_size + id_size + 64
    }
}

/// Specialized cache for vector data with optimizations for batch operations and collection partitioning
#[deprecated(
    since = "0.1.7",
    note = "VectorStore deprecated in favor of OS page cache + ZeroCopyIOSystem. Use zero-copy system with filename-based keys for better memory efficiency on high-dimensional vectors."
)]
pub struct VectorStore {
    /// Collection identifier for partitioning (optional for backward compatibility)
    collection_id: Option<String>,
    /// Base cache with partitioned keys when collection_id is set
    base: BaseCacheImpl<String, VectorRecord>,
    /// Partitioned base cache for collection-aware operations
    partitioned_base: Option<BaseCacheImpl<PartitionedVectorKey, VectorRecord>>,
}

impl VectorStore {
    pub fn new(max_memory_mb: usize) -> Self {
        Self {
            collection_id: None,
            base: BaseCacheImpl::new(max_memory_mb),
            partitioned_base: None,
        }
    }

    /// Create a new VectorStore for a specific collection
    pub fn new_with_collection(collection_id: String, max_memory_mb: usize) -> Self {
        Self {
            collection_id: Some(collection_id),
            base: BaseCacheImpl::new(0), // Unused when partitioned
            partitioned_base: Some(BaseCacheImpl::new(max_memory_mb)),
        }
    }

    /// Batch get operation optimized for locality with collection awareness
    pub async fn batch_get(&self, ids: &[String]) -> Vec<Option<VectorRecord>> {
        let mut results = Vec::with_capacity(ids.len());

        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                for id in ids {
                    let key = PartitionedVectorKey::new(coll_id.clone(), id.clone());
                    results.push(partitioned.get_with_hooks(&key).await);
                }
            } else {
                // Fallback if partitioned base not initialized
                return vec![None; ids.len()];
            }
        } else {
            // Use regular cache
            for id in ids {
                results.push(self.base.get_with_hooks(id).await);
            }
        }

        results
    }

    /// Batch put operation with collection awareness
    pub async fn batch_put(&self, records: Vec<(String, VectorRecord)>) {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                for (id, record) in records {
                    let key = PartitionedVectorKey::new(coll_id.clone(), id);
                    partitioned.put_with_hooks(key, record).await;
                }
            }
        } else {
            // Use regular cache
            for (id, record) in records {
                self.base.put_with_hooks(id, record).await;
            }
        }
    }

    /// Prefetch vectors that are likely to be accessed together
    pub async fn similarity_prefetch(&self, _query_vector: &[f32], _k: usize) {
        // TODO: Implement similarity-based prefetching
        // This would use an index to find similar vectors and prefetch them
    }

    /// Resize the cache
    pub async fn resize(&self, _new_size_mb: usize) -> anyhow::Result<()> {
        // TODO: Implement cache resizing
        Ok(())
    }

    /// Clear all cache entries
    pub async fn clear_all(&self) -> anyhow::Result<()> {
        // TODO: Implement cache clearing
        Ok(())
    }

    /// Check if a key exists in the cache
    pub async fn contains(&self, key: &str) -> bool {
        self.get(key).await.is_some()
    }

    /// Get a vector from the cache with collection awareness
    pub async fn get(&self, key: &str) -> Option<VectorRecord> {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                let partitioned_key = PartitionedVectorKey::new(coll_id.clone(), key.to_string());
                partitioned.get_with_hooks(&partitioned_key).await
            } else {
                None
            }
        } else {
            // Use regular cache
            self.base.get_with_hooks(&key.to_string()).await
        }
    }

    /// Get a vector from the cache with hooks (alias for compatibility)
    pub async fn get_with_hooks(&self, key: &String) -> Option<VectorRecord> {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                let partitioned_key = PartitionedVectorKey::new(coll_id.clone(), key.clone());
                partitioned.get_with_hooks(&partitioned_key).await
            } else {
                None
            }
        } else {
            // Use regular cache
            self.base.get_with_hooks(key).await
        }
    }

    /// Put a vector in the cache with collection awareness
    pub async fn put(&self, key: String, value: VectorRecord) {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                let partitioned_key = PartitionedVectorKey::new(coll_id.clone(), key);
                partitioned.put_with_hooks(partitioned_key, value).await;
            }
        } else {
            // Use regular cache
            self.base.put_with_hooks(key, value).await;
        }
    }

    /// Put a vector in the cache with hooks (alias for compatibility)
    pub async fn put_with_hooks(&self, key: String, value: VectorRecord) {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                let partitioned_key = PartitionedVectorKey::new(coll_id.clone(), key);
                partitioned.put_with_hooks(partitioned_key, value).await;
            }
        } else {
            // Use regular cache
            self.base.put_with_hooks(key, value).await;
        }
    }

    /// Access metrics from base cache
    pub fn metrics(&self) -> &crate::storage::cache::metrics::CacheMetrics {
        if let Some(ref partitioned) = self.partitioned_base {
            partitioned.metrics()
        } else {
            self.base.metrics()
        }
    }

    /// Invalidate a cache entry with collection awareness
    pub async fn invalidate(&self, key: &str) -> bool {
        if let Some(ref coll_id) = self.collection_id {
            // Use partitioned cache
            if let Some(ref partitioned) = self.partitioned_base {
                let partitioned_key = PartitionedVectorKey::new(coll_id.clone(), key.to_string());
                BaseCache::invalidate(partitioned, &partitioned_key).await
            } else {
                false
            }
        } else {
            // Use regular cache
            BaseCache::invalidate(&self.base, &key.to_string()).await
        }
    }
}

// ========================================================================================
// SSTable Block Cache Operations - Extending VectorStore for SST Engine Integration
// ========================================================================================

/// Key for SSTable block caching
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct SstBlockKey {
    pub file_path: String,
    pub block_offset: u64,
    pub block_size: usize,
}

impl SstBlockKey {
    pub fn new(file_path: String, block_offset: u64, block_size: usize) -> Self {
        Self {
            file_path,
            block_offset,
            block_size,
        }
    }

    /// Convert to cache key string
    pub fn to_cache_key(&self) -> String {
        format!("sst_block_{}_{}", self.file_path, self.block_offset)
    }
}

/// Compressed block data wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedBlock {
    pub data: Vec<u8>,
    pub compression: CompressionType,
    pub uncompressed_size: usize,
    pub vector_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Zstd,
    Lz4,
    Snappy,
}

impl VectorStore {
    /// Cache an SSTable block as compressed data
    pub async fn cache_compressed_block(
        &self,
        key: &SstBlockKey,
        compressed_data: Vec<u8>,
        compression: CompressionType,
        uncompressed_size: usize,
    ) -> Result<()> {
        // For now, store as a special vector record with compressed data in metadata
        // In production, would have a separate compressed block cache
        let cache_key = key.to_cache_key();

        // Create a placeholder vector record that holds the compressed block
        let block_record = VectorRecord {
            id: cache_key.clone(),
            vector: vec![], // No vector data, just using as container
            metadata: vec![
                crate::proto::proximadb::MetadataItem {
                    key: "compressed_block".to_string(),
                    value: Some(crate::proto::proximadb::metadata_item::Value::StringValue(
                        base64_encode(&compressed_data),
                    )),
                },
                crate::proto::proximadb::MetadataItem {
                    key: "compression_type".to_string(),
                    value: Some(crate::proto::proximadb::metadata_item::Value::StringValue(
                        format!("{:?}", compression),
                    )),
                },
                crate::proto::proximadb::MetadataItem {
                    key: "uncompressed_size".to_string(),
                    value: Some(crate::proto::proximadb::metadata_item::Value::NumberValue(
                        uncompressed_size as f64,
                    )),
                },
            ],
            version: None,
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: None,
            expires_at: None,
            quantized_vector: None,
            source: None,
        };

        self.put(cache_key, block_record).await;
        Ok(())
    }

    /// Retrieve a compressed block from cache
    pub async fn get_compressed_block(&self, key: &SstBlockKey) -> Option<CompressedBlock> {
        let cache_key = key.to_cache_key();
        let record = self.get(&cache_key).await?;

        // Extract compressed data from metadata
        let mut compressed_data = None;
        let mut compression_type = CompressionType::None;
        let mut uncompressed_size = 0usize;

        for item in &record.metadata {
            match item.key.as_str() {
                "compressed_block" => {
                    if let Some(crate::proto::proximadb::metadata_item::Value::StringValue(data)) =
                        &item.value
                    {
                        compressed_data = base64_decode(data).ok();
                    }
                }
                "compression_type" => {
                    if let Some(crate::proto::proximadb::metadata_item::Value::StringValue(ctype)) =
                        &item.value
                    {
                        compression_type = match ctype.as_str() {
                            "Zstd" => CompressionType::Zstd,
                            "Lz4" => CompressionType::Lz4,
                            "Snappy" => CompressionType::Snappy,
                            _ => CompressionType::None,
                        };
                    }
                }
                "uncompressed_size" => {
                    if let Some(crate::proto::proximadb::metadata_item::Value::NumberValue(size)) =
                        &item.value
                    {
                        uncompressed_size = *size as usize;
                    }
                }
                _ => {}
            }
        }

        compressed_data.map(|data| CompressedBlock {
            data,
            compression: compression_type,
            uncompressed_size,
            vector_count: 0, // Would be extracted from block header
        })
    }

    /// Cache decoded vectors from an SSTable block
    pub async fn cache_block_vectors(
        &self,
        key: &SstBlockKey,
        vectors: Vec<VectorRecord>,
    ) -> Result<()> {
        // Store each vector with a composite key
        for (idx, vector) in vectors.into_iter().enumerate() {
            let vector_key = format!("{}_v{}", key.to_cache_key(), idx);
            self.put(vector_key, vector).await;
        }
        Ok(())
    }

    /// Get cached vectors from an SSTable block
    pub async fn get_block_vectors(&self, key: &SstBlockKey, count: usize) -> Vec<VectorRecord> {
        let mut vectors = Vec::with_capacity(count);
        for idx in 0..count {
            let vector_key = format!("{}_v{}", key.to_cache_key(), idx);
            if let Some(vector) = self.get(&vector_key).await {
                vectors.push(vector);
            } else {
                break; // Stop if we don't find a vector
            }
        }
        vectors
    }

    /// Invalidate all cached data for an SSTable file
    pub async fn invalidate_sstable(&self, file_path: &str) -> Result<()> {
        // In a real implementation, would track all keys for a file
        // For now, this is a placeholder
        tracing::debug!("Invalidating SSTable cache for: {}", file_path);
        Ok(())
    }
}
