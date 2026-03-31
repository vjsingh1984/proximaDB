/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Shuffle Exchange for Distributed Query Execution
//!
//! This module implements the shuffle exchange protocol for redistributing data
//! across nodes during distributed query execution, enabling efficient cross-shard
//! joins and aggregations.
//!
//! ## Shuffle Exchange Protocol
//!
//! 1. **Partition**: Data is partitioned by shuffle key across nodes
//! 2. **Exchange**: Data is sent to target nodes via gRPC
//! 3. **Sort**: Received data is sorted by shuffle key
//! 4. **Process**: Local processing on sorted data
//!
//! ## Architecture
//!
//! ```text
//! Node 1: [A, B] → Shuffle → Node 2: [C, D]
//! Node 2: [C, D] → Shuffle → Node 3: [E, F]
//! Node 3: [E, F] → Shuffle → Node 1: [A, B]
//! ```
//!
//! ## Use Cases
//!
//! - **Distributed Joins**: Redistribute data on join keys
//! - **Distributed Aggregation**: Redistribute for GROUP BY
//! - **Distributed Sorting**: Global sort across nodes
//! - **Data Skew Mitigation**: Handle uneven data distribution

use crate::core::error::ProximaDBError;
use std::collections::HashMap;
use tracing::{debug, info};

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Configuration for shuffle exchange
#[derive(Debug, Clone)]
pub struct ShuffleConfig {
    /// Number of nodes participating in shuffle
    pub num_nodes: usize,

    /// Batch size for data transfer
    pub batch_size: usize,

    /// Compression enabled for transfer
    pub compression_enabled: bool,

    /// Maximum shuffle size in bytes
    pub max_shuffle_size: usize,
}

impl Default for ShuffleConfig {
    fn default() -> Self {
        Self {
            num_nodes: 3,
            batch_size: 1000,
            compression_enabled: true,
            max_shuffle_size: 1_000_000_000, // 1GB
        }
    }
}

/// Shuffle partition key
#[derive(Debug, Clone)]
pub enum ShuffleKey {
    /// String key
    String(String),

    /// Integer key
    Integer(i64),

    /// Float key (rounded for hashing) - manually implement Hash/Eq
    Float(f64),

    /// Composite key (multiple fields)
    Composite(Vec<String>),
}

// Implement PartialEq for ShuffleKey
impl PartialEq for ShuffleKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ShuffleKey::String(a), ShuffleKey::String(b)) => a == b,
            (ShuffleKey::Integer(a), ShuffleKey::Integer(b)) => a == b,
            (ShuffleKey::Float(a), ShuffleKey::Float(b)) => {
                // Float comparison with NaN handling
                if a.is_nan() && b.is_nan() {
                    true
                } else {
                    a == b
                }
            }
            (ShuffleKey::Composite(a), ShuffleKey::Composite(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for ShuffleKey {}

// Implement PartialOrd for ShuffleKey (required for sorting)
impl PartialOrd for ShuffleKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (ShuffleKey::String(a), ShuffleKey::String(b)) => a.partial_cmp(b),
            (ShuffleKey::Integer(a), ShuffleKey::Integer(b)) => a.partial_cmp(b),
            (ShuffleKey::Float(a), ShuffleKey::Float(b)) => a.partial_cmp(b),
            (ShuffleKey::Composite(a), ShuffleKey::Composite(b)) => {
                // Lexicographic comparison for composite keys
                a.partial_cmp(b)
            }
            // Define cross-type ordering for consistency: String < Integer < Float < Composite
            (ShuffleKey::String(_), _) => Some(std::cmp::Ordering::Less),
            (_, ShuffleKey::String(_)) => Some(std::cmp::Ordering::Greater),
            (ShuffleKey::Integer(_), ShuffleKey::Float(_)) => Some(std::cmp::Ordering::Less),
            (ShuffleKey::Integer(_), ShuffleKey::Composite(_)) => Some(std::cmp::Ordering::Less),
            (ShuffleKey::Float(_), ShuffleKey::Integer(_)) => Some(std::cmp::Ordering::Greater),
            (ShuffleKey::Float(_), ShuffleKey::Composite(_)) => Some(std::cmp::Ordering::Less),
            (ShuffleKey::Composite(_), _) => Some(std::cmp::Ordering::Greater),
        }
    }
}

