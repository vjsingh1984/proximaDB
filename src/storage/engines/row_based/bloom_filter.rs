//! LSM Bloom Filter Module
//! 
//! This module provides bloom filter functionality for LSM storage engine
//! with optimized memory usage and custom serialization support.

pub use crate::core::bloom::{
    BloomFilterStrategy,
    BloomFilterConfig,
    BloomStrategy,
    HashAlgorithm,
    MetadataBloomFilter,
    factory::BloomFilterFactory,
    strategies::{
        ByteAlignedBloomFilter,
        CompositeBloomFilter,
    }
};

use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use anyhow::Result;

/// Stats for bloom filter (avoiding HashMap for bincode compatibility)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BloomFilterStats {
    pub key_count: u64,
    pub metadata_columns: u64,
    pub total_keys: u64,
    pub key_lookups_saved: u64,
    pub metadata_queries_saved: u64,
}

/// Combined bloom filter for SSTable (keys + metadata)
/// Memory target: ~8MB per collection (down from ~40MB)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "SerializedSstableBloomFilter", into = "SerializedSstableBloomFilter")]
pub struct SstableBloomFilter {
    /// Key filter configuration
    pub key_filter_config: BloomFilterConfig,
    /// Key filter data
    pub key_filter_data: Vec<u8>,
    /// Metadata filter data  
    pub metadata_filter_data: Vec<u8>,
    /// Statistics
    pub stats: BloomFilterStats,
    /// Memory usage tracking
    #[serde(skip)]
    memory_usage: Option<usize>,
}

/// Serializable version of SstableBloomFilter to work around bincode limitations
#[derive(Debug, Serialize, Deserialize)]
pub struct SerializedSstableBloomFilter {
    // BloomFilterConfig fields flattened to avoid Option<f64> issues
    // strategy removed -  u8,
    bits_per_key: u32,
    false_positive_rate: f64,  // Use NaN for None
    expected_items: usize,
    enabled: bool,
    hash_algorithm: u8,
    
    // Filter data
    key_filter_data: Vec<u8>,
    metadata_filter_data: Vec<u8>,
    
    // Stats fields
    key_count: u64,
    metadata_columns: u64,
    total_keys: u64,
    key_lookups_saved: u64,
    metadata_queries_saved: u64,
}

/// Configuration for hierarchical bloom filters in SST files
/// Enables selective loading based on query patterns
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchicalBloomConfig {
    /// Global filters (loaded selectively)
    pub global_key_filter: BloomFilterConfig,
    pub global_metadata_filter: BloomFilterConfig,
    
    /// Block-level filters (lazy loaded)
    pub block_key_filter: BloomFilterConfig,
    pub block_metadata_filter: BloomFilterConfig,
    
    /// Smart thresholds for enabling hierarchical features
    pub block_count_threshold: usize,        // Enable per-block blooms if blocks > N
    pub metadata_column_threshold: usize,    // Enable metadata blooms if columns > N
}

