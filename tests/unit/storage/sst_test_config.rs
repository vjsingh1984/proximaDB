//! Consistent configuration for SST tests

use proximadb::core::{SstConfig, BloomFilterConfig, WriteBufferUserConfig};
use proximadb::storage::persistence::filesystem::FilesystemConfig;
use std::path::Path;

// Inline persistent test assignments module
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tokio::fs;

/// Test assignment data stored on disk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestAssignmentData {
    pub collection_id: String,
    pub base_directory: String,
    pub data_url: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Persistent assignment manager for tests
pub struct PersistentTestAssignments {
    /// Path to the assignment file
    assignment_file: PathBuf,
    /// Semaphore for file access (allows concurrent reads, sequential writes)
    file_semaphore: Arc<Semaphore>,
    /// In-memory cache for fast access
    cache: Arc<RwLock<HashMap<String, TestAssignmentData>>>,
}

impl PersistentTestAssignments {
    /// Create a new persistent assignment manager
    pub fn new() -> Self {
        let assignment_file = std::env::temp_dir().join("proximadb_test_assignments.json");
        
        Self {
            assignment_file,
            file_semaphore: Arc::new(Semaphore::new(10)), // Allow 10 concurrent reads, 1 write
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or create assignment for a collection
    pub async fn get_or_create_assignment(&self, collection_id: &str) -> Result<TestAssignmentData> {
        // First check cache
        {
            let cache = self.cache.read().await;
            if let Some(assignment) = cache.get(collection_id) {
                return Ok(assignment.clone());
            }
        }

        // Acquire semaphore for file access
        let _permit = self.file_semaphore.acquire().await.unwrap();

        // Double-check cache after acquiring lock
        {
            let cache = self.cache.read().await;
            if let Some(assignment) = cache.get(collection_id) {
                return Ok(assignment.clone());
            }
        }

        // Load assignments from disk
        let mut assignments = self.load_assignments_from_disk().await?;

        // Check if assignment exists on disk
        if let Some(assignment) = assignments.get(collection_id) {
            // Update cache and return
            {
                let mut cache = self.cache.write().await;
                cache.insert(collection_id.to_string(), assignment.clone());
            }
            return Ok(assignment.clone());
        }

        // Create new assignment using fixed directory path instead of tempfile
        let base_directory = format!("/tmp/proximadb_test_{}", collection_id);
        let data_url = format!("file://{}/{}/data", base_directory, collection_id);

        let assignment = TestAssignmentData {
            collection_id: collection_id.to_string(),
            base_directory: base_directory.clone(),
            data_url: data_url.clone(),
            created_at: chrono::Utc::now(),
        };

        // Create the data directory
        let data_path = PathBuf::from(&base_directory).join(collection_id).join("data");
        fs::create_dir_all(&data_path).await?;

        // Store on disk
        assignments.insert(collection_id.to_string(), assignment.clone());
        self.save_assignments_to_disk(&assignments).await?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(collection_id.to_string(), assignment.clone());
        }

        println!("Created persistent test assignment for {}: {}", collection_id, data_url);

        Ok(assignment)
    }

    /// Update assignment for a collection
    pub async fn update_assignment(&self, collection_id: &str, assignment: TestAssignmentData) -> Result<()> {
        // Acquire semaphore for file access
        let _permit = self.file_semaphore.acquire().await.unwrap();

        // Load assignments from disk
        let mut assignments = self.load_assignments_from_disk().await?;

        // Update in disk storage
        assignments.insert(collection_id.to_string(), assignment.clone());
        self.save_assignments_to_disk(&assignments).await?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(collection_id.to_string(), assignment);
        }

        Ok(())
    }

    /// Remove assignment for a collection
    pub async fn remove_assignment(&self, collection_id: &str) -> Result<()> {
        // Acquire semaphore for file access
        let _permit = self.file_semaphore.acquire().await.unwrap();

        // Load assignments from disk
        let mut assignments = self.load_assignments_from_disk().await?;

        // Remove from disk storage
        assignments.remove(collection_id);
        self.save_assignments_to_disk(&assignments).await?;

        // Remove from cache
        {
            let mut cache = self.cache.write().await;
            cache.remove(collection_id);
        }

        Ok(())
    }

    /// Clear all assignments (for test cleanup)
    pub async fn clear_all_assignments(&self) -> Result<()> {
        // Acquire semaphore for file access
        let _permit = self.file_semaphore.acquire().await.unwrap();

        // Clear disk storage
        if self.assignment_file.exists() {
            fs::remove_file(&self.assignment_file).await?;
        }

        // Clear cache
        {
            let mut cache = self.cache.write().await;
            cache.clear();
        }

        Ok(())
    }

    /// Load assignments from disk file
    async fn load_assignments_from_disk(&self) -> Result<HashMap<String, TestAssignmentData>> {
        if !self.assignment_file.exists() {
            return Ok(HashMap::new());
        }

        let content = fs::read_to_string(&self.assignment_file).await?;
        if content.trim().is_empty() {
            // Handle empty file case
            return Ok(HashMap::new());
        }

        let assignments: HashMap<String, TestAssignmentData> = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse assignment file: {}", e))?;
        Ok(assignments)
    }

    /// Save assignments to disk file
    async fn save_assignments_to_disk(&self, assignments: &HashMap<String, TestAssignmentData>) -> Result<()> {
        let content = serde_json::to_string_pretty(assignments)?;
        fs::write(&self.assignment_file, content).await?;
        Ok(())
    }
}

/// Global instance for test assignments
static TEST_ASSIGNMENTS: std::sync::OnceLock<PersistentTestAssignments> = std::sync::OnceLock::new();

/// Get the global test assignments instance
pub fn get_test_assignments() -> &'static PersistentTestAssignments {
    TEST_ASSIGNMENTS.get_or_init(|| PersistentTestAssignments::new())
}

/// Create a consistent test configuration for SST
pub fn create_test_sst_config(base_path: &str) -> SstConfig {
    SstConfig {
        // Level configuration
        level_count: 4,  // Fewer levels for tests
        max_levels: 4,
        compaction_threshold: 2,  // Low threshold for testing
        max_files_per_level: 4,
        level_size_multiplier: 4.0,
        
        // Block and file settings
        block_size_kb: 16,  // Smaller blocks for tests
        
        // Storage type
        compaction_strategy: "leveled".to_string(),
        compression: "none".to_string(),  // No compression for tests
        compression_enabled: false,
        compression_level: 0,
        
        // Bloom filter - use consistent settings
        bloom_filter_config: Some(BloomFilterConfig {
            bits_per_key: 10,
            enabled: true,
            ..Default::default()
        }),
        decompression_cache_config: None,
        
        // Cache
        cache_size_mb: 32,
        
        // Background operations
        background_thread_count: 2,
        
        // Directories - use assignment service compatible paths
        // The assignment service will create {base_path}/{collection_id}/data
        // So we set the base path here and let assignment service handle collection paths
        data_directory: format!("{}/data", base_path),
        
        // Memory mapping
        mmap_enabled: false,
        prefetch_enabled: false,
        prefetch_size_kb: 0,
    }
}

/// Create a consistent test configuration for WriteBuffer
pub fn create_test_write_buffer_config(base_path: &str) -> WriteBufferUserConfig {
    WriteBufferUserConfig {
        write_buffer_size_mb: 4,  // Small for tests
        memory_flush_size_bytes: 1024 * 1024,  // 1MB flush threshold
        memtable_type: "BTree".to_string(),
        sync_mode: "perbatch".to_string(),
        write_buffer_directory: format!("{}/write_buffer", base_path),
        enable_wal: true,
        vector_count_threshold: 100,  // Small threshold for tests
    }
}

/// Create consistent filesystem configuration for tests
pub fn create_test_filesystem_config() -> FilesystemConfig {
    FilesystemConfig::default()
}

/// Setup test directories
pub async fn setup_test_directories(base_path: &Path) -> anyhow::Result<()> {
    use tokio::fs;
    
    // Create base directory
    fs::create_dir_all(base_path).await?;
    
    // Create subdirectories
    fs::create_dir_all(base_path.join("data")).await?;
    fs::create_dir_all(base_path.join("write_buffer")).await?;
    
    Ok(())
}

/// Setup storage assignment for a collection with persistent directory management
/// This ensures the same collection always gets the same directory across test runs
pub async fn setup_storage_assignment(collection_id: &str, _base_path: &str) -> anyhow::Result<TestAssignmentData> {
    use proximadb::core::config::StorageLocation;
    
    // Get persistent assignment (creates new one if doesn't exist)
    let test_assignments = get_test_assignments();
    let test_assignment = test_assignments.get_or_create_assignment(collection_id).await?;
    
    // 🔴 UNUSED - Assignment service removed
    // let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    
    // Create storage location using the persistent assignment's base directory
    let storage_location = StorageLocation {
        url: format!("file://{}", test_assignment.base_directory),
        weight: 1,
        tags: Default::default(),
    };
    
    // 🔴 OBSOLETE - Assignment service removed, using test assignment directly
    // No need to register with service - just use the test assignment paths
    
    // Assignment service removed - collections now embed storage_assignment
    // Use the test assignment directly
    /*
    let assignment = assignment_service.get_assignment(collection_id).await;
    if let Some(assignment) = assignment {
        // If assignment differs from our persistent one, update the persistent storage
        if assignment.data_url != test_assignment.data_url {
            println!("Assignment service returned different URL: {} vs persistent {}", 
                    assignment.data_url, test_assignment.data_url);
            
            // Update our persistent assignment to match what assignment service returned
            let updated_assignment = TestAssignmentData {
                collection_id: collection_id.to_string(),
                base_directory: if assignment.data_url.starts_with("file://") {
                    assignment.data_url.strip_prefix("file://").unwrap()
                        .trim_end_matches(&format!("/{}/data", collection_id))
                        .to_string()
                } else {
                    assignment.data_url.trim_end_matches(&format!("/{}/data", collection_id)).to_string()
                },
                data_url: assignment.data_url.clone(),
                created_at: chrono::Utc::now(),
            };
            
            // Update persistent storage
            test_assignments.update_assignment(collection_id, updated_assignment).await?;
        }
        
        let data_path = assignment.data_url.strip_prefix("file://").unwrap_or(&assignment.data_url);
        tokio::fs::create_dir_all(data_path).await?;
        println!("Created data directory: {}", data_path);
    } else {
        return Err(anyhow::anyhow!("Assignment was not created for collection {}", collection_id));
    }
    */
    
    // Just use the test assignment directly
    let data_path = test_assignment.data_url.strip_prefix("file://").unwrap_or(&test_assignment.data_url);
    tokio::fs::create_dir_all(data_path).await?;
    println!("Created data directory: {}", data_path);
    
    Ok(test_assignment)
}

/// Cleanup test directories  
pub async fn cleanup_test_directories(base_path: &Path) -> anyhow::Result<()> {
    use tokio::fs;
    
    if base_path.exists() {
        fs::remove_dir_all(base_path).await?;
    }
    
    Ok(())
}

/// Cleanup SSTable files for a collection
pub async fn cleanup_sstable_files(collection_id: &str) -> anyhow::Result<()> {
    let test_assignments = get_test_assignments();
    
    // Get the assignment to find the data directory
    if let Ok(assignment) = test_assignments.get_or_create_assignment(collection_id).await {
        let data_path = assignment.data_url.strip_prefix("file://").unwrap_or(&assignment.data_url);
        let data_dir = PathBuf::from(data_path);
        
        if data_dir.exists() {
            // Remove all .sst files in the directory
            let mut entries = fs::read_dir(&data_dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_file() && path.extension().map_or(false, |ext| ext == "sst") {
                    println!("Cleaning up SSTable file: {}", path.display());
                    let _ = fs::remove_file(&path).await; // Ignore errors for missing files
                }
            }
        }
    }
    
    Ok(())
}

/// Cleanup assignment for a collection (removes from persistent storage only)
pub async fn cleanup_assignment(collection_id: &str) -> anyhow::Result<()> {
    // 🔴 UNUSED - Assignment service removed
    // let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    
    // Remove from global assignment service - NO LONGER NEEDED
    // let _ = assignment_service.remove_assignment(collection_id).await;
    
    // Remove from persistent test assignments
    let test_assignments = get_test_assignments();
    let _ = test_assignments.remove_assignment(collection_id).await;
    
    Ok(())
}

/// Cleanup all test assignments (for complete test isolation)
pub async fn cleanup_all_assignments() -> anyhow::Result<()> {
    let test_assignments = get_test_assignments();
    test_assignments.clear_all_assignments().await?;
    
    Ok(())
}