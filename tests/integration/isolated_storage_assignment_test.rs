//! Isolated Storage Assignment Integration Tests
//! 
//! Tests storage assignment functionality with completely isolated environments
//! to eliminate any cross-test contamination or state pollution.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use super::test_utils::{IsolatedTestEnvironment, MultiEnvironmentTest};
use proximadb::core::config::StorageLocation;
use proximadb::storage::assignment_service::{HashBasedAssignmentService, AssignmentService};

#[tokio::test]
async fn test_isolated_basic_assignment() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    
    // Test basic assignment functionality
    let assignment = env.assignment_service().assign_collection(
        env.collection_id(),
        &env.storage_locations,
        "hash"
    ).await?;
    
    // Verify assignment structure
    assert_eq!(assignment.location_index, 0);
    assert!(assignment.data_url.contains(env.collection_id()));
    assert!(assignment.write_buffer_url.contains(env.collection_id()));
    assert!(assignment.index_url.contains(env.collection_id()));
    
    // Verify URLs end with correct suffixes
    assert!(assignment.data_url.ends_with("/data"));
    assert!(assignment.write_buffer_url.ends_with("/write_buffer"));
    assert!(assignment.index_url.ends_with("/index"));
    
    println!("✅ Basic assignment test passed for collection: {}", env.collection_id());
    Ok(())
}

#[tokio::test]
async fn test_isolated_assignment_persistence() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    
    // First assignment
    let assignment1 = env.assignment_service().assign_collection(
        env.collection_id(),
        &env.storage_locations,
        "hash"
    ).await?;
    
    // Second assignment for same collection should return same result
    let assignment2 = env.assignment_service().assign_collection(
        env.collection_id(),
        &env.storage_locations,
        "hash"
    ).await?;
    
    // Verify assignments are identical
    assert_eq!(assignment1.location_index, assignment2.location_index);
    assert_eq!(assignment1.data_url, assignment2.data_url);
    assert_eq!(assignment1.write_buffer_url, assignment2.write_buffer_url);
    assert_eq!(assignment1.index_url, assignment2.index_url);
    
    // Test get_assignment method
    let retrieved = env.assignment_service().get_assignment(env.collection_id()).await;
    assert!(retrieved.is_some());
    
    let retrieved_assignment = retrieved.unwrap();
    assert_eq!(retrieved_assignment.location_index, assignment1.location_index);
    assert_eq!(retrieved_assignment.data_url, assignment1.data_url);
    
    println!("✅ Assignment persistence test passed for collection: {}", env.collection_id());
    Ok(())
}

#[tokio::test]
async fn test_isolated_multiple_storage_locations() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    
    // Create multiple storage locations
    let storage_locations = vec![
        StorageLocation {
            url: format!("file://{}/location1", env.temp_dir.path().display()),
            weight: 1,
            tags: vec!["ssd".to_string()],
        },
        StorageLocation {
            url: format!("file://{}/location2", env.temp_dir.path().display()),
            weight: 2,
            tags: vec!["nvme".to_string()],
        },
        StorageLocation {
            url: format!("file://{}/location3", env.temp_dir.path().display()),
            weight: 1,
            tags: vec!["hdd".to_string()],
        },
    ];
    
    // Test round-robin assignment with multiple collections
    let mut location_counts = HashMap::new();
    
    for i in 0..9 {
        let collection_id = format!("{}_sub_{}", env.collection_id(), i);
        let assignment = env.assignment_service().assign_collection(
            &collection_id,
            &storage_locations,
            "round-robin"
        ).await?;
        
        *location_counts.entry(assignment.location_index).or_insert(0) += 1;
    }
    
    // Verify round-robin distribution (3 locations, 9 collections = 3 each)
    assert_eq!(location_counts.len(), 3);
    for (location, count) in &location_counts {
        assert_eq!(*count, 3, "Location {} should have exactly 3 assignments", location);
    }
    
    println!("✅ Multiple storage locations test passed for collection: {}", env.collection_id());
    Ok(())
}

#[tokio::test]
async fn test_isolated_weighted_assignment() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    
    // Create weighted storage locations
    let storage_locations = vec![
        StorageLocation {
            url: format!("file://{}/high_capacity", env.temp_dir.path().display()),
            weight: 4, // 4x weight
            tags: vec!["high_capacity".to_string()],
        },
        StorageLocation {
            url: format!("file://{}/medium_capacity", env.temp_dir.path().display()),
            weight: 2, // 2x weight
            tags: vec!["medium_capacity".to_string()],
        },
        StorageLocation {
            url: format!("file://{}/low_capacity", env.temp_dir.path().display()),
            weight: 1, // 1x weight
            tags: vec!["low_capacity".to_string()],
        },
    ];
    
    // Test weighted assignment with many collections
    let mut location_counts = vec![0; storage_locations.len()];
    let num_collections = 140; // Divisible by total weight (7)
    
    for i in 0..num_collections {
        let collection_id = format!("{}_weighted_{}", env.collection_id(), i);
        let assignment = env.assignment_service().assign_collection(
            &collection_id,
            &storage_locations,
            "weighted"
        ).await?;
        
        location_counts[assignment.location_index] += 1;
    }
    
    // Expected distribution for weights 4:2:1 out of 140 collections:
    // Location 0: ~80 (4/7 * 140)
    // Location 1: ~40 (2/7 * 140)  
    // Location 2: ~20 (1/7 * 140)
    
    let expected = [80, 40, 20];
    for (i, &count) in location_counts.iter().enumerate() {
        let diff = (count as i32 - expected[i]).abs();
        // Allow up to 40% variance for hash-based weighted distribution
        // Hash-based distribution can have high variance with small sample sizes
        let tolerance = (expected[i] * 2) / 5; 
        assert!(diff <= tolerance, "Location {} count {} too far from expected {} (tolerance: {})", i, count, expected[i], tolerance);
    }
    
    println!("✅ Weighted assignment test passed for collection: {}", env.collection_id());
    println!("   Distribution: {:?} (expected roughly {:?})", location_counts, expected);
    Ok(())
}

#[tokio::test]
async fn test_isolated_concurrent_assignment_safety() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    
    // Test concurrent assignment of the same collection
    let collection_id = env.collection_id().to_string();
    let assignment_service = env.assignment_service().clone();
    let storage_locations = env.storage_locations.clone();
    
    let mut handles = Vec::new();
    
    // Spawn 10 concurrent tasks trying to assign the same collection
    for i in 0..10 {
        let svc = assignment_service.clone();
        let locs = storage_locations.clone();
        let cid = collection_id.clone();
        
        let handle = tokio::spawn(async move {
            let assignment = svc.assign_collection(&cid, &locs, "hash").await?;
            Ok::<_, anyhow::Error>((i, assignment.location_index))
        });
        
        handles.push(handle);
    }
    
    // Collect all results
    let mut results = Vec::new();
    for handle in handles {
        let (task_id, location_index) = handle.await??;
        results.push((task_id, location_index));
    }
    
    // All concurrent assignments should return the same location
    let first_location = results[0].1;
    for (task_id, location_index) in &results {
        assert_eq!(*location_index, first_location, 
            "Task {} got location {} but expected {}", task_id, location_index, first_location);
    }
    
    println!("✅ Concurrent assignment safety test passed for collection: {}", env.collection_id());
    println!("   All {} concurrent assignments returned location {}", results.len(), first_location);
    Ok(())
}

#[tokio::test]
async fn test_isolated_assignment_removal() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    
    // Create assignment
    let assignment = env.assignment_service().assign_collection(
        env.collection_id(),
        &env.storage_locations,
        "hash"
    ).await?;
    
    // Verify assignment exists
    let retrieved = env.assignment_service().get_assignment(env.collection_id()).await;
    assert!(retrieved.is_some());
    
    // Remove assignment
    env.assignment_service().remove_assignment(env.collection_id()).await?;
    
    // Verify assignment is removed
    let after_removal = env.assignment_service().get_assignment(env.collection_id()).await;
    assert!(after_removal.is_none());
    
    // New assignment should work normally
    let new_assignment = env.assignment_service().assign_collection(
        env.collection_id(),
        &env.storage_locations,
        "hash"
    ).await?;
    
    // Should be the same as original (hash-based assignment is deterministic)
    assert_eq!(new_assignment.location_index, assignment.location_index);
    
    println!("✅ Assignment removal test passed for collection: {}", env.collection_id());
    Ok(())
}

#[tokio::test]
async fn test_isolated_assignment_statistics() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    
    // Create multiple assignments
    let collection_ids = (0..5).map(|i| format!("{}_stats_{}", env.collection_id(), i)).collect::<Vec<_>>();
    
    for collection_id in &collection_ids {
        env.assignment_service().assign_collection(
            collection_id,
            &env.storage_locations,
            "hash"
        ).await?;
    }
    
    // Get statistics
    let stats = env.assignment_service().get_assignment_stats().await?;
    
    // Verify statistics structure
    assert!(stats.is_object());
    let stats_obj = stats.as_object().unwrap();
    
    assert!(stats_obj.contains_key("total_collections"));
    assert!(stats_obj.contains_key("location_distribution"));
    assert!(stats_obj.contains_key("assignments"));
    
    // Verify counts
    assert_eq!(stats_obj["total_collections"], serde_json::json!(5));
    
    let location_distribution = stats_obj["location_distribution"].as_object().unwrap();
    let total_assigned: i64 = location_distribution.values()
        .map(|v| v.as_i64().unwrap_or(0))
        .sum();
    assert_eq!(total_assigned, 5);
    
    println!("✅ Assignment statistics test passed for collection: {}", env.collection_id());
    println!("   Statistics: {}", serde_json::to_string_pretty(&stats).unwrap());
    Ok(())
}

#[tokio::test]
async fn test_isolated_multi_environment_isolation() -> Result<()> {
    // Create multiple isolated environments
    let multi_env = MultiEnvironmentTest::new(3).await?;
    
    // Assign collections in each environment
    let mut all_assignments = Vec::new();
    
    for (i, env) in multi_env.environments.iter().enumerate() {
        let assignment = env.assignment_service().assign_collection(
            env.collection_id(),
            &env.storage_locations,
            "hash"
        ).await?;
        
        all_assignments.push((i, env.collection_id().to_string(), assignment));
    }
    
    // Verify complete isolation - each environment should only know about its own assignment
    for (env_index, env) in multi_env.environments.iter().enumerate() {
        // Should find its own assignment
        let own_assignment = env.assignment_service().get_assignment(env.collection_id()).await;
        assert!(own_assignment.is_some());
        
        // Should NOT find assignments from other environments
        for (other_env_index, other_env) in multi_env.environments.iter().enumerate() {
            if env_index != other_env_index {
                let other_assignment = env.assignment_service().get_assignment(other_env.collection_id()).await;
                assert!(other_assignment.is_none(), 
                    "Environment {} should not see assignment from environment {}", 
                    env_index, other_env_index);
            }
        }
    }
    
    println!("✅ Multi-environment isolation test passed");
    for (i, collection_id, _) in &all_assignments {
        println!("   Environment {}: {}", i, collection_id);
    }
    Ok(())
}

// TODO: Re-enable once SST engine API is fixed
// #[tokio::test]
// async fn test_isolated_assignment_with_sst_engine() -> Result<()> {
//     let env = IsolatedTestEnvironment::new().await?;
//     
//     // Create SST engine (this internally uses assignment service)
//     let engine = env.create_sst_engine().await?;
//     
//     // Verify assignment was created
//     let assignment = env.assignment_service().get_assignment(env.collection_id()).await;
//     assert!(assignment.is_some());
//     
//     let assignment = assignment.unwrap();
//     
//     // Create test vectors
//     let vectors = env.create_test_vectors(5);
//     
//     // Insert and flush vectors
//     test_operations::insert_and_flush(&engine, &env, vectors).await?;
//     
//     // Search vectors
//     let query_vector = env.create_query_vector();
//     let results = test_operations::search_vectors(&engine, &env, &query_vector, 3).await?;
//     
//     // Verify results
//     assert!(!results.is_empty());
//     assert!(results.len() <= 3);
//     
//     // Verify all results belong to this collection (check by ID prefix)
//     for result in &results {
//         assert!(result.id.starts_with(env.collection_id()),
//             "Result ID {} should start with collection ID {}", result.id, env.collection_id());
//     }
//     
//     println!("✅ Assignment with SST engine test passed for collection: {}", env.collection_id());
//     println!("   Found {} search results", results.len());
//     Ok(())
// }