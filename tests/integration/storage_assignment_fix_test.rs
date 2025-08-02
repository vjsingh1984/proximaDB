// Test for the specific storage assignment fix
// Verifies that assignments are stored with collection UUID, not name

use std::sync::Arc;
use anyhow::Result;
use tempfile::TempDir;

use proximadb::core::config::{StorageConfig, StorageLocation, AssignmentConfig};
use proximadb::services::collection_service::CollectionService;
use proximadb::storage::metadata::backends::filestore_backend::FilestoreMetadataBackend;
use proximadb::storage::assignment_service::{get_assignment_service, HashBasedAssignmentService, set_assignment_service};
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::proto::proximadb::CollectionConfig;

#[tokio::test]
async fn test_assignment_stored_with_uuid_not_name() -> Result<()> {
    // Create temporary directory for test
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_str().unwrap();
    
    // Setup storage configuration
    let storage_config = StorageConfig {
        metadata_url: format!("file://{}/metadata", temp_path),
        storage_locations: vec![StorageLocation {
            url: format!("file://{}/data", temp_path),
            weight: 1,
            tags: vec!["test".to_string()],
        }],
        assignment_config: AssignmentConfig {
            strategy: "hash".to_string(),
            affinity: true,
        },
        ..Default::default()
    };
    
    // Initialize assignment service
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
    set_assignment_service(assignment_service.clone())?;
    
    // Create metadata backend
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(&storage_config.metadata_url, storage_config.clone()).await?
    );
    
    // Create collection service
    let collection_service = Arc::new(
        CollectionService::new(metadata_backend.clone(), storage_config.clone()).await?
    );
    
    // Test data
    let collection_name = "test_assignment_collection_long_name"; // Must be >= 8 chars
    
    // Step 1: Create a collection
    let collection_config = CollectionConfig {
        name: collection_name.to_string(),
        dimension: 128,
        distance_metric: "cosine".to_string(),
        engine: "viper".to_string(),
        ..Default::default()
    };
    
    let create_response = collection_service.create_collection(&collection_config).await?;
    assert!(create_response.success, "Collection creation failed: {:?}", create_response.error_message);
    
    let collection = create_response.collection.unwrap();
    let collection_id = collection.id.clone();
    let collection_name_from_config = collection.config.unwrap().name.clone();
    
    println!("✅ Created collection: name='{}', id='{}'", collection_name_from_config, collection_id);
    
    // Step 2: CRITICAL TEST - Assignment should exist for collection UUID
    let assignment_by_uuid = assignment_service.get_assignment(&collection_id).await;
    assert!(assignment_by_uuid.is_some(), 
            "❌ FAILED: No assignment found for collection UUID: '{}'. This indicates the assignment fix didn't work!", 
            collection_id);
    
    let assignment = assignment_by_uuid.unwrap();
    println!("✅ PASS: Found assignment for collection UUID '{}': WAL={}, Data={}", 
             collection_id, assignment.write_buffer_url, assignment.data_url);
    
    // Step 3: Assignment should NOT exist for collection name (old broken behavior)
    let assignment_by_name = assignment_service.get_assignment(&collection_name).await;
    assert!(assignment_by_name.is_none(), 
            "❌ FAILED: Assignment found for collection name '{}' - assignments should only be stored by UUID!", 
            collection_name);
    
    println!("✅ PASS: No assignment found for collection name '{}' (correct behavior)", collection_name);
    
    // Step 4: Test collection name resolution to UUID
    let resolved_id = collection_service.resolve_collection_id(&collection_name).await?;
    assert_eq!(resolved_id, Some(collection_id.clone()), 
               "❌ FAILED: Collection name '{}' should resolve to UUID '{}'", 
               collection_name, collection_id);
    
    println!("✅ PASS: Collection name '{}' correctly resolves to UUID '{}'", collection_name, collection_id);
    
    // Step 5: Test UUID resolution (should return same UUID)
    let resolved_id_from_uuid = collection_service.resolve_collection_id(&collection_id).await?;
    assert_eq!(resolved_id_from_uuid, Some(collection_id.clone()),
               "❌ FAILED: Collection UUID '{}' should resolve to itself", 
               collection_id);
    
    println!("✅ PASS: Collection UUID '{}' correctly resolves to itself", collection_id);
    
    // Step 6: Verify assignment URLs are correctly constructed
    assert!(assignment.write_buffer_url.contains(&collection_id),
            "❌ FAILED: WAL URL should contain collection UUID: '{}'", assignment.write_buffer_url);
    assert!(assignment.data_url.contains(&collection_id),
            "❌ FAILED: Data URL should contain collection UUID: '{}'", assignment.data_url);
    assert!(assignment.index_url.contains(&collection_id),
            "❌ FAILED: Index URL should contain collection UUID: '{}'", assignment.index_url);
    
    println!("✅ PASS: All assignment URLs correctly contain collection UUID");
    
    println!("🎉 STORAGE ASSIGNMENT FIX VERIFIED: All tests passed!");
    
    Ok(())
}

#[tokio::test]
async fn test_assignment_recovery_with_uuid() -> Result<()> {
    // Create temporary directory for test
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_str().unwrap();
    
    let storage_config = StorageConfig {
        metadata_url: format!("file://{}/metadata", temp_path),
        storage_locations: vec![StorageLocation {
            url: format!("file://{}/data", temp_path),
            weight: 1,
            tags: vec!["test".to_string()],
        }],
        assignment_config: AssignmentConfig {
            strategy: "hash".to_string(),
            affinity: true,
        },
        ..Default::default()
    };
    
    let collection_name = "recovery_test_collection";
    let collection_id: String;
    
    // Phase 1: Create collection and assignment
    {
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        set_assignment_service(assignment_service.clone())?;
        
        let metadata_backend = Arc::new(
            FilestoreMetadataBackend::new(&storage_config.metadata_url, storage_config.clone()).await?
        );
        
        let collection_service = Arc::new(
            CollectionService::new(metadata_backend, storage_config.clone()).await?
        );
        
        let collection_config = CollectionConfig {
            name: collection_name.to_string(),
            dimension: 64,
            distance_metric: "cosine".to_string(),
            engine: "viper".to_string(),
            ..Default::default()
        };
        
        let create_response = collection_service.create_collection(&collection_config).await?;
        assert!(create_response.success);
        
        collection_id = create_response.collection.unwrap().id;
        
        // Verify assignment exists for UUID
        let assignment = assignment_service.get_assignment(&collection_id).await;
        assert!(assignment.is_some(), "Assignment should exist for UUID after creation");
        
        // Create actual directories to simulate real data
        let assignment = assignment.unwrap();
        let filesystem_factory = proximadb::storage::persistence::filesystem::FilesystemFactory::new(Default::default()).await?;
        let filesystem = filesystem_factory.get_filesystem(&assignment.location_url)?;
        filesystem.create_dir_all(&assignment.write_buffer_url).await?;
        filesystem.create_dir_all(&assignment.data_url).await?;
        filesystem.create_dir_all(&assignment.index_url).await?;
        
        println!("✅ Phase 1: Created collection '{}' with UUID '{}' and directories", collection_name, collection_id);
    }
    
    // Phase 2: Simulate restart - new assignment service instance
    {
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let new_assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        set_assignment_service(new_assignment_service.clone())?;
        
        // Assignment should not exist in new instance (until discovery)
        let assignment = new_assignment_service.get_assignment(&collection_id).await;
        assert!(assignment.is_none(), "Assignment should not exist in new instance before discovery");
        
        // Simulate discovery process
        let recovery_report = new_assignment_service
            .discover_and_recover(&storage_config.storage_locations)
            .await?;
        
        println!("✅ Phase 2: Discovery completed, found {} collections", 
                recovery_report.discovered_collections.len());
        
        // Assignment should now exist after discovery for UUID (not name)
        let recovered_assignment = new_assignment_service.get_assignment(&collection_id).await;
        assert!(recovered_assignment.is_some(), "Assignment should exist for UUID after discovery");
        
        // Assignment should NOT exist for name
        let name_assignment = new_assignment_service.get_assignment(collection_name).await;
        assert!(name_assignment.is_none(), "Assignment should not exist for name after discovery");
        
        println!("✅ Assignment recovered successfully for UUID '{}'", collection_id);
    }
    
    println!("🎉 ASSIGNMENT RECOVERY TEST PASSED!");
    
    Ok(())
}

#[tokio::test] 
async fn test_multiple_collections_assignment_uniqueness() -> Result<()> {
    use std::collections::HashSet;
    
    // Create temporary directory
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_str().unwrap();
    
    let storage_config = StorageConfig {
        metadata_url: format!("file://{}/metadata", temp_path),
        storage_locations: vec![
            StorageLocation {
                url: format!("file://{}/data1", temp_path),
                weight: 1,
                tags: vec!["test".to_string()],
            },
            StorageLocation {
                url: format!("file://{}/data2", temp_path),
                weight: 1,
                tags: vec!["test".to_string()],
            },
        ],
        assignment_config: AssignmentConfig {
            strategy: "hash".to_string(),
            affinity: true,
        },
        ..Default::default()
    };
    
    // Initialize services
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
    set_assignment_service(assignment_service.clone())?;
    
    let metadata_backend = Arc::new(
        FilestoreMetadataBackend::new(&storage_config.metadata_url, storage_config.clone()).await?
    );
    
    let collection_service = Arc::new(
        CollectionService::new(metadata_backend.clone(), storage_config.clone()).await?
    );
    
    // Create multiple collections
    let mut collection_ids = Vec::new();
    let mut assignment_urls = HashSet::new();
    
    for i in 0..5 {
        let collection_name = format!("multi_collection_test_{:02}", i);
        
        let collection_config = CollectionConfig {
            name: collection_name.clone(),
            dimension: 32,
            distance_metric: "cosine".to_string(),
            engine: "viper".to_string(),
            ..Default::default()
        };
        
        let create_response = collection_service.create_collection(&collection_config).await?;
        assert!(create_response.success, "Failed to create collection {}", collection_name);
        
        let collection_id = create_response.collection.unwrap().id;
        collection_ids.push(collection_id.clone());
        
        // Verify assignment exists for UUID
        let assignment = assignment_service.get_assignment(&collection_id).await;
        assert!(assignment.is_some(), "Assignment missing for collection {}", collection_id);
        
        let assignment = assignment.unwrap();
        
        // Check assignment uniqueness
        let assignment_key = format!("{}|{}|{}", assignment.write_buffer_url, assignment.data_url, assignment.index_url);
        assert!(!assignment_urls.contains(&assignment_key), 
                "Duplicate assignment detected for collection {}: {}", collection_id, assignment_key);
        assignment_urls.insert(assignment_key);
        
        println!("✅ Collection {} (UUID: {}) has unique assignment", collection_name, collection_id);
    }
    
    // Verify all assignments exist
    for collection_id in &collection_ids {
        let assignment = assignment_service.get_assignment(collection_id).await;
        assert!(assignment.is_some(), "Assignment missing for collection {}", collection_id);
    }
    
    println!("🎉 MULTIPLE COLLECTIONS TEST PASSED: {} collections with unique assignments", collection_ids.len());
    
    Ok(())
}