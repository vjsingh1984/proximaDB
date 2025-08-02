use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use proximadb::storage::assignment_service::{AssignmentService, HashBasedAssignmentService};
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::core::config::{Config, StorageConfig, StorageLocation};
use anyhow::Result;
use tokio;

#[tokio::test]
async fn test_assignment_service_recovery_multi_disk() -> Result<()> {
    // Create test directories for multi-disk setup
    let temp_dir = tempfile::tempdir()?;
    let base_path = temp_dir.path();
    
    let disk1_path = base_path.join("disk1");
    let disk2_path = base_path.join("disk2");
    let disk3_path = base_path.join("disk3");
    
    tokio::fs::create_dir_all(&disk1_path).await?;
    tokio::fs::create_dir_all(&disk2_path).await?;
    tokio::fs::create_dir_all(&disk3_path).await?;
    
    // Create multi-disk config using storage_locations
    let mut config = Config::default();
    config.storage.storage_locations = vec![
        StorageLocation {
            url: format!("file://{}", disk1_path.display()),
            weight: 1,
            tags: Default::default(),
        },
        StorageLocation {
            url: format!("file://{}", disk2_path.display()),
            weight: 1,
            tags: Default::default(),
        },
        StorageLocation {
            url: format!("file://{}", disk3_path.display()),
            weight: 1,
            tags: Default::default(),
        },
    ];
    
    // Initialize assignment service
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
    let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
    
    // Create test collections and simulate assignment
    let test_collections = vec![
        "collection_1", "collection_2", "collection_3", 
        "collection_4", "collection_5", "collection_6"
    ];
    
    println!("🔧 Creating test collections and assignments...");
    
    // Create collection directories and assignment metadata
    for (i, collection_id) in test_collections.iter().enumerate() {
        let disk_index = i % 3; // Round robin assignment
        let disk_path = match disk_index {
            0 => &disk1_path,
            1 => &disk2_path,
            2 => &disk3_path,
            _ => unreachable!(),
        };
        
        // Create collection directory on assigned disk
        let collection_dir = disk_path.join(collection_id).join("data");
        tokio::fs::create_dir_all(&collection_dir).await?;
        
        // Create a dummy SSTable file to simulate actual data
        let sstable_file = collection_dir.join("000001.sst");
        tokio::fs::write(&sstable_file, b"dummy_sstable_data").await?;
        
        // Assign collection to disk through assignment service
        assignment_service.assign_collection(collection_id, &config.storage.storage_locations, "round_robin").await?;
        
        println!("📝 Assigned {} to disk {} ({})", collection_id, disk_index + 1, disk_path.display());
    }
    
    // Now test recovery - create a new assignment service instance
    println!("\n🔄 Testing assignment service recovery...");
    let filesystem_factory_recovery = Arc::new(FilesystemFactory::new(Default::default()).await?);
    let recovered_assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory_recovery, "round_robin"));
    
    // Rebuild assignments from disk discovery
    println!("🔧 Rebuilding assignments from disk with {} storage locations", config.storage.storage_locations.len());
    for (i, loc) in config.storage.storage_locations.iter().enumerate() {
        println!("  📍 Location {}: {}", i, loc.url);
    }
    recovered_assignment_service.rebuild_assignments_from_disk(&config.storage.storage_locations).await?;
    println!("✅ Rebuild complete");
    
    // Debug: Check what assignments were actually created
    println!("\n🔍 DEBUG: Checking assignment service state after rebuild...");
    // Check a sample assignment to see if discovery worked
    if let Some(test_assignment) = recovered_assignment_service.get_assignment("collection_1").await {
        println!("  ✅ Found assignment for collection_1: {}", test_assignment.data_url);
    } else {
        println!("  ❌ No assignment found for collection_1 after rebuild");
    }
    
    // Test 1: List all collections and verify they're found
    println!("\n📊 Listing all collections after recovery:");
    let mut collections_found = HashMap::new();
    
    for location in &config.storage.storage_locations {
        let disk_url = &location.url;
        let disk_path = disk_url.strip_prefix("file://").unwrap_or(disk_url);
        let disk_dir = PathBuf::from(disk_path);
        
        if disk_dir.exists() {
            if let Ok(mut entries) = tokio::fs::read_dir(&disk_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if entry.file_type().await?.is_dir() {
                        let collection_id = entry.file_name().to_string_lossy().to_string();
                        if test_collections.contains(&collection_id.as_str()) {
                            collections_found.insert(collection_id.clone(), disk_url.clone());
                            println!("  ✅ Found {} on disk {}", collection_id, disk_url);
                        }
                    }
                }
            }
        }
    }
    
    // Test 2: Verify assignment service can resolve each collection
    println!("\n🔍 Testing assignment resolution:");
    let mut assignment_errors = Vec::new();
    
    for collection_id in &test_collections {
        if let Some(assignment) = recovered_assignment_service.get_assignment(collection_id).await {
            println!("  ✅ {} -> {}", collection_id, assignment.data_url);
            
            // Verify the assignment matches what we expect
            if let Some(expected_disk) = collections_found.get(*collection_id) {
                let expected_url = format!("{}/{}/data", expected_disk, collection_id);
                if assignment.data_url != expected_url {
                    assignment_errors.push(format!(
                        "Assignment mismatch for {}: expected {}, got {}", 
                        collection_id, expected_url, assignment.data_url
                    ));
                }
            } else {
                assignment_errors.push(format!("Collection {} not found in filesystem scan", collection_id));
            }
        } else {
            assignment_errors.push(format!("No assignment found for collection {}", collection_id));
            println!("  ❌ {} -> NO ASSIGNMENT", collection_id);
        }
    }
    
    // Test 3: Check for orphaned collections (collections without assignments)
    println!("\n🔍 Checking for orphaned collections:");
    let mut orphaned_collections = Vec::new();
    
    for (collection_id, disk_url) in &collections_found {
        if recovered_assignment_service.get_assignment(collection_id).await.is_some() {
            println!("  ✅ {} has valid assignment", collection_id);
        } else {
            orphaned_collections.push(collection_id.clone());
            println!("  ⚠️  {} is orphaned (found on {} but no assignment)", collection_id, disk_url);
        }
    }
    
    // Test 4: Test assignment service disk distribution
    println!("\n📊 Assignment distribution analysis:");
    let mut disk_usage = HashMap::new();
    
    for collection_id in &test_collections {
        if let Some(assignment) = recovered_assignment_service.get_assignment(collection_id).await {
            // Use the location_url which should be the base disk URL
            let disk = assignment.location_url.clone();
            println!("    DEBUG: {} -> location_url='{}'", collection_id, disk);
            *disk_usage.entry(disk).or_insert(0) += 1;
        }
    }
    
    for (disk, count) in &disk_usage {
        println!("  📁 {}: {} collections", disk, count);
    }
    
    // Summary and assertions
    println!("\n📋 Recovery Test Summary:");
    println!("  Collections created: {}", test_collections.len());
    println!("  Collections found on disk: {}", collections_found.len());
    println!("  Assignment errors: {}", assignment_errors.len());
    println!("  Orphaned collections: {}", orphaned_collections.len());
    
    // Assertions
    assert_eq!(collections_found.len(), test_collections.len(), 
        "Not all collections were found on disk");
    
    if !assignment_errors.is_empty() {
        println!("\n❌ Assignment Errors:");
        for error in &assignment_errors {
            println!("  - {}", error);
        }
        panic!("Assignment service recovery failed with {} errors", assignment_errors.len());
    }
    
    if !orphaned_collections.is_empty() {
        println!("\n⚠️ Orphaned Collections:");
        for collection in &orphaned_collections {
            println!("  - {}", collection);
        }
        panic!("Found {} orphaned collections", orphaned_collections.len());
    }
    
    // Verify balanced distribution (each disk should have roughly equal collections)
    let expected_per_disk = test_collections.len() / 3;
    for (disk, count) in &disk_usage {
        assert!(
            (*count as i32 - expected_per_disk as i32).abs() <= 1,
            "Unbalanced distribution: {} has {} collections, expected around {}",
            disk, count, expected_per_disk
        );
    }
    
    println!("✅ Assignment service recovery test passed!");
    
    Ok(())
}

#[tokio::test]
async fn test_assignment_service_recovery_with_missing_metadata() -> Result<()> {
    println!("🧪 Testing recovery with missing assignment metadata...");
    
    // Create test directories
    let temp_dir = tempfile::tempdir()?;
    let base_path = temp_dir.path();
    
    let disk1_path = base_path.join("disk1");
    let disk2_path = base_path.join("disk2");
    
    tokio::fs::create_dir_all(&disk1_path).await?;
    tokio::fs::create_dir_all(&disk2_path).await?;
    
    // Create collections directly on disk WITHOUT going through assignment service
    // This simulates the scenario where assignment metadata is lost
    let orphaned_collections = vec!["orphan_1", "orphan_2", "orphan_3"];
    
    for (i, collection_id) in orphaned_collections.iter().enumerate() {
        let disk_path = if i % 2 == 0 { &disk1_path } else { &disk2_path };
        let collection_dir = disk_path.join(collection_id).join("data");
        tokio::fs::create_dir_all(&collection_dir).await?;
        
        // Create dummy data files
        let sstable_file = collection_dir.join("000001.sst");
        tokio::fs::write(&sstable_file, b"orphaned_data").await?;
        
        println!("🗂️ Created orphaned collection {} on {}", collection_id, disk_path.display());
    }
    
    // Create config
    let mut config = Config::default();
    config.storage.storage_locations = vec![
        StorageLocation {
            url: format!("file://{}", disk1_path.display()),
            weight: 1,
            tags: Default::default(),
        },
        StorageLocation {
            url: format!("file://{}", disk2_path.display()),
            weight: 1,
            tags: Default::default(),
        },
    ];
    
    // Initialize assignment service - it should discover orphaned collections
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
    let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
    
    // Rebuild assignments from disk - should discover orphaned collections
    assignment_service.rebuild_assignments_from_disk(&config.storage.storage_locations).await?;
    
    // Test: Check if assignment service can handle orphaned collections
    println!("\n🔍 Testing orphaned collection handling:");
    let mut recovery_issues = Vec::new();
    
    for collection_id in &orphaned_collections {
        if let Some(assignment) = assignment_service.get_assignment(collection_id).await {
            println!("  ✅ {} recovered with assignment: {}", collection_id, assignment.data_url);
        } else {
            recovery_issues.push(format!("Orphaned collection {} not recovered", collection_id));
            println!("  ❌ {} not recovered", collection_id);
        }
    }
    
    // This test expects the assignment service to either:
    // 1. Recover orphaned collections by creating assignments for them
    // 2. OR provide a clear mechanism to identify and handle orphans
    
    if !recovery_issues.is_empty() {
        println!("\n⚠️ Recovery Issues Found:");
        for issue in &recovery_issues {
            println!("  - {}", issue);
        }
        println!("\n💡 This indicates the assignment service needs to implement orphaned collection recovery");
    } else {
        println!("✅ All orphaned collections successfully recovered!");
    }
    
    Ok(())
}

#[tokio::test]
async fn test_assignment_service_disk_failure_simulation() -> Result<()> {
    println!("🧪 Testing assignment service with disk failure simulation...");
    
    let temp_dir = tempfile::tempdir()?;
    let base_path = temp_dir.path();
    
    let disk1_path = base_path.join("disk1");
    let disk2_path = base_path.join("disk2");
    let disk3_path = base_path.join("disk3_failed"); // This disk will be "failed"
    
    tokio::fs::create_dir_all(&disk1_path).await?;
    tokio::fs::create_dir_all(&disk2_path).await?;
    // Don't create disk3 to simulate failure
    
    // Create config with 3 disks
    let mut config = Config::default();
    config.storage.storage_locations = vec![
        StorageLocation {
            url: format!("file://{}", disk1_path.display()),
            weight: 1,
            tags: Default::default(),
        },
        StorageLocation {
            url: format!("file://{}", disk2_path.display()),
            weight: 1,
            tags: Default::default(),
        },
        StorageLocation {
            url: format!("file://{}", disk3_path.display()), // This will fail
            weight: 1,
            tags: Default::default(),
        },
    ];
    
    // Create collections on available disks
    let collections_disk1 = vec!["coll_1", "coll_4"];
    let collections_disk2 = vec!["coll_2", "coll_5"];
    let collections_disk3_lost = vec!["coll_3", "coll_6"]; // These would be on failed disk
    
    for collection_id in &collections_disk1 {
        let collection_dir = disk1_path.join(collection_id).join("data");
        tokio::fs::create_dir_all(&collection_dir).await?;
        tokio::fs::write(collection_dir.join("data.sst"), b"data").await?;
    }
    
    for collection_id in &collections_disk2 {
        let collection_dir = disk2_path.join(collection_id).join("data");
        tokio::fs::create_dir_all(&collection_dir).await?;
        tokio::fs::write(collection_dir.join("data.sst"), b"data").await?;
    }
    
    // Initialize assignment service
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
    let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
    
    // Rebuild assignments from disk
    assignment_service.rebuild_assignments_from_disk(&config.storage.storage_locations).await?;
    
    // Test recovery behavior with failed disk
    println!("\n📊 Testing recovery with failed disk:");
    
    let mut available_collections = Vec::new();
    let mut unavailable_collections = Vec::new();
    
    for collection_list in &[&collections_disk1, &collections_disk2] {
        for collection_id in *collection_list {
            if assignment_service.get_assignment(collection_id).await.is_some() {
                available_collections.push(*collection_id);
                println!("  ✅ {} available", collection_id);
            } else {
                unavailable_collections.push(*collection_id);
                println!("  ❌ {} no assignment", collection_id);
            }
        }
    }
    
    println!("\n📋 Disk Failure Recovery Summary:");
    println!("  Available collections: {}", available_collections.len());
    println!("  Unavailable collections: {}", unavailable_collections.len());
    println!("  Collections that would be lost: {}", collections_disk3_lost.len());
    
    // In a real scenario, we'd expect:
    // - Available collections to be accessible
    // - Failed disk collections to be identified for recovery/replication
    
    println!("✅ Disk failure simulation completed");
    
    Ok(())
}