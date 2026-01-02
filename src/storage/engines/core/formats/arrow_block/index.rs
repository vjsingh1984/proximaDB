//! Arrow Block Index
//!
//! B+ tree index for O(log n) block and record lookups within Arrow block files.

use serde::{Deserialize, Serialize};

/// B+ tree index for Arrow block files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrowBlockIndex {
    /// Block-level entries: maps (min_id) -> block entry
    pub block_entries: Vec<ArrowIndexEntry>,

    /// B+ tree leaf nodes for fast key lookup
    pub bplus_tree: Option<BPlusTree>,

    /// Bloom filter bytes (serialized)
    pub bloom_filter: Option<Vec<u8>>,

    /// Total records indexed
    pub total_records: u64,
}

/// Entry in the block index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrowIndexEntry {
    /// Block number
    pub block_num: u32,

    /// Minimum ID in block
    pub min_id: String,

    /// Maximum ID in block
    pub max_id: String,

    /// Offset in file (bytes)
    pub offset: u64,

    /// Size of block (bytes)
    pub size: u64,

    /// Number of records in block
    pub record_count: u32,

    /// Minimum timestamp in block
    pub min_timestamp: Option<i64>,

    /// Maximum timestamp in block
    pub max_timestamp: Option<i64>,
}

/// Simplified B+ tree for ID lookups
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BPlusTree {
    /// Internal nodes: key -> child index
    pub internal_nodes: Vec<BPlusInternalNode>,

    /// Leaf nodes: key ranges -> block entries
    pub leaf_nodes: Vec<BPlusLeafNode>,

    /// Tree depth
    pub depth: u8,

    /// Fanout (keys per node)
    pub fanout: u16,
}

/// B+ tree internal node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BPlusInternalNode {
    /// Keys that separate children
    pub keys: Vec<String>,

    /// Child node indices
    pub children: Vec<u32>,
}

/// B+ tree leaf node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BPlusLeafNode {
    /// Start index in block_entries
    pub start_idx: usize,

    /// Number of entries in this leaf
    pub len: usize,

    /// End key for range queries
    pub end_key: String,

    /// Next leaf node index (for range scans)
    pub next_leaf: Option<u32>,
}

impl ArrowBlockIndex {
    /// Create empty index
    pub fn new() -> Self {
        Self {
            block_entries: Vec::new(),
            bplus_tree: None,
            bloom_filter: None,
            total_records: 0,
        }
    }

    /// Add a block entry
    pub fn add_entry(&mut self, entry: ArrowIndexEntry) {
        self.total_records += entry.record_count as u64;
        self.block_entries.push(entry);
    }

    /// Build B+ tree index after all entries are added
    pub fn build_bplus_tree(&mut self, fanout: u16) {
        if self.block_entries.is_empty() {
            return;
        }

        // Sort entries by min_id
        self.block_entries.sort_by(|a, b| a.min_id.cmp(&b.min_id));

        // Build leaf nodes
        let mut leaf_nodes = Vec::new();
        let entries_per_leaf = fanout as usize;

        for (i, chunk) in self.block_entries.chunks(entries_per_leaf).enumerate() {
            let start_idx = i * entries_per_leaf;
            let end_key = chunk.last().map(|e| e.max_id.clone()).unwrap_or_default();
            let next_leaf = if (i + 1) * entries_per_leaf < self.block_entries.len() {
                Some((i + 1) as u32)
            } else {
                None
            };

            leaf_nodes.push(BPlusLeafNode {
                start_idx,
                len: chunk.len(),
                end_key,
                next_leaf,
            });
        }

        // Build internal nodes (single level for simplicity)
        let internal_nodes = if leaf_nodes.len() > 1 {
            let keys: Vec<String> = leaf_nodes
                .iter()
                .skip(1)
                .map(|leaf| {
                    self.block_entries
                        .get(leaf.start_idx)
                        .map(|e| e.min_id.clone())
                        .unwrap_or_default()
                })
                .collect();

            let children: Vec<u32> = (0..leaf_nodes.len() as u32).collect();

            vec![BPlusInternalNode { keys, children }]
        } else {
            Vec::new()
        };

        let depth = if internal_nodes.is_empty() { 1 } else { 2 };

        self.bplus_tree = Some(BPlusTree {
            internal_nodes,
            leaf_nodes,
            depth,
            fanout,
        });
    }

    /// Find block entry for a given ID using B+ tree
    pub fn find_block_for_id(&self, id: &str) -> Option<&ArrowIndexEntry> {
        if let Some(ref tree) = self.bplus_tree {
            // Find the right leaf node
            let leaf_idx = self.find_leaf_for_key(tree, id)?;
            let leaf = tree.leaf_nodes.get(leaf_idx)?;

            // Search within leaf's entries
            for i in leaf.start_idx..leaf.start_idx + leaf.len {
                if let Some(entry) = self.block_entries.get(i) {
                    if id >= entry.min_id.as_str() && id <= entry.max_id.as_str() {
                        return Some(entry);
                    }
                }
            }
        } else {
            // Linear scan fallback
            for entry in &self.block_entries {
                if id >= entry.min_id.as_str() && id <= entry.max_id.as_str() {
                    return Some(entry);
                }
            }
        }
        None
    }

    /// Find leaf node index for a key
    fn find_leaf_for_key(&self, tree: &BPlusTree, key: &str) -> Option<usize> {
        if tree.internal_nodes.is_empty() {
            // Single leaf tree
            return Some(0);
        }

        // Binary search in internal node
        let root = &tree.internal_nodes[0];
        let mut child_idx = root.children.len() - 1;

        for (i, separator) in root.keys.iter().enumerate() {
            if key < separator.as_str() {
                child_idx = i;
                break;
            }
        }

        Some(root.children.get(child_idx).copied()? as usize)
    }

    /// Find all blocks in a key range
    pub fn find_blocks_in_range(&self, start_id: &str, end_id: &str) -> Vec<&ArrowIndexEntry> {
        let mut results = Vec::new();

        for entry in &self.block_entries {
            // Check if block overlaps with range
            if entry.max_id.as_str() >= start_id && entry.min_id.as_str() <= end_id {
                results.push(entry);
            }
        }

        results
    }

    /// Find all blocks in a timestamp range
    pub fn find_blocks_in_time_range(&self, start_ts: i64, end_ts: i64) -> Vec<&ArrowIndexEntry> {
        self.block_entries
            .iter()
            .filter(|entry| {
                if let (Some(min_ts), Some(max_ts)) = (entry.min_timestamp, entry.max_timestamp) {
                    max_ts >= start_ts && min_ts <= end_ts
                } else {
                    true // Include blocks without timestamp info
                }
            })
            .collect()
    }

    /// Serialize index to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).unwrap_or_default()
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bincode::deserialize(bytes).ok()
    }

    /// Get index size in bytes
    pub fn size_bytes(&self) -> usize {
        self.to_bytes().len()
    }
}

impl Default for ArrowBlockIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl ArrowIndexEntry {
    /// Create new entry
    pub fn new(
        block_num: u32,
        min_id: String,
        max_id: String,
        offset: u64,
        size: u64,
        record_count: u32,
    ) -> Self {
        Self {
            block_num,
            min_id,
            max_id,
            offset,
            size,
            record_count,
            min_timestamp: None,
            max_timestamp: None,
        }
    }

    /// Set timestamp range
    pub fn with_timestamps(mut self, min: i64, max: i64) -> Self {
        self.min_timestamp = Some(min);
        self.max_timestamp = Some(max);
        self
    }

    /// Check if ID might be in this block
    pub fn might_contain(&self, id: &str) -> bool {
        id >= self.min_id.as_str() && id <= self.max_id.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_entries(count: usize) -> Vec<ArrowIndexEntry> {
        (0..count)
            .map(|i| {
                let start = i * 100;
                let end = start + 99;
                ArrowIndexEntry::new(
                    i as u32,
                    format!("id_{:05}", start),
                    format!("id_{:05}", end),
                    (i * 1000) as u64,
                    1000,
                    100,
                )
            })
            .collect()
    }

    #[test]
    fn test_index_creation() {
        let mut index = ArrowBlockIndex::new();
        for entry in create_test_entries(10) {
            index.add_entry(entry);
        }

        assert_eq!(index.block_entries.len(), 10);
        assert_eq!(index.total_records, 1000);
    }

    #[test]
    fn test_bplus_tree_build() {
        let mut index = ArrowBlockIndex::new();
        for entry in create_test_entries(20) {
            index.add_entry(entry);
        }

        index.build_bplus_tree(8);

        assert!(index.bplus_tree.is_some());
        let tree = index.bplus_tree.as_ref().unwrap();
        assert!(!tree.leaf_nodes.is_empty());
    }

    #[test]
    fn test_find_block_for_id() {
        let mut index = ArrowBlockIndex::new();
        for entry in create_test_entries(10) {
            index.add_entry(entry);
        }
        index.build_bplus_tree(4);

        // Find ID in first block
        let entry = index.find_block_for_id("id_00050");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().block_num, 0);

        // Find ID in fifth block
        let entry = index.find_block_for_id("id_00450");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().block_num, 4);

        // ID not in any block
        let entry = index.find_block_for_id("id_99999");
        assert!(entry.is_none());
    }

    #[test]
    fn test_find_blocks_in_range() {
        let mut index = ArrowBlockIndex::new();
        for entry in create_test_entries(10) {
            index.add_entry(entry);
        }

        let blocks = index.find_blocks_in_range("id_00200", "id_00500");
        assert_eq!(blocks.len(), 4); // Blocks 2, 3, 4, 5
    }

    #[test]
    fn test_serialization() {
        let mut index = ArrowBlockIndex::new();
        for entry in create_test_entries(5) {
            index.add_entry(entry);
        }
        index.build_bplus_tree(4);

        let bytes = index.to_bytes();
        let recovered = ArrowBlockIndex::from_bytes(&bytes).unwrap();

        assert_eq!(recovered.block_entries.len(), 5);
        assert_eq!(recovered.total_records, 500);
        assert!(recovered.bplus_tree.is_some());
    }
}