impl HierarchicalBloomConfig {
    /// Create configuration optimized for specific SST level
    pub fn for_level(level: u8) -> Self {
        match level {
            // L0: Optimize for fast writes, minimal bloom overhead
            0 => Self {
                global_key_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::ByteAligned,
                    bits_per_key: 8,  // Lower memory usage for frequent writes
                    false_positive_rate: Some(0.01),
                    expected_items: 10_000,
                    enabled: true,
                    hash_algorithm: HashAlgorithm::XXHash,
                },
                global_metadata_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::Simple,
                    bits_per_key: 4,  // Minimal metadata filtering at L0
                    false_positive_rate: Some(0.05),
                    expected_items: 5_000,
                    enabled: false,  // Disable for write performance
                    hash_algorithm: HashAlgorithm::XXHash,
                },
                block_key_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::Simple,
                    bits_per_key: 6,
                    false_positive_rate: Some(0.02),
                    expected_items: 1_000,
                    enabled: false,  // No block-level blooms at L0
                    hash_algorithm: HashAlgorithm::XXHash,
                },
                block_metadata_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::Simple,
                    bits_per_key: 4,
                    false_positive_rate: Some(0.05),
                    expected_items: 500,
                    enabled: false,
                    hash_algorithm: HashAlgorithm::XXHash,
                },
                block_count_threshold: 100,     // Enable block blooms only for large L0 files
                metadata_column_threshold: 10,  // Enable metadata only with many columns
            },
            
            // L1-L2: Balanced performance for point queries and metadata filtering
            1..=2 => Self {
                global_key_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::ByteAligned,
                    bits_per_key: 10,
                    false_positive_rate: Some(0.005),
                    expected_items: 50_000,
                    enabled: true,
                    hash_algorithm: HashAlgorithm::XXHash,
                },
                global_metadata_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::Composite,
                    bits_per_key: 8,
                    false_positive_rate: Some(0.01),
                    expected_items: 25_000,
                    enabled: true,
                    hash_algorithm: HashAlgorithm::XXHash,
                },
                block_key_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::ByteAligned,
                    bits_per_key: 8,
                    false_positive_rate: Some(0.01),
                    expected_items: 2_000,
                    enabled: true,  // Enable block-level optimization
                    hash_algorithm: HashAlgorithm::XXHash,
                },
                block_metadata_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::Simple,
                    bits_per_key: 6,
                    false_positive_rate: Some(0.02),
                    expected_items: 1_000,
                    enabled: true,
                    hash_algorithm: HashAlgorithm::XXHash,
                },
                block_count_threshold: 20,      // Enable block blooms for moderate files
                metadata_column_threshold: 5,   // Enable metadata with fewer columns
            },
            
            // L3+: Optimize for read efficiency, highest bloom filter fidelity
            _ => Self {
                global_key_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::Composite,
                    bits_per_key: 12,  // High fidelity for deep levels
                    false_positive_rate: Some(0.002),
                    expected_items: 200_000,
                    enabled: true,
                    hash_algorithm: HashAlgorithm::CityHash,
                },
                global_metadata_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::Composite,
                    bits_per_key: 10,
                    false_positive_rate: Some(0.005),
                    expected_items: 100_000,
                    enabled: true,
                    hash_algorithm: HashAlgorithm::CityHash,
                },
                block_key_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::Composite,
                    bits_per_key: 10,
                    false_positive_rate: Some(0.005),
                    expected_items: 5_000,
                    enabled: true,
                    hash_algorithm: HashAlgorithm::CityHash,
                },
                block_metadata_filter: BloomFilterConfig {
                    // strategy removed -  BloomStrategy::ByteAligned,
                    bits_per_key: 8,
                    false_positive_rate: Some(0.01),
                    expected_items: 2_500,
                    enabled: true,
                    hash_algorithm: HashAlgorithm::CityHash,
                },
                block_count_threshold: 5,       // Enable block blooms for all files
                metadata_column_threshold: 2,   // Enable metadata with any columns
            }
        }
    }
    
    /// Create configuration for metadata-heavy workloads
    pub fn for_metadata_workload() -> Self {
        Self {
            global_key_filter: BloomFilterConfig {
                // strategy removed -  BloomStrategy::ByteAligned,
                bits_per_key: 10,
                false_positive_rate: Some(0.005),
                expected_items: 100_000,
                enabled: true,
                hash_algorithm: HashAlgorithm::XXHash,
            },
            global_metadata_filter: BloomFilterConfig {
                // strategy removed -  BloomStrategy::Composite,
                bits_per_key: 12,  // High fidelity for metadata filtering
                false_positive_rate: Some(0.002),
                expected_items: 50_000,
                enabled: true,
                hash_algorithm: HashAlgorithm::CityHash,
            },
            block_key_filter: BloomFilterConfig {
                // strategy removed -  BloomStrategy::Simple,
                bits_per_key: 6,
                false_positive_rate: Some(0.02),
                expected_items: 2_000,
                enabled: false,  // Focus on metadata, not per-block keys
                hash_algorithm: HashAlgorithm::XXHash,
            },
            block_metadata_filter: BloomFilterConfig {
                // strategy removed -  BloomStrategy::Composite,
                bits_per_key: 10,
                false_positive_rate: Some(0.005),
                expected_items: 1_000,
                enabled: true,
                hash_algorithm: HashAlgorithm::CityHash,
            },
            block_count_threshold: 1,       // Always enable for metadata workloads
            metadata_column_threshold: 1,   // Enable with any metadata
        }
    }
    
    /// Create minimal configuration for write-heavy workloads
    pub fn for_write_optimized() -> Self {
        Self {
            global_key_filter: BloomFilterConfig {
                // strategy removed -  BloomStrategy::Simple,
                bits_per_key: 6,  // Minimal overhead
                false_positive_rate: Some(0.02),
                expected_items: 10_000,
                enabled: true,
                hash_algorithm: HashAlgorithm::Murmur3,  // Fastest hash
            },
            global_metadata_filter: BloomFilterConfig {
                // strategy removed -  BloomStrategy::Simple,
                bits_per_key: 4,
                false_positive_rate: Some(0.05),
                expected_items: 5_000,
                enabled: false,  // Disable for maximum write speed
                hash_algorithm: HashAlgorithm::Murmur3,
            },
            block_key_filter: BloomFilterConfig {
                // strategy removed -  BloomStrategy::Simple,
                bits_per_key: 4,
                false_positive_rate: Some(0.05),
                expected_items: 500,
                enabled: false,
                hash_algorithm: HashAlgorithm::Murmur3,
            },
            block_metadata_filter: BloomFilterConfig {
                // strategy removed -  BloomStrategy::Simple,
                bits_per_key: 4,
                false_positive_rate: Some(0.05),
                expected_items: 250,
                enabled: false,
                hash_algorithm: HashAlgorithm::Murmur3,
            },
            block_count_threshold: 1000,    // Very high threshold
            metadata_column_threshold: 50,  // Very high threshold
        }
    }
}