impl Ord for ShuffleKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
    }
}

// Implement Hash for ShuffleKey
impl std::hash::Hash for ShuffleKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            ShuffleKey::String(s) => s.hash(state),
            ShuffleKey::Integer(i) => i.hash(state),
            ShuffleKey::Float(f) => {
                // Hash the bit representation for floats
                f.to_bits().hash(state)
            }
            ShuffleKey::Composite(keys) => {
                for key in keys {
                    key.hash(state);
                }
            }
        }
    }
}

/// Shuffle data block
#[derive(Debug, Clone)]
pub struct ShuffleBlock {
    /// Partition key for this block
    pub key: ShuffleKey,

    /// Target node for this block
    pub target_node: String,

    /// Data records (serialized)
    pub data: Vec<Vec<u8>>,

    /// Block size in bytes
    pub size_bytes: usize,
}

/// Shuffle exchange coordinator
pub struct ShuffleExchange {
    config: ShuffleConfig,

    /// Local node ID
    local_node_id: String,

    /// Available nodes
    available_nodes: Vec<String>,
}

impl ShuffleExchange {
    /// Create a new shuffle exchange coordinator
    pub fn new(config: ShuffleConfig, local_node_id: String, available_nodes: Vec<String>) -> Self {
        Self {
            config,
            local_node_id,
            available_nodes,
        }
    }

    /// Partition data by shuffle key
    ///
    /// # Arguments
    ///
    /// * `data` - Data to partition (vector of (shuffle_key, record))
    /// * `partition_fn` - Function to extract partition key from record
    ///
    /// # Returns
    ///
    /// Vector of shuffle blocks, one per target node
    pub fn partition_data<T>(&self, data: Vec<(ShuffleKey, T)>) -> Result<Vec<ShuffleBlock>>
    where
        T: serde::Serialize,
    {
        info!(
            "Partitioning {} records across {} nodes",
            data.len(),
            self.config.num_nodes
        );

        // Create partition buckets
        let mut partitions: Vec<Vec<(ShuffleKey, Vec<u8>)>> =
            vec![Vec::new(); self.config.num_nodes];

        // Partition each record
        for (key, record) in data {
            // Serialize record
            let serialized = bincode::serialize(&record)
                .map_err(|e| ProximaDBError::Internal(format!("Serialization error: {}", e)))?;

            // Determine target partition
            let partition_id = self.get_partition_id(&key);

            partitions[partition_id].push((key, serialized));
        }

        // Create shuffle blocks
        let mut blocks = Vec::new();

        for (partition_id, partition_data) in partitions.into_iter().enumerate() {
            if partition_data.is_empty() {
                continue;
            }

            let target_node = self.available_nodes.get(partition_id).ok_or_else(|| {
                ProximaDBError::Internal(format!("Invalid partition ID: {}", partition_id))
            })?;

            // Extract keys and data
            let keys: Vec<ShuffleKey> = partition_data.iter().map(|(k, _)| k.clone()).collect();

            let data_bytes: Vec<Vec<u8>> = partition_data.into_iter().map(|(_, d)| d).collect();

            let total_size = data_bytes.iter().map(|d| d.len()).sum();

            blocks.push(ShuffleBlock {
                key: keys[0].clone(), // Use first key as representative
                target_node: target_node.clone(),
                data: data_bytes,
                size_bytes: total_size,
            });
        }

        Ok(blocks)
    }

    /// Get partition ID for a shuffle key
    fn get_partition_id(&self, key: &ShuffleKey) -> usize {
        let hash = match key {
            ShuffleKey::String(s) => self.hash_string(s),
            ShuffleKey::Integer(i) => self.hash_int(*i),
            ShuffleKey::Float(f) => self.hash_float(*f),
            ShuffleKey::Composite(keys) => {
                // Hash composite key by combining field hashes
                let mut hash: u64 = 0;
                for key in keys {
                    hash = hash.wrapping_add(self.hash_string(key));
                }
                hash
            }
        };

        (hash % self.config.num_nodes as u64) as usize
    }

    /// Hash string value
    fn hash_string(&self, s: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;

        let mut hasher = DefaultHasher::new();
        hasher.write(s.as_bytes());
        hasher.finish()
    }

    /// Hash integer value
    fn hash_int(&self, i: i64) -> u64 {
        i as u64
    }

    /// Hash float value (rounded to handle floating point precision)
    fn hash_float(&self, f: f64) -> u64 {
        f.to_bits()
    }

    /// Execute shuffle exchange
    ///
    /// # Arguments
    ///
    /// * `blocks` - Shuffle blocks to distribute
    /// * `send_fn` - Function to send data to target node
    ///
    /// # Returns
    ///
    /// Map of target node → sent block size
    pub async fn execute_shuffle<F>(
        &self,
        blocks: Vec<ShuffleBlock>,
        mut send_fn: F,
    ) -> Result<HashMap<String, usize>>
    where
        F: FnMut(String, Vec<Vec<u8>>) -> Result<usize>,
    {
        info!("Executing shuffle exchange: {} blocks", blocks.len());

        let mut sent_sizes = HashMap::new();

        for block in blocks {
            if block.target_node == self.local_node_id {
                // Keep local data
                debug!(
                    "Keeping local block: {} records, {} bytes",
                    block.data.len(),
                    block.size_bytes
                );
                continue;
            }

            // Send to target node
            debug!(
                "Sending block to {}: {} records, {} bytes",
                block.target_node,
                block.data.len(),
                block.size_bytes
            );

            let size = send_fn(block.target_node.clone(), block.data)?;
            sent_sizes.insert(block.target_node, size);
        }

        Ok(sent_sizes)
    }

    /// Receive shuffled data from other nodes
    ///
    /// # Arguments
    ///
    /// * `receive_fn` - Function to receive data from other nodes
    ///
    /// # Returns
    ///
    /// Received data blocks
    pub async fn receive_shuffled_data<F>(&self, mut receive_fn: F) -> Result<Vec<Vec<u8>>>
    where
        F: FnMut() -> Result<Vec<Vec<u8>>>,
    {
        debug!("Receiving shuffled data from other nodes");

        let mut received_data = Vec::new();

        // Receive from each node (excluding self)
        for node in &self.available_nodes {
            if node == &self.local_node_id {
                continue;
            }

            match receive_fn() {
                Ok(data) => {
                    debug!("Received {} records from {}", data.len(), node);
                    received_data.extend(data);
                }
                Err(e) => {
                    debug!("No data received from {}: {:?}", node, e);
                }
            }
        }

        info!(
            "Shuffle exchange complete: received {} records",
            received_data.len()
        );

        Ok(received_data)
    }

    /// Sort received data by shuffle key
    ///
    /// # Arguments
    ///
    /// * `data` - Received data blocks
    /// * `key_fn` - Function to extract sort key from record
    ///
    /// # Returns
    ///
    /// Sorted data
    pub fn sort_data<T>(
        &self,
        data: Vec<Vec<u8>>,
        key_fn: impl Fn(&T) -> ShuffleKey + Clone,
    ) -> Result<Vec<T>>
    where
        T: serde::de::DeserializeOwned,
    {
        info!("Sorting {} records by shuffle key", data.len());

        let mut records: Vec<T> = Vec::with_capacity(data.len());

        // Deserialize records
        for bytes in data {
            let record: T = bincode::deserialize(&bytes)
                .map_err(|e| ProximaDBError::Internal(format!("Deserialization error: {}", e)))?;
            records.push(record);
        }

        // Sort by key
        records.sort_by(|a, b| {
            let key_a = key_fn(a);
            let key_b = key_fn(b);
            key_a
                .partial_cmp(&key_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(records)
    }

    /// Estimate shuffle cost
    ///
    /// # Arguments
    ///
    /// * `data_size_bytes` - Total size of data to shuffle
    ///
    /// # Returns
    ///
    /// Estimated cost in bytes transferred
    pub fn estimate_shuffle_cost(&self, data_size_bytes: usize) -> usize {
        // Each node receives roughly 1/N of the data (ideal case)
        // Plus overhead for serialization and headers

        let data_per_node = data_size_bytes / self.config.num_nodes;
        let overhead = data_size_bytes * 10 / 100; // 10% overhead

        data_per_node + overhead
    }

    /// Create shuffle key from join columns
    ///
    /// # Arguments
    ///
    /// * `join_columns` - Values of join columns
    ///
    /// # Returns
    ///
    /// Shuffle key for partitioning
    pub fn create_join_key(join_columns: &[serde_json::Value]) -> Result<ShuffleKey> {
        if join_columns.len() == 1 {
            // Single column join
            match &join_columns[0] {
                serde_json::Value::String(s) => Ok(ShuffleKey::String(s.clone())),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Ok(ShuffleKey::Integer(i))
                    } else if let Some(f) = n.as_f64() {
                        Ok(ShuffleKey::Float(f))
                    } else {
                        Err(ProximaDBError::Internal(
                            "Invalid number value for shuffle key".to_string(),
                        ))
                    }
                }
                _ => Err(ProximaDBError::Internal(
                    "Unsupported JSON type for shuffle key".to_string(),
                )),
            }
        } else {
            // Multi-column join
            let keys: Vec<String> = join_columns
                .iter()
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".to_string()))
                .collect();

            Ok(ShuffleKey::Composite(keys))
        }
    }
}

/// Shuffle exchange statistics
#[derive(Debug, Clone)]
pub struct ShuffleStats {
    /// Number of records shuffled
    pub records_shuffled: usize,

    /// Total bytes transferred
    pub bytes_transferred: usize,

    /// Time taken for shuffle (milliseconds)
    pub shuffle_time_ms: u64,

    /// Number of target nodes
    pub num_target_nodes: usize,

    /// Data skew (ratio of largest to smallest partition)
    pub data_skew: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shuffle_config_default() {
        let config = ShuffleConfig::default();
        assert_eq!(config.num_nodes, 3);
        assert_eq!(config.batch_size, 1000);
        assert!(config.compression_enabled);
    }

    #[test]
    fn test_partition_by_string_key() {
        let config = ShuffleConfig::default();
        let exchange = ShuffleExchange::new(
            config,
            "node1".to_string(),
            vec![
                "node1".to_string(),
                "node2".to_string(),
                "node3".to_string(),
            ],
        );

        let data = vec![
            (ShuffleKey::String("key_a".to_string()), vec![1u8]),
            (ShuffleKey::String("key_b".to_string()), vec![2u8]),
            (ShuffleKey::String("key_c".to_string()), vec![3u8]),
        ];

        let blocks = exchange.partition_data(data).unwrap();

        assert!(!blocks.is_empty());
        // All blocks should target one of the nodes
        for block in &blocks {
            assert!(vec!["node1", "node2", "node3"].contains(&block.target_node.as_str()));
        }
    }

    #[test]
    fn test_hash_string() {
        let config = ShuffleConfig::default();
        let exchange = ShuffleExchange::new(
            config,
            "node1".to_string(),
            vec![
                "node1".to_string(),
                "node2".to_string(),
                "node3".to_string(),
            ],
        );

        let hash1 = exchange.hash_string("test");
        let hash2 = exchange.hash_string("test");
        let hash3 = exchange.hash_string("different");

        assert_eq!(hash1, hash2); // Same input → same hash
        assert_ne!(hash1, hash3); // Different input → different hash
    }

    #[test]
    fn test_estimate_shuffle_cost() {
        let config = ShuffleConfig::default();
        let exchange = ShuffleExchange::new(
            config,
            "node1".to_string(),
            vec![
                "node1".to_string(),
                "node2".to_string(),
                "node3".to_string(),
            ],
        );

        let cost = exchange.estimate_shuffle_cost(1000); // 1KB
        // 1KB / 3 nodes + 10% overhead ≈ 366 bytes
        assert!(cost > 300 && cost < 500);
    }

    #[test]
    fn test_create_join_key_single() {
        let columns = vec![serde_json::json!("test_value")];
        let key = ShuffleExchange::create_join_key(&columns).unwrap();

        assert!(matches!(key, ShuffleKey::String(_)));
    }

    #[test]
    fn test_create_join_key_multi() {
        let columns = vec![
            serde_json::json!("col1"),
            serde_json::json!(42),
            serde_json::json!(3.14),
        ];
        let key = ShuffleExchange::create_join_key(&columns).unwrap();

        assert!(matches!(key, ShuffleKey::Composite(_)));
    }
}
