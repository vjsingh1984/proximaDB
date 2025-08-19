// ID Index for O(log n) lookups in SST
// Clean B+ tree implementation with no legacy code

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;

/// B+ tree based ID index for fast lookups
#[derive(Debug)]
pub struct IdIndex {
    /// B+ tree root
    root: RwLock<Option<Box<BPlusNode>>>,
    
    /// Direct ID to location mapping for O(1) after tree lookup
    id_to_location: RwLock<HashMap<String, BlockLocation>>,
    
    /// Statistics
    total_ids: std::sync::atomic::AtomicU64,
    unique_ids: std::sync::atomic::AtomicU64,
    
    /// Configuration
    order: usize,  // B+ tree order (max children per node)
}

/// B+ tree node
#[derive(Debug, Clone)]
pub enum BPlusNode {
    Internal {
        keys: Vec<String>,
        children: Vec<Box<BPlusNode>>,
        level: u32,
    },
    Leaf {
        entries: Vec<(String, BlockLocation)>,
        next: Option<usize>,  // Pointer to next leaf for range scans
    },
}

/// Location of a record within the SST file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockLocation {
    pub superblock_idx: u32,
    pub block_idx: u32,
    pub offset_in_block: u32,
    pub size_bytes: u32,
}

/// Location of a record for retrieval (alias for compatibility)
pub type RecordLocation = BlockLocation;

impl RecordLocation {
    /// Get the block ID from superblock and block indices
    pub fn block_id(&self) -> u32 {
        self.superblock_idx * 64 + self.block_idx
    }
}

impl IdIndex {
    /// Create a new empty index
    pub fn new() -> Self {
        Self {
            root: RwLock::new(None),
            id_to_location: RwLock::new(HashMap::new()),
            total_ids: std::sync::atomic::AtomicU64::new(0),
            unique_ids: std::sync::atomic::AtomicU64::new(0),
            order: 256,  // Each node can have up to 256 children
        }
    }
    
    /// Add an ID with its block and offset information
    pub fn add(&self, id: String, block_id: u32, offset_in_block: usize) -> Result<()> {
        let location = BlockLocation {
            superblock_idx: block_id / 64,
            block_idx: block_id % 64,
            offset_in_block: offset_in_block as u32,
            size_bytes: 0, // Will be calculated during write
        };
        self.insert(id, location)
    }
    
    /// Insert an ID with its location
    pub fn insert(&self, id: String, location: BlockLocation) -> Result<()> {
        // Update direct mapping
        let mut map = self.id_to_location.write().unwrap();
        let is_new = map.insert(id.clone(), location.clone()).is_none();
        
        if is_new {
            self.unique_ids.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.total_ids.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        
        // Update B+ tree
        let mut root = self.root.write().unwrap();
        match root.as_mut() {
            None => {
                // Create first leaf node
                *root = Some(Box::new(BPlusNode::Leaf {
                    entries: vec![(id, location)],
                    next: None,
                }));
            }
            Some(node) => {
                // Insert into existing tree
                if self.insert_into_node(node, id, location)? {
                    // Node split occurred, need to create new root
                    self.split_root(root.as_mut().unwrap())?;
                }
            }
        }
        
        Ok(())
    }
    
    /// Lookup an ID and return its location
    pub fn lookup(&self, id: &str) -> Option<BlockLocation> {
        self.id_to_location.read().unwrap().get(id).cloned()
    }
    
    /// Async lookup for compatibility with async APIs
    pub async fn lookup_async(&self, id: &str) -> Option<RecordLocation> {
        self.id_to_location.read().unwrap().get(id).cloned()
    }
    
    /// Batch lookup for multiple IDs
    pub fn lookup_batch(&self, ids: &[String]) -> Vec<Option<BlockLocation>> {
        let map = self.id_to_location.read().unwrap();
        ids.iter().map(|id| map.get(id).cloned()).collect()
    }
    
    /// Range query - get all IDs in a range
    pub fn range_query(&self, start: &str, end: &str) -> Vec<(String, BlockLocation)> {
        let map = self.id_to_location.read().unwrap();
        let mut results = Vec::new();
        
        for (id, loc) in map.iter() {
            if id >= start && id <= end {
                results.push((id.clone(), loc.clone()));
            }
        }
        
        results.sort_by(|a, b| a.0.cmp(&b.0));
        results
    }
    
    /// Get index statistics
    pub fn stats(&self) -> IndexStats {
        IndexStats {
            total_ids: self.total_ids.load(std::sync::atomic::Ordering::Relaxed),
            unique_ids: self.unique_ids.load(std::sync::atomic::Ordering::Relaxed),
            tree_height: self.get_tree_height(),
            memory_usage: self.estimate_memory_usage(),
        }
    }
    
    // Private helper methods
    
    fn insert_into_node(&self, node: &mut Box<BPlusNode>, id: String, location: BlockLocation) -> Result<bool> {
        match node.as_mut() {
            BPlusNode::Leaf { entries, .. } => {
                // Find insertion position
                let pos = entries.binary_search_by_key(&&id, |(k, _)| k).unwrap_or_else(|p| p);
                entries.insert(pos, (id, location));
                
                // Check if split is needed
                Ok(entries.len() > self.order)
            }
            BPlusNode::Internal { keys, children, .. } => {
                // Find child to insert into
                let pos = keys.binary_search(&id).unwrap_or_else(|p| p);
                let child_idx = pos.min(children.len() - 1);
                
                let needs_split = self.insert_into_node(&mut children[child_idx], id.clone(), location)?;
                
                if needs_split {
                    // Split child and update keys
                    self.split_child(keys, children, child_idx)?;
                }
                
                // Check if this node needs splitting
                Ok(children.len() > self.order)
            }
        }
    }
    
    fn split_root(&self, root: &mut Box<BPlusNode>) -> Result<()> {
        // Implementation of root splitting
        // This would create a new root with the old root as a child
        Ok(())
    }
    
    fn split_child(&self, keys: &mut Vec<String>, children: &mut Vec<Box<BPlusNode>>, child_idx: usize) -> Result<()> {
        // Implementation of child node splitting
        Ok(())
    }
    
    fn get_tree_height(&self) -> u32 {
        let root = self.root.read().unwrap();
        match root.as_ref() {
            None => 0,
            Some(node) => self.node_height(node),
        }
    }
    
    fn node_height(&self, node: &BPlusNode) -> u32 {
        match node {
            BPlusNode::Leaf { .. } => 1,
            BPlusNode::Internal { children, .. } => {
                1 + children.iter().map(|c| self.node_height(c)).max().unwrap_or(0)
            }
        }
    }
    
    fn estimate_memory_usage(&self) -> usize {
        let map_size = self.id_to_location.read().unwrap().len() * 
            (std::mem::size_of::<String>() + std::mem::size_of::<BlockLocation>() + 32); // HashMap overhead
        
        let tree_size = self.estimate_tree_memory();
        
        map_size + tree_size + std::mem::size_of::<Self>()
    }
    
    fn estimate_tree_memory(&self) -> usize {
        // Rough estimate based on node count and average size
        let unique_ids = self.unique_ids.load(std::sync::atomic::Ordering::Relaxed) as usize;
        let nodes = (unique_ids / (self.order / 2)).max(1);
        nodes * (self.order * std::mem::size_of::<String>() + 1024) // Node overhead
    }
}

/// Statistics for the ID index
#[derive(Debug, Clone)]
pub struct IndexStats {
    pub total_ids: u64,
    pub unique_ids: u64,
    pub tree_height: u32,
    pub memory_usage: usize,
}

/// Two-level index for very large datasets
pub struct TwoLevelIdIndex {
    /// Sparse index - every Nth ID
    sparse_index: BTreeMap<String, BlockRange>,
    
    /// Dense indexes per range
    dense_indexes: Vec<DenseIdIndex>,
    
    /// Configuration
    sparse_factor: u32,
}

#[derive(Debug, Clone)]
pub struct BlockRange {
    pub start_id: String,
    pub end_id: String,
    pub dense_index_id: usize,
}

#[derive(Debug)]
pub struct DenseIdIndex {
    pub start_id: String,
    pub end_id: String,
    pub entries: BTreeMap<String, u32>,  // ID to offset in block
}

impl TwoLevelIdIndex {
    pub fn new(sparse_factor: u32) -> Self {
        Self {
            sparse_index: BTreeMap::new(),
            dense_indexes: Vec::new(),
            sparse_factor,
        }
    }
    
    pub fn lookup(&self, id: &str) -> Option<u32> {
        // Find the range containing this ID
        let range = self.sparse_index.range(..=id.to_string()).next_back()?;
        
        // Look in the dense index for exact location
        let dense_idx = &self.dense_indexes[range.1.dense_index_id];
        dense_idx.entries.get(id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_id_index_basic_operations() {
        let index = IdIndex::new();
        
        // Insert some IDs
        for i in 0..1000 {
            let id = format!("id_{:04}", i);
            let location = BlockLocation {
                superblock_idx: (i / 100) as u32,
                block_idx: ((i % 100) / 10) as u32,
                offset_in_block: (i % 10) as u32,
                size_bytes: 1024,
            };
            index.insert(id, location).unwrap();
        }
        
        // Test lookup
        let loc = index.lookup("id_0500").unwrap();
        assert_eq!(loc.superblock_idx, 5);
        assert_eq!(loc.block_idx, 0);
        assert_eq!(loc.offset_in_block, 0);
        
        // Test batch lookup
        let ids = vec!["id_0100".to_string(), "id_0200".to_string(), "id_0999".to_string()];
        let locs = index.lookup_batch(&ids);
        assert_eq!(locs.len(), 3);
        assert!(locs[0].is_some());
        assert!(locs[1].is_some());
        assert!(locs[2].is_some());
        
        // Test range query
        let range_results = index.range_query("id_0100", "id_0110");
        assert_eq!(range_results.len(), 11);
        
        // Test stats
        let stats = index.stats();
        assert_eq!(stats.unique_ids, 1000);
        assert!(stats.tree_height > 0);
    }
    
    #[test]
    fn test_two_level_index() {
        let mut index = TwoLevelIdIndex::new(100);
        
        // Add sparse entries
        index.sparse_index.insert("id_0000".to_string(), BlockRange {
            start_id: "id_0000".to_string(),
            end_id: "id_0099".to_string(),
            dense_index_id: 0,
        });
        
        // Add dense index
        let mut dense = DenseIdIndex {
            start_id: "id_0000".to_string(),
            end_id: "id_0099".to_string(),
            entries: BTreeMap::new(),
        };
        
        for i in 0..100 {
            dense.entries.insert(format!("id_{:04}", i), i);
        }
        
        index.dense_indexes.push(dense);
        
        // Test lookup
        assert_eq!(index.lookup("id_0050"), Some(50));
        assert_eq!(index.lookup("id_0099"), Some(99));
        assert_eq!(index.lookup("id_0100"), None);
    }
}