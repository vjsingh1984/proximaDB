use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Node identifier in the distributed system
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(id: String) -> Self {
        NodeId(id)
    }
    
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Virtual node representation in the hash ring
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualNode {
    pub node_id: NodeId,
    pub virtual_id: u32,
    pub hash: u64,
}

impl VirtualNode {
    pub fn new(node_id: NodeId, virtual_id: u32) -> Self {
        let hash = Self::calculate_hash(&node_id, virtual_id);
        VirtualNode {
            node_id,
            virtual_id,
            hash,
        }
    }
    
    fn calculate_hash(node_id: &NodeId, virtual_id: u32) -> u64 {
        let mut hasher = DefaultHasher::new();
        node_id.hash(&mut hasher);
        virtual_id.hash(&mut hasher);
        hasher.finish()
    }
}

/// Consistent hash ring implementation with virtual nodes
/// 
/// This implementation provides:
/// - Even data distribution across nodes
/// - Automatic rebalancing when nodes join/leave
/// - Predictable data placement
/// - Configurable virtual nodes per physical node
#[derive(Debug, Clone)]
pub struct ConsistentHashRing {
    /// Hash ring storing virtual nodes sorted by hash value
    ring: BTreeMap<u64, VirtualNode>,
    /// Map from physical node to its virtual nodes
    nodes: HashMap<NodeId, Vec<VirtualNode>>,
    /// Number of virtual nodes per physical node
    virtual_nodes_per_node: u32,
    /// Replication factor for data redundancy
    replication_factor: u32,
}

impl ConsistentHashRing {
    /// Create a new consistent hash ring
    pub fn new(virtual_nodes_per_node: u32, replication_factor: u32) -> Self {
        ConsistentHashRing {
            ring: BTreeMap::new(),
            nodes: HashMap::new(),
            virtual_nodes_per_node,
            replication_factor,
        }
    }
    
    /// Add a physical node to the ring
    pub fn add_node(&mut self, node_id: NodeId) -> Result<()> {
        if self.nodes.contains_key(&node_id) {
            return Err(anyhow::anyhow!("Node {} already exists in ring", node_id.as_str()));
        }
        
        let mut virtual_nodes = Vec::new();
        for i in 0..self.virtual_nodes_per_node {
            let virtual_node = VirtualNode::new(node_id.clone(), i);
            self.ring.insert(virtual_node.hash, virtual_node.clone());
            virtual_nodes.push(virtual_node);
        }
        
        self.nodes.insert(node_id, virtual_nodes);
        Ok(())
    }
    
    /// Remove a physical node from the ring
    pub fn remove_node(&mut self, node_id: &NodeId) -> Result<()> {
        if let Some(virtual_nodes) = self.nodes.remove(node_id) {
            for virtual_node in virtual_nodes {
                self.ring.remove(&virtual_node.hash);
            }
            Ok(())
        } else {
            Err(anyhow::anyhow!("Node {} not found in ring", node_id.as_str()))
        }
    }
    
    /// Get the primary node responsible for a given key
    pub fn get_node(&self, key: &str) -> Option<NodeId> {
        self.get_nodes(key, 1).into_iter().next()
    }
    
    /// Get the nodes responsible for a given key (including replicas)
    pub fn get_nodes(&self, key: &str, count: u32) -> Vec<NodeId> {
        if self.ring.is_empty() {
            return Vec::new();
        }
        
        let key_hash = self.calculate_key_hash(key);
        let mut result = Vec::new();
        let mut seen_physical_nodes = std::collections::HashSet::new();
        
        // Find the first virtual node with hash >= key_hash
        let mut iter = self.ring.range(key_hash..).chain(self.ring.range(..));
        
        for (_, virtual_node) in iter {
            if !seen_physical_nodes.contains(&virtual_node.node_id) {
                result.push(virtual_node.node_id.clone());
                seen_physical_nodes.insert(virtual_node.node_id.clone());
                
                if result.len() >= count as usize {
                    break;
                }
            }
        }
        
        result
    }
    
    /// Get all nodes responsible for a collection (with replication)
    pub fn get_collection_nodes(&self, collection_id: &str) -> Vec<NodeId> {
        self.get_nodes(collection_id, self.replication_factor)
    }
    
    /// Calculate hash for a key
    fn calculate_key_hash(&self, key: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    }
    
    /// Get all physical nodes in the ring
    pub fn get_all_nodes(&self) -> Vec<NodeId> {
        self.nodes.keys().cloned().collect()
    }
    
    /// Get the number of physical nodes in the ring
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    
    /// Check if a node exists in the ring
    pub fn contains_node(&self, node_id: &NodeId) -> bool {
        self.nodes.contains_key(node_id)
    }
    
    /// Get ring statistics for monitoring
    pub fn get_statistics(&self) -> RingStatistics {
        RingStatistics {
            physical_nodes: self.nodes.len(),
            virtual_nodes: self.ring.len(),
            virtual_nodes_per_node: self.virtual_nodes_per_node,
            replication_factor: self.replication_factor,
        }
    }
    
    /// Rebalance data when nodes are added or removed
    pub fn get_rebalance_plan(&self, old_ring: &ConsistentHashRing, collection_ids: &[String]) -> RebalancePlan {
        let mut migrations = Vec::new();
        
        for collection_id in collection_ids {
            let old_nodes = old_ring.get_collection_nodes(collection_id);
            let new_nodes = self.get_collection_nodes(collection_id);
            
            // Find nodes that should no longer have this collection
            let nodes_to_remove: Vec<_> = old_nodes
                .iter()
                .filter(|node| !new_nodes.contains(node))
                .cloned()
                .collect();
            
            // Find nodes that should now have this collection
            let nodes_to_add: Vec<_> = new_nodes
                .iter()
                .filter(|node| !old_nodes.contains(node))
                .cloned()
                .collect();
            
            if !nodes_to_remove.is_empty() || !nodes_to_add.is_empty() {
                migrations.push(CollectionMigration {
                    collection_id: collection_id.clone(),
                    from_nodes: nodes_to_remove,
                    to_nodes: nodes_to_add,
                });
            }
        }
        
        RebalancePlan { migrations }
    }
}

/// Statistics about the hash ring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RingStatistics {
    pub physical_nodes: usize,
    pub virtual_nodes: usize,
    pub virtual_nodes_per_node: u32,
    pub replication_factor: u32,
}

/// Plan for rebalancing data across nodes
#[derive(Debug, Clone)]
pub struct RebalancePlan {
    pub migrations: Vec<CollectionMigration>,
}

/// Migration plan for a single collection
#[derive(Debug, Clone)]
pub struct CollectionMigration {
    pub collection_id: String,
    pub from_nodes: Vec<NodeId>,
    pub to_nodes: Vec<NodeId>,
}

impl RebalancePlan {
    pub fn is_empty(&self) -> bool {
        self.migrations.is_empty()
    }
    
    pub fn migration_count(&self) -> usize {
        self.migrations.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_consistent_hash_ring_basic() {
        let mut ring = ConsistentHashRing::new(100, 3);
        
        // Add nodes
        ring.add_node(NodeId::new("node1".to_string())).unwrap();
        ring.add_node(NodeId::new("node2".to_string())).unwrap();
        ring.add_node(NodeId::new("node3".to_string())).unwrap();
        
        assert_eq!(ring.node_count(), 3);
        assert_eq!(ring.ring.len(), 300); // 3 nodes * 100 virtual nodes
        
        // Test key distribution
        let collection_nodes = ring.get_collection_nodes("test_collection");
        assert_eq!(collection_nodes.len(), 3); // replication factor
        
        // Test node removal
        ring.remove_node(&NodeId::new("node2".to_string())).unwrap();
        assert_eq!(ring.node_count(), 2);
        assert_eq!(ring.ring.len(), 200); // 2 nodes * 100 virtual nodes
    }
    
    #[test]
    fn test_consistent_distribution() {
        let mut ring = ConsistentHashRing::new(100, 1);
        
        // Add nodes
        ring.add_node(NodeId::new("node1".to_string())).unwrap();
        ring.add_node(NodeId::new("node2".to_string())).unwrap();
        ring.add_node(NodeId::new("node3".to_string())).unwrap();
        
        // Test distribution of many keys
        let mut node_counts = HashMap::new();
        for i in 0..1000 {
            let key = format!("key_{}", i);
            let node = ring.get_node(&key).unwrap();
            *node_counts.entry(node).or_insert(0) += 1;
        }
        
        // Check that distribution is reasonably balanced
        let counts: Vec<_> = node_counts.values().collect();
        let min_count = **counts.iter().min().unwrap();
        let max_count = **counts.iter().max().unwrap();
        
        // Distribution should be within 20% of perfect balance
        assert!(max_count - min_count < 200);
    }
    
    #[test]
    fn test_rebalance_plan() {
        let mut old_ring = ConsistentHashRing::new(100, 2);
        old_ring.add_node(NodeId::new("node1".to_string())).unwrap();
        old_ring.add_node(NodeId::new("node2".to_string())).unwrap();
        
        let mut new_ring = ConsistentHashRing::new(100, 2);
        new_ring.add_node(NodeId::new("node1".to_string())).unwrap();
        new_ring.add_node(NodeId::new("node2".to_string())).unwrap();
        new_ring.add_node(NodeId::new("node3".to_string())).unwrap();
        
        let collections = vec!["collection1".to_string(), "collection2".to_string()];
        let plan = new_ring.get_rebalance_plan(&old_ring, &collections);
        
        // Should have migrations since we added a new node
        assert!(!plan.is_empty());
    }
}