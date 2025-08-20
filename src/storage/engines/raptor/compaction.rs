use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::{HashMap, HashSet};
use super::{RaptorConfig, hnsw_compaction::HnswAwareCompactionManager};
use super::hnsw_manager::HnswManager;
use crate::proto::proximadb::VectorRecord;

/// Unified compaction manager for RAPTOR that integrates with the framework
/// but uses aggressive single-file strategy for HNSW graph maintenance
pub struct CompactionManager {
    base_path: String,
    config: RaptorConfig,
    hnsw_compaction: Option<Arc<HnswAwareCompactionManager>>,
}

impl CompactionManager {
    pub fn new(base_path: String, config: RaptorConfig) -> Self {
        Self { 
            base_path: base_path.clone(), 
            config,
            hnsw_compaction: None,
        }
    }
    
    /// Initialize with HNSW manager for graph-aware compaction
    pub async fn with_hnsw(
        mut self,
        hnsw_manager: Arc<HnswManager>,
        filesystem: Arc<crate::storage::persistence::filesystem::zero_copy_filesystem::ZeroCopyFilesystem>,
        transaction_coordinator: Arc<crate::storage::transaction_coordinator::TransactionCoordinator>,
    ) -> Self {
        self.hnsw_compaction = Some(Arc::new(
            HnswAwareCompactionManager::new(
                self.base_path.clone(),
                self.config.clone(),
                hnsw_manager,
                filesystem,
                transaction_coordinator,
            ).await
        ));
        self
    }
    
    /// Check if compaction is needed based on RAPTOR's aggressive single-file policy
    pub async fn needs_compaction(&self) -> Result<bool> {
        // RAPTOR maintains exactly ONE L0 file for optimal HNSW locality
        // Trigger compaction immediately when a second file appears
        let files = self.list_raptor_files().await?;
        
        // If we have 2 or more files, compact immediately
        Ok(files.len() >= 2)
    }
    
    /// Perform compaction using unified framework with RAPTOR-specific settings
    pub async fn compact(&self) -> Result<()> {
        if let Some(ref hnsw_compaction) = self.hnsw_compaction {
            // HNSW-aware compaction that rebuilds graph
            let files = self.list_files().await?;
            if files.len() > 1 {
                let output_file = format!("{}/compacted_{}.rapt", 
                    self.base_path, 
                    chrono::Utc::now().timestamp_millis()
                );
                
                tracing::info!(
                    "RAPTOR: Triggering HNSW-aware compaction for {} files -> single file",
                    files.len()
                );
                
                hnsw_compaction.compact_with_graph_rebuild(
                    files,
                    &output_file
                ).await?;
                
                // Clean up old files after successful compaction
                self.cleanup_old_files().await?;
            }
        } else {
            // Standard compaction without HNSW
            tracing::info!("RAPTOR: Standard compaction (non-HNSW mode)");
            self.standard_compact().await?;
        }
        
        Ok(())
    }
    
    /// List all RAPTOR files in the base path
    async fn list_raptor_files(&self) -> Result<Vec<String>> {
        self.list_files().await
    }
    
    /// List all RAPTOR files in the base path
    async fn list_files(&self) -> Result<Vec<String>> {
        use tokio::fs;
        
        let mut files = Vec::new();
        let mut entries = fs::read_dir(&self.base_path).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("rapt") {
                files.push(path.to_string_lossy().to_string());
            }
        }
        
        files.sort(); // Sort by name (which includes timestamp)
        Ok(files)
    }
    
    /// Standard compaction for non-HNSW mode
    async fn standard_compact(&self) -> Result<()> {
        // Merge multiple rowgroups into larger ones
        // This is simplified - would actually read and merge files
        tracing::debug!("RAPTOR: Performing standard compaction");
        Ok(())
    }
    
    /// Clean up old files after successful compaction
    async fn cleanup_old_files(&self) -> Result<()> {
        use tokio::fs;
        
        let files = self.list_files().await?;
        
        // Keep only the most recent compacted file
        if files.len() > 1 {
            // The last file should be the newly compacted one
            let keep_file = files.last().unwrap();
            
            for file in &files[..files.len() - 1] {
                tracing::debug!("RAPTOR: Removing old file: {}", file);
                fs::remove_file(file).await?;
            }
            
            tracing::info!("RAPTOR: Cleaned up {} old files, kept: {}", 
                files.len() - 1, keep_file);
        }
        
        Ok(())
    }
    
    /// Get compaction configuration for unified framework integration
    pub fn get_compaction_config(&self) -> CompactionConfig {
        // Convert from config::CompactionConfig to local CompactionConfig
        self.config.compaction_config.as_ref().map(|cc| CompactionConfig {
            max_level: cc.max_level,
            l0_trigger_file_count: cc.l0_trigger_file_count,
            target_file_size: cc.target_file_size,
        }).unwrap_or(CompactionConfig {
            max_level: 0,
            l0_trigger_file_count: 2,
            target_file_size: usize::MAX,
        })
    }
    
    /// Reorganize vectors by HNSW locality during compaction
    /// This ensures vectors that are neighbors in HNSW are co-located in row groups
    pub async fn reorganize_by_hnsw_locality(
        &self,
        vectors: Vec<VectorRecord>,
        hnsw_graph: &HnswGraph,
    ) -> Result<Vec<RowGroup>> {
        let mut row_groups = Vec::new();
        let mut visited = HashSet::new();
        let mut current_group = Vec::new();
        
        // Build adjacency map from HNSW graph
        let adjacency = self.build_adjacency_map(hnsw_graph);
        
        // Start from entry points and traverse by locality
        for entry_point in &hnsw_graph.entry_points {
            if visited.contains(entry_point) {
                continue;
            }
            
            // BFS traversal to gather connected components
            let mut queue = vec![*entry_point];
            
            while let Some(node_id) = queue.pop() {
                if visited.contains(&node_id) {
                    continue;
                }
                
                visited.insert(node_id);
                
                // Add vector to current group
                if let Some(vector) = vectors.iter().find(|v| {
                    v.id.as_ref().map(|id| id == &format!("node_{}", node_id)).unwrap_or(false)
                }) {
                    current_group.push(vector.clone());
                    
                    // If group is full (1K vectors), start new group
                    if current_group.len() >= 1000 {
                        row_groups.push(RowGroup {
                            vectors: current_group.clone(),
                            local_hnsw: self.build_local_hnsw(&current_group),
                        });
                        current_group.clear();
                    }
                }
                
                // Add neighbors to queue
                if let Some(neighbors) = adjacency.get(&node_id) {
                    for &neighbor in neighbors {
                        if !visited.contains(&neighbor) {
                            queue.push(neighbor);
                        }
                    }
                }
            }
        }
        
        // Handle remaining vectors
        if !current_group.is_empty() {
            row_groups.push(RowGroup {
                vectors: current_group,
                local_hnsw: LocalHnswSegment::new(),
            });
        }
        
        // Add any unvisited vectors (shouldn't happen with proper HNSW)
        for vector in vectors {
            let node_id = self.extract_node_id(&vector);
            if !visited.contains(&node_id) {
                // Find the best row group or create new one
                if let Some(last_group) = row_groups.last_mut() {
                    if last_group.vectors.len() < 1000 {
                        last_group.vectors.push(vector);
                    } else {
                        row_groups.push(RowGroup {
                            vectors: vec![vector],
                            local_hnsw: LocalHnswSegment::new(),
                        });
                    }
                }
            }
        }
        
        tracing::info!(
            "RAPTOR: Reorganized {} vectors into {} row groups by HNSW locality",
            vectors.len(),
            row_groups.len()
        );
        
        Ok(row_groups)
    }
    
    /// Build adjacency map from HNSW graph
    fn build_adjacency_map(&self, graph: &HnswGraph) -> HashMap<u32, Vec<u32>> {
        let mut adjacency = HashMap::new();
        
        for edge in &graph.edges {
            adjacency.entry(edge.from)
                .or_insert_with(Vec::new)
                .push(edge.to);
            adjacency.entry(edge.to)
                .or_insert_with(Vec::new)
                .push(edge.from);
        }
        
        adjacency
    }
    
    /// Build local HNSW segment for a row group
    fn build_local_hnsw(&self, vectors: &[VectorRecord]) -> LocalHnswSegment {
        // Simplified - would actually build proper HNSW
        LocalHnswSegment {
            num_nodes: vectors.len(),
            entry_point: 0,
        }
    }
    
    /// Extract node ID from vector record
    fn extract_node_id(&self, vector: &VectorRecord) -> u32 {
        vector.id.as_ref()
            .and_then(|id| id.strip_prefix("node_"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
}

/// Represents a row group with localized vectors
pub struct RowGroup {
    pub vectors: Vec<VectorRecord>,
    pub local_hnsw: LocalHnswSegment,
}

/// Local HNSW segment for a row group
pub struct LocalHnswSegment {
    pub num_nodes: usize,
    pub entry_point: u32,
}

impl LocalHnswSegment {
    pub fn new() -> Self {
        Self {
            num_nodes: 0,
            entry_point: 0,
        }
    }
}

/// HNSW graph structure
pub struct HnswGraph {
    pub entry_points: Vec<u32>,
    pub edges: Vec<HnswEdge>,
}

/// Edge in HNSW graph
pub struct HnswEdge {
    pub from: u32,
    pub to: u32,
    pub distance: f32,
}

/// Compaction configuration for unified framework
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    pub max_level: usize,
    pub l0_trigger_file_count: usize,
    pub target_file_size: usize,
}