impl SstableBloomFilter {
    /// Create a new SstableBloomFilter
    pub fn new(
        key_filter_config: BloomFilterConfig,
        key_filter_data: Vec<u8>,
        metadata_filter_data: Vec<u8>,
        stats: BloomFilterStats,
    ) -> Self {
        let memory_usage = std::mem::size_of::<Self>() + key_filter_data.len() + metadata_filter_data.len();
        Self {
            key_filter_config,
            key_filter_data,
            metadata_filter_data,
            stats,
            memory_usage: Some(memory_usage),
        }
    }
    /// Check if key might exist
    pub fn might_contain_key(&self, key: &str) -> Result<bool> {
        // Create SerializedBloomFilter structure for deserialization
        let serialized = crate::core::bloom::SerializedBloomFilter {
            strategy_type: self.key_filter_config.strategy,
            version: crate::core::bloom::SerializedBloomFilter::CURRENT_VERSION,
            config: self.key_filter_config.clone(),
            data: self.key_filter_data.clone(),
            metadata: HashMap::new(),
        };
        let filter = BloomFilterFactory::from_serialized(&serialized)?;
        Ok(filter.might_contain(key.as_bytes()))
    }
    
    /// Check if metadata might match using MetadataItem for type safety
    pub fn might_match_metadata(&self, _column: &str, _item: &crate::proto::proximadb::MetadataItem) -> Result<bool> {
        if self.metadata_filter_data.is_empty() {
            return Ok(false);
        }
        
        // Conservative approach: assume metadata might match
        // TODO: Implement proper deserialization once we fix BloomFilterConfig serialization
        Ok(true)
    }
    
    /// Check if entry might match query conditions
    pub fn might_match_query(
        &self, 
        key: Option<&str>, 
        metadata_conditions: Option<&HashMap<String, String>>
    ) -> Result<bool> {
        // Check key if provided
        if let Some(k) = key {
            if !self.might_contain_key(k)? {
                return Ok(false);
            }
        }
        
        // Check metadata conditions if provided
        if let Some(_conditions) = metadata_conditions {
            if !self.metadata_filter_data.is_empty() {
                // Conservative approach: assume metadata might match
                // TODO: Implement proper deserialization once we fix BloomFilterConfig serialization
                return Ok(true);
            }
        }
        
        Ok(true)
    }
    
    /// Get total size in bytes
    pub fn total_size_bytes(&self) -> usize {
        self.key_filter_data.len() + self.metadata_filter_data.len()
    }
    
    /// Get efficiency statistics
    pub fn efficiency_stats(&self) -> HashMap<String, u64> {
        let mut map = HashMap::new();
        map.insert("key_count".to_string(), self.stats.key_count);
        map.insert("metadata_columns".to_string(), self.stats.metadata_columns);
        map.insert("total_keys".to_string(), self.stats.total_keys);
        map.insert("key_lookups_saved".to_string(), self.stats.key_lookups_saved);
        map.insert("metadata_queries_saved".to_string(), self.stats.metadata_queries_saved);
        map
    }
    
    /// Get memory usage in bytes
    pub fn memory_usage_bytes(&self) -> usize {
        self.memory_usage.unwrap_or_else(|| {
            std::mem::size_of::<Self>() + 
            self.key_filter_data.len() + 
            self.metadata_filter_data.len()
        })
    }
    
