#[cfg(test)]
mod assignment_service_advanced_tests {
    use anyhow::Result;
    use std::sync::Arc;
    use tempfile::TempDir;
    use std::collections::HashSet;
    
    use proximadb::storage::assignment_service::{
        get_assignment_service, AssignmentService, HashBasedAssignmentService,
    };
    use proximadb::core::config::StorageLocation;
    
    #[tokio::test]
    async fn test_assignment_service_singleton() {
        // Test that get_assignment_service returns the same instance
        let service1 = get_assignment_service();
        let service2 = get_assignment_service();
        
        // Should be the same Arc instance
        assert!(Arc::ptr_eq(&service1, &service2));
    }
    
    #[tokio::test]
    async fn test_assignment_persistence_across_calls() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let service = get_assignment_service();
        
        let locations = vec![
            StorageLocation {
                url: format!("file://{}/storage1", temp_dir.path().display()),
                weight: 1,
                tags: vec!["test".to_string()],
            },
            StorageLocation {
                url: format!("file://{}/storage2", temp_dir.path().display()),
                weight: 1,
                tags: vec!["test".to_string()],
            },
        ];
        
        // Assign a collection
        let collection_id = "persist_test";
        let assignment1 = service.assign_collection(
            collection_id,
            &locations,
            "round-robin"
        ).await?;
        
        // Get the assignment again - should return the same location
        let existing = service.get_assignment(collection_id).await;
        assert!(existing.is_some());
        
        let existing_assignment = existing.unwrap();
        assert_eq!(existing_assignment.location_index, assignment1.location_index);
        assert_eq!(existing_assignment.data_url, assignment1.data_url);
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_assignment_distribution_fairness() -> Result<()> {
        // Create a new instance for this test to avoid global state issues
        let service = Arc::new(proximadb::storage::assignment_service::HashBasedAssignmentService::new());
        
        let locations = vec![
            StorageLocation {
                url: "file:///test/loc1".to_string(),
                weight: 1,
                tags: vec![],
            },
            StorageLocation {
                url: "file:///test/loc2".to_string(),
                weight: 1,
                tags: vec![],
            },
            StorageLocation {
                url: "file:///test/loc3".to_string(),
                weight: 1,
                tags: vec![],
            },
            StorageLocation {
                url: "file:///test/loc4".to_string(),
                weight: 1,
                tags: vec![],
            },
        ];
        
        // Assign many collections and check distribution
        let mut location_counts = vec![0; locations.len()];
        let num_collections = 400;
        
        for i in 0..num_collections {
            let assignment = service.assign_collection(
                &format!("fairness_test_{}", i),
                &locations,
                "round-robin"
            ).await?;
            
            location_counts[assignment.location_index] += 1;
        }
        
        // Each location should get exactly 100 assignments with round-robin
        for (idx, count) in location_counts.iter().enumerate() {
            assert_eq!(*count, 100, "Location {} got {} assignments instead of 100", idx, count);
        }
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_assignment_with_different_weights() -> Result<()> {
        let service = get_assignment_service();
        
        let locations = vec![
            StorageLocation {
                url: "file:///weighted/high".to_string(),
                weight: 4, // 4x weight
                tags: vec!["high-capacity".to_string()],
            },
            StorageLocation {
                url: "file:///weighted/medium".to_string(),
                weight: 2, // 2x weight
                tags: vec!["medium-capacity".to_string()],
            },
            StorageLocation {
                url: "file:///weighted/low".to_string(),
                weight: 1, // 1x weight
                tags: vec!["low-capacity".to_string()],
            },
        ];
        
        // With weights 4:2:1, total weight is 7
        // Expected distribution for 700 collections:
        // Location 0: ~400 (4/7)
        // Location 1: ~200 (2/7)
        // Location 2: ~100 (1/7)
        
        let mut location_counts = vec![0; locations.len()];
        let num_collections = 700;
        
        for i in 0..num_collections {
            let assignment = service.assign_collection(
                &format!("weighted_test_{}", i),
                &locations,
                "weighted"
            ).await?;
            
            location_counts[assignment.location_index] += 1;
        }
        
        // Check that distribution roughly matches weights
        // Allow 10% variance
        assert!((location_counts[0] as f64 - 400.0).abs() < 40.0);
        assert!((location_counts[1] as f64 - 200.0).abs() < 20.0);
        assert!((location_counts[2] as f64 - 100.0).abs() < 10.0);
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_assignment_url_normalization() -> Result<()> {
        let service = get_assignment_service();
        
        // Test URLs with various formats
        let test_cases = vec![
            ("file:///path/to/storage/", "collection1"),
            ("s3://bucket/prefix", "collection2"),
            ("wasbs://container@account.blob.core.windows.net/path/", "collection3"),
            ("gs://bucket/nested/path//", "collection4"),
        ];
        
        for (base_url, collection_id) in test_cases {
            let locations = vec![StorageLocation {
                url: base_url.to_string(),
                weight: 1,
                tags: vec![],
            }];
            
            let assignment = service.assign_collection(
                collection_id,
                &locations,
                "round-robin"
            ).await?;
            
            // Check that URLs are properly constructed
            assert!(!assignment.write_buffer_url.contains("//wal"));
            assert!(!assignment.data_url.contains("//data"));
            assert!(!assignment.index_url.contains("//index"));
            
            // Check that collection_id is included
            assert!(assignment.write_buffer_url.contains(collection_id));
            assert!(assignment.data_url.contains(collection_id));
            assert!(assignment.index_url.contains(collection_id));
        }
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_concurrent_assignment_safety() -> Result<()> {
        // Use a fresh service instance to avoid global state pollution
        let service = Arc::new(HashBasedAssignmentService::new());
        
        let locations = vec![
            StorageLocation {
                url: "file:///concurrent/loc1".to_string(),
                weight: 1,
                tags: vec![],
            },
            StorageLocation {
                url: "file:///concurrent/loc2".to_string(),
                weight: 1,
                tags: vec![],
            },
        ];
        
        // Test that the same collection assigned concurrently gets the same location
        let collection_id = "concurrent_same_collection_unique_123";
        
        // Ensure no existing assignment for this collection
        let _ = service.remove_assignment(collection_id).await;
        
        let mut handles = Vec::new();
        
        for _ in 0..10 {
            let svc = service.clone();
            let locs = locations.clone();
            let cid = collection_id.to_string();
            
            let handle = tokio::spawn(async move {
                svc.assign_collection(&cid, &locs, "round-robin").await
            });
            
            handles.push(handle);
        }
        
        let mut assignments = Vec::new();
        for handle in handles {
            let assignment = handle.await??;
            assignments.push(assignment.location_index);
        }
        
        // All concurrent assignments for the same collection should return the same location
        let first_location = assignments[0];
        for location in &assignments {
            assert_eq!(*location, first_location);
        }
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_assignment_with_empty_strategy() -> Result<()> {
        let service = get_assignment_service();
        
        let locations = vec![
            StorageLocation {
                url: "file:///test".to_string(),
                weight: 1,
                tags: vec![],
            },
        ];
        
        // Test with empty strategy (should use default)
        let assignment = service.assign_collection(
            "empty_strategy_test",
            &locations,
            ""
        ).await?;
        
        // Should still succeed with default strategy
        assert_eq!(assignment.location_index, 0);
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_assignment_idempotency() -> Result<()> {
        let service = get_assignment_service();
        
        let locations = vec![
            StorageLocation {
                url: "file:///idempotent/1".to_string(),
                weight: 1,
                tags: vec!["ssd".to_string()],
            },
            StorageLocation {
                url: "file:///idempotent/2".to_string(),
                weight: 1,
                tags: vec!["hdd".to_string()],
            },
        ];
        
        let collection_id = "idempotent_test";
        
        // Assign multiple times
        let mut assignments = Vec::new();
        for _ in 0..5 {
            let assignment = service.assign_collection(
                collection_id,
                &locations,
                "round-robin"
            ).await?;
            assignments.push(assignment);
        }
        
        // All assignments should be identical
        for i in 1..assignments.len() {
            assert_eq!(assignments[i].location_index, assignments[0].location_index);
            assert_eq!(assignments[i].data_url, assignments[0].data_url);
            assert_eq!(assignments[i].write_buffer_url, assignments[0].write_buffer_url);
            assert_eq!(assignments[i].index_url, assignments[0].index_url);
        }
        
        Ok(())
    }
}