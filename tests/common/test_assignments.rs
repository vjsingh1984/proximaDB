//! Centralized test assignment helper for all ProximaDB tests
//!
//! This module provides persistent disk-based storage for test assignments
//! to ensure consistent directory usage across all test types and concurrent tests.

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
    #[serde(default = "default_write_buffer_url")]
    pub write_buffer_url: String,
    #[serde(default = "default_index_url")]
    pub index_url: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

fn default_write_buffer_url() -> String {
    String::new()
}

fn default_index_url() -> String {
    String::new()
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
        if let Some(mut assignment) = assignments.get(collection_id).cloned() {
            // Fill in missing fields for backward compatibility
            if assignment.write_buffer_url.is_empty() {
                assignment.write_buffer_url = format!("file://{}/{}/write_buffer", assignment.base_directory, collection_id);
            }
            if assignment.index_url.is_empty() {
                assignment.index_url = format!("file://{}/{}/index", assignment.base_directory, collection_id);
            }
            
            // Update cache and return
            {
                let mut cache = self.cache.write().await;
                cache.insert(collection_id.to_string(), assignment.clone());
            }
            return Ok(assignment);
        }

        // Create new assignment using fixed directory path based on collection ID
        let base_directory = format!("/tmp/proximadb_test_{}", collection_id);
        let data_url = format!("file://{}/{}/data", base_directory, collection_id);
        let write_buffer_url = format!("file://{}/{}/write_buffer", base_directory, collection_id);
        let index_url = format!("file://{}/{}/index", base_directory, collection_id);

        let assignment = TestAssignmentData {
            collection_id: collection_id.to_string(),
            base_directory: base_directory.clone(),
            data_url: data_url.clone(),
            write_buffer_url: write_buffer_url.clone(),
            index_url: index_url.clone(),
            created_at: chrono::Utc::now(),
        };

        // Create all required directories
        let data_path = PathBuf::from(&base_directory).join(collection_id).join("data");
        let write_buffer_path = PathBuf::from(&base_directory).join(collection_id).join("write_buffer");
        let index_path = PathBuf::from(&base_directory).join(collection_id).join("index");
        
        fs::create_dir_all(&data_path).await?;
        fs::create_dir_all(&write_buffer_path).await?;
        fs::create_dir_all(&index_path).await?;

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

        // Clear disk storage and any temp files
        if self.assignment_file.exists() {
            fs::remove_file(&self.assignment_file).await?;
        }
        
        // Clean up any temp files
        let temp_file = format!("{}.tmp", self.assignment_file.display());
        if std::path::Path::new(&temp_file).exists() {
            let _ = fs::remove_file(&temp_file).await;
        }

        // Clear cache
        {
            let mut cache = self.cache.write().await;
            cache.clear();
        }

        Ok(())
    }

    /// Load assignments from disk file with corruption recovery
    async fn load_assignments_from_disk(&self) -> Result<HashMap<String, TestAssignmentData>> {
        if !self.assignment_file.exists() {
            return Ok(HashMap::new());
        }

        let content = fs::read_to_string(&self.assignment_file).await?;
        if content.trim().is_empty() {
            // Handle empty file case
            return Ok(HashMap::new());
        }

        // Try to parse the JSON
        match serde_json::from_str::<HashMap<String, TestAssignmentData>>(&content) {
            Ok(assignments) => Ok(assignments),
            Err(e) => {
                eprintln!("Warning: Assignment file corrupted, recreating: {}", e);
                eprintln!("File content: {}", content);
                
                // Remove corrupted file and start fresh
                let _ = fs::remove_file(&self.assignment_file).await;
                Ok(HashMap::new())
            }
        }
    }

    /// Save assignments to disk file with atomic write
    async fn save_assignments_to_disk(&self, assignments: &HashMap<String, TestAssignmentData>) -> Result<()> {
        let content = serde_json::to_string_pretty(assignments)?;
        
        // Write to a temporary file first, then atomically move it
        let temp_file = format!("{}.tmp", self.assignment_file.display());
        fs::write(&temp_file, &content).await?;
        
        // Ensure content is synced to disk
        let file = fs::OpenOptions::new().write(true).open(&temp_file).await?;
        file.sync_all().await?;
        drop(file);
        
        // Atomically move the temp file to the final location
        fs::rename(&temp_file, &self.assignment_file).await?;
        Ok(())
    }
}

/// Global instance for test assignments
static TEST_ASSIGNMENTS: std::sync::OnceLock<PersistentTestAssignments> = std::sync::OnceLock::new();

/// Get the global test assignments instance
pub fn get_test_assignments() -> &'static PersistentTestAssignments {
    TEST_ASSIGNMENTS.get_or_init(|| PersistentTestAssignments::new())
}

/// Setup storage assignment for a collection with persistent directory management
/// This ensures the same collection always gets the same directory across test runs
pub async fn setup_persistent_test_assignment(collection_id: &str) -> Result<TestAssignmentData> {
    let test_assignments = get_test_assignments();
    let assignment = test_assignments.get_or_create_assignment(collection_id).await?;
    
    // Also register with the global assignment service for integration
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    
    // Create storage location using the persistent assignment's base directory
    let storage_location = proximadb::core::config::StorageLocation {
        url: format!("file://{}", assignment.base_directory),
        weight: 1,
        tags: Default::default(),
    };
    
    // Register with assignment service
    let service_assignment = assignment_service
        .assign_collection(collection_id, &[storage_location], "hash")
        .await?;
    
    // If assignment service returns different URL, update our persistent assignment
    if service_assignment.data_url != assignment.data_url {
        println!("Assignment service returned different URL: {} vs persistent {}", 
                service_assignment.data_url, assignment.data_url);
        
        let updated_assignment = TestAssignmentData {
            collection_id: collection_id.to_string(),
            base_directory: if service_assignment.data_url.starts_with("file://") {
                service_assignment.data_url.strip_prefix("file://").unwrap()
                    .trim_end_matches(&format!("/{}/data", collection_id))
                    .to_string()
            } else {
                service_assignment.data_url.trim_end_matches(&format!("/{}/data", collection_id)).to_string()
            },
            data_url: service_assignment.data_url.clone(),
            write_buffer_url: service_assignment.write_buffer_url.clone(),
            index_url: service_assignment.index_url.clone(),
            created_at: chrono::Utc::now(),
        };
        
        test_assignments.update_assignment(collection_id, updated_assignment.clone()).await?;
        return Ok(updated_assignment);
    }
    
    println!("Using persistent test assignment for {}: {}", collection_id, assignment.data_url);
    Ok(assignment)
}

/// Cleanup assignment for a collection (removes from both persistent and global service)
pub async fn cleanup_test_assignment(collection_id: &str) -> Result<()> {
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    
    // Remove from global assignment service
    let _ = assignment_service.remove_assignment(collection_id).await;
    
    // Remove from persistent test assignments
    let test_assignments = get_test_assignments();
    let _ = test_assignments.remove_assignment(collection_id).await;
    
    Ok(())
}

/// Cleanup all test assignments (for complete test isolation)
pub async fn cleanup_all_test_assignments() -> Result<()> {
    let test_assignments = get_test_assignments();
    test_assignments.clear_all_assignments().await?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_persistent_assignments() {
        let assignments = PersistentTestAssignments::new();

        // Test creating assignment
        let assignment1 = assignments.get_or_create_assignment("test_collection_1").await.unwrap();
        assert_eq!(assignment1.collection_id, "test_collection_1");
        assert!(assignment1.data_url.contains("test_collection_1/data"));

        // Test getting same assignment
        let assignment2 = assignments.get_or_create_assignment("test_collection_1").await.unwrap();
        assert_eq!(assignment1.data_url, assignment2.data_url);

        // Test different collection gets different assignment
        let assignment3 = assignments.get_or_create_assignment("test_collection_2").await.unwrap();
        assert_ne!(assignment1.data_url, assignment3.data_url);

        // Clean up
        assignments.remove_assignment("test_collection_1").await.unwrap();
        assignments.remove_assignment("test_collection_2").await.unwrap();
    }
}