    /// Check if within target memory limit (8MB)
    pub fn is_within_memory_target(&self) -> bool {
        self.memory_usage_bytes() < 8 * 1024 * 1024
    }
    
    
    /// Custom serialization using manual byte layout to avoid bincode issues
    pub fn serialize(&self) -> Result<Vec<u8>> {
        use std::io::Write;
        let mut buffer = Vec::new();
        
        // Write a simple header
        buffer.write_all(b"BF01")?; // Magic bytes + version
        
        // Write config
        let strategy_byte = match self.key_filter_config.strategy {
            BloomStrategy::BitPacked => 0u8,
            BloomStrategy::ByteAligned => 1u8,
            BloomStrategy::Simple => 2u8,
            BloomStrategy::Composite => 3u8,
        };
        buffer.write_all(&strategy_byte.to_le_bytes())?;
        buffer.write_all(&self.key_filter_config.bits_per_key.to_le_bytes())?;
        buffer.write_all(&self.key_filter_config.false_positive_rate.to_le_bytes())?;
        buffer.write_all(&self.key_filter_config.expected_items.to_le_bytes())?;
        buffer.write_all(&[if self.key_filter_config.enabled { 1u8 } else { 0u8 }])?;
        
        let hash_byte = match self.key_filter_config.hash_algorithm {
            HashAlgorithm::Murmur3 => 0u8,
            HashAlgorithm::XXHash => 1u8,
            HashAlgorithm::CityHash => 2u8,
        };
        buffer.write_all(&hash_byte.to_le_bytes())?;
        
        // Write stats
        buffer.write_all(&self.stats.key_count.to_le_bytes())?;
        buffer.write_all(&self.stats.metadata_columns.to_le_bytes())?;
        buffer.write_all(&self.stats.total_keys.to_le_bytes())?;
        buffer.write_all(&self.stats.key_lookups_saved.to_le_bytes())?;
        buffer.write_all(&self.stats.metadata_queries_saved.to_le_bytes())?;
        
        // Write data lengths and data
        buffer.write_all(&(self.key_filter_data.len() as u32).to_le_bytes())?;
        buffer.write_all(&self.key_filter_data)?;
        
        buffer.write_all(&(self.metadata_filter_data.len() as u32).to_le_bytes())?;
        buffer.write_all(&self.metadata_filter_data)?;
        
        Ok(buffer)
    }
    
    /// Custom deserialization using manual byte layout to avoid bincode issues
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        use std::io::Read;
        let mut cursor = std::io::Cursor::new(data);
        
        // Read and validate header
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic)?;
        if &magic != b"BF01" {
            return Err(anyhow::anyhow!("Invalid bloom filter format"));
        }
        
        // Read config
        let mut strategy_buf = [0u8; 1];
        cursor.read_exact(&mut strategy_buf)?;
        let strategy = match strategy_buf[0] {
            0 => BloomStrategy::BitPacked,
            1 => BloomStrategy::ByteAligned,
            2 => BloomStrategy::Simple,
            3 => BloomStrategy::Composite,
            _ => BloomStrategy::ByteAligned,
        };
        
        let mut bits_per_key_buf = [0u8; 4];
        cursor.read_exact(&mut bits_per_key_buf)?;
        let bits_per_key = u32::from_le_bytes(bits_per_key_buf);
        
        let mut fpr_buf = [0u8; 8];
        cursor.read_exact(&mut fpr_buf)?;
        let fpr = f64::from_le_bytes(fpr_buf);
        let false_positive_rate = if fpr.is_nan() { None } else { Some(fpr) };
        
        let mut expected_items_buf = [0u8; 8];
        cursor.read_exact(&mut expected_items_buf)?;
        let expected_items = usize::from_le_bytes(expected_items_buf);
        
        let mut enabled_buf = [0u8; 1];
        cursor.read_exact(&mut enabled_buf)?;
        let enabled = enabled_buf[0] != 0;
        
        let mut hash_buf = [0u8; 1];
        cursor.read_exact(&mut hash_buf)?;
        let hash_algorithm = match hash_buf[0] {
            0 => HashAlgorithm::Murmur3,
            1 => HashAlgorithm::XXHash,
            2 => HashAlgorithm::CityHash,
            _ => HashAlgorithm::Murmur3,
        };
        
        // Read stats
        let mut stats_buf = [0u8; 8];
        
        cursor.read_exact(&mut stats_buf)?;
        let key_count = u64::from_le_bytes(stats_buf);
        
        cursor.read_exact(&mut stats_buf)?;
        let metadata_columns = u64::from_le_bytes(stats_buf);
        
        cursor.read_exact(&mut stats_buf)?;
        let total_keys = u64::from_le_bytes(stats_buf);
        
        cursor.read_exact(&mut stats_buf)?;
        let key_lookups_saved = u64::from_le_bytes(stats_buf);
        
        cursor.read_exact(&mut stats_buf)?;
        let metadata_queries_saved = u64::from_le_bytes(stats_buf);
        
        // Read data
        let mut len_buf = [0u8; 4];
        cursor.read_exact(&mut len_buf)?;
        let key_data_len = u32::from_le_bytes(len_buf) as usize;
        
        let mut key_filter_data = vec![0u8; key_data_len];
        cursor.read_exact(&mut key_filter_data)?;
        
        cursor.read_exact(&mut len_buf)?;
        let meta_data_len = u32::from_le_bytes(len_buf) as usize;
        
        let mut metadata_filter_data = vec![0u8; meta_data_len];
        cursor.read_exact(&mut metadata_filter_data)?;
        
        // Create the structures
        let stats = BloomFilterStats {
            key_count,
            metadata_columns,
            total_keys,
            key_lookups_saved,
            metadata_queries_saved,
        };
        
        let key_filter_config = BloomFilterConfig {
            strategy,
            bits_per_key,
            false_positive_rate,
            expected_items,
            enabled,
            hash_algorithm,
        };
        
        Ok(Self::new(
            key_filter_config,
            key_filter_data,
            metadata_filter_data,
            stats,
        ))
    }
}

// Implement conversion for serde
impl From<SerializedSstableBloomFilter> for SstableBloomFilter {
    fn from(serialized: SerializedSstableBloomFilter) -> Self {
        let strategy = match serialized.strategy {
            0 => BloomStrategy::BitPacked,
            1 => BloomStrategy::ByteAligned,
            2 => BloomStrategy::Simple,
            3 => BloomStrategy::Composite,
            _ => BloomStrategy::ByteAligned,
        };
        
        let hash_algorithm = match serialized.hash_algorithm {
            0 => HashAlgorithm::Murmur3,
            1 => HashAlgorithm::XXHash,
            2 => HashAlgorithm::CityHash,
            _ => HashAlgorithm::Murmur3,
        };
        
        let false_positive_rate = if serialized.false_positive_rate.is_nan() {
            None
        } else {
            Some(serialized.false_positive_rate)
        };
        
        let memory_usage = std::mem::size_of::<Self>() + 
            serialized.key_filter_data.len() + 
            serialized.metadata_filter_data.len();
        
        Self {
            key_filter_config: BloomFilterConfig {
                strategy,
                bits_per_key: serialized.bits_per_key,
                false_positive_rate,
                expected_items: serialized.expected_items,
                enabled: serialized.enabled,
                hash_algorithm,
            },
            key_filter_data: serialized.key_filter_data,
            metadata_filter_data: serialized.metadata_filter_data,
            stats: BloomFilterStats {
                key_count: serialized.key_count,
                metadata_columns: serialized.metadata_columns,
                total_keys: serialized.total_keys,
                key_lookups_saved: serialized.key_lookups_saved,
                metadata_queries_saved: serialized.metadata_queries_saved,
            },
            memory_usage: Some(memory_usage),
        }
    }
}

impl From<SstableBloomFilter> for SerializedSstableBloomFilter {
    fn from(bf: SstableBloomFilter) -> Self {
        Self {
            strategy: match bf.key_filter_config.strategy {
                BloomStrategy::BitPacked => 0,
                BloomStrategy::ByteAligned => 1,
                BloomStrategy::Simple => 2,
                BloomStrategy::Composite => 3,
            },
            bits_per_key: bf.key_filter_config.bits_per_key,
            false_positive_rate: bf.key_filter_config.false_positive_rate,
            expected_items: bf.key_filter_config.expected_items,
            enabled: bf.key_filter_config.enabled,
            hash_algorithm: match bf.key_filter_config.hash_algorithm {
                HashAlgorithm::Murmur3 => 0,
                HashAlgorithm::XXHash => 1,
                HashAlgorithm::CityHash => 2,
            },
            key_filter_data: bf.key_filter_data,
            metadata_filter_data: bf.metadata_filter_data,
            key_count: bf.stats.key_count,
            metadata_columns: bf.stats.metadata_columns,
            total_keys: bf.stats.total_keys,
            key_lookups_saved: bf.stats.key_lookups_saved,
            metadata_queries_saved: bf.stats.metadata_queries_saved,
        }
    }
}