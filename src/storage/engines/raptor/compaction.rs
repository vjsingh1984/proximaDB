use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use super::{RaptorConfig, hnsw_compaction::HnswAwareCompactionManager};
use super::hnsw_manager::HnswManager;

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
    
    /// Check if compaction is needed based on RAPTOR's aggressive policy
    pub async fn needs_compaction(&self) -> Result<bool> {
        if let Some(ref hnsw_compaction) = self.hnsw_compaction {
            // HNSW mode: compact when we have more than 1 file
            hnsw_compaction.needs_compaction().await
        } else {
            // Non-HNSW mode: use standard threshold
            let files = self.list_files().await?;
            Ok(files.len() >= self.config.compaction_threshold_files)
        }
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
        self.config.compaction_config.clone().unwrap_or(CompactionConfig {
            max_level: 0,
            l0_trigger_file_count: 2,
            target_file_size: usize::MAX,
        })
    }
}

/// Compaction configuration for unified framework
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    pub max_level: usize,
    pub l0_trigger_file_count: usize,
    pub target_file_size: usize,
}