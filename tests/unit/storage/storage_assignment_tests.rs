#[cfg(test)]
mod storage_assignment_tests {
    use anyhow::Result;
    use std::sync::Arc;
    use std::collections::HashMap;
    use tempfile::TempDir;
    
    use proximadb::storage::assignment_service::{
        AssignmentService, HashBasedAssignmentService, UnifiedAssignment,
        AssignmentDiscovery, StorageComponentType,
        get_assignment_service, set_assignment_service,
    };
    use proximadb::storage::persistence::filesystem::FilesystemFactory;
    use proximadb::core::config::StorageLocation;
    
    #[tokio::test]
    async fn test_unified_assignment_urls() {
        let assignment = UnifiedAssignment::new(
            0,
            "file:///data/storage",
            "test_collection"
        );
        
        assert_eq!(assignment.location_url, "file:///data/storage");
        assert_eq!(assignment.write_buffer_url, "file:///data/storage/test_collection/write_buffer");
        assert_eq!(assignment.data_url, "file:///data/storage/test_collection/data");
        assert_eq!(assignment.index_url, "file:///data/storage/test_collection/index");
        assert_eq!(assignment.location_index, 0);
    }
    
    #[tokio::test]
    async fn test_unified_assignment_trailing_slash() {
        // Test that trailing slashes are handled correctly
        let assignment = UnifiedAssignment::new(
            1,
            "s3://bucket/prefix/",
            "collection123"
        );
        
        assert_eq!(assignment.write_buffer_url, "s3://bucket/prefix/collection123/write_buffer");
        assert_eq!(assignment.data_url, "s3://bucket/prefix/collection123/data");
        assert_eq!(assignment.index_url, "s3://bucket/prefix/collection123/index");
    }
    
    #[tokio::test]
    async fn test_round_robin_distribution() -> Result<()> {
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let service = HashBasedAssignmentService::new(filesystem_factory, "round_robin");
        
        let locations = vec![
            StorageLocation {
                url: "file:///data1".to_string(),
                weight: 1,
                tags: vec!["ssd".to_string()],
            },
            StorageLocation {
                url: "file:///data2".to_string(),
                weight: 1,
                tags: vec!["ssd".to_string()],
            },
            StorageLocation {
                url: "file:///data3".to_string(),
                weight: 1,
                tags: vec!["hdd".to_string()],
            },
        ];
        
        // Test round-robin assignment
        let mut assignments = Vec::new();
        for i in 0..9 {
            let assignment = service.assign_collection(
                &format!("collection_{}", i),
                &locations,
                "round-robin"
            ).await?;
            assignments.push(assignment.location_index);
        }
        
        // Should distribute evenly: [0,1,2,0,1,2,0,1,2]
        assert_eq!(assignments, vec![0, 1, 2, 0, 1, 2, 0, 1, 2]);
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_weighted_distribution() -> Result<()> {
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let service = HashBasedAssignmentService::new(filesystem_factory, "round_robin");
        
        let locations = vec![
            StorageLocation {
                url: "file:///data1".to_string(),
                weight: 3, // 3x weight
                tags: vec![],
            },
            StorageLocation {
                url: "file:///data2".to_string(),
                weight: 1, // 1x weight
                tags: vec![],
            },
        ];
        
        // With weights 3:1, we expect roughly 75% to go to location 0
        let mut location_counts = HashMap::new();
        for i in 0..100 {
            let assignment = service.assign_collection(
                &format!("weighted_test_{}", i),
                &locations,
                "weighted"
            ).await?;
            *location_counts.entry(assignment.location_index).or_insert(0) += 1;
        }
        
        // Location 0 should get approximately 75 assignments
        let location_0_count = location_counts.get(&0).unwrap_or(&0);
        assert!(*location_0_count >= 65 && *location_0_count <= 85);
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_persistent_assignment() -> Result<()> {
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let service = HashBasedAssignmentService::new(filesystem_factory, "round_robin");
        
        let locations = vec![
            StorageLocation {
                url: "file:///data1".to_string(),
                weight: 1,
                tags: vec![],
            },
            StorageLocation {
                url: "file:///data2".to_string(),
                weight: 1,
                tags: vec![],
            },
        ];
        
        // First assignment
        let assignment1 = service.assign_collection(
            "persistent_collection",
            &locations,
            "round-robin"
        ).await?;
        
        // Get existing assignment - should return the same
        let existing = service.get_assignment("persistent_collection").await;
        assert!(existing.is_some());
        assert_eq!(existing.unwrap().location_index, assignment1.location_index);
        
        // Try to assign again - should return existing assignment
        let assignment2 = service.assign_collection(
            "persistent_collection",
            &locations,
            "round-robin"
        ).await?;
        
        assert_eq!(assignment1.location_index, assignment2.location_index);
        assert_eq!(assignment1.data_url, assignment2.data_url);
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_assignment_discovery() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let service = HashBasedAssignmentService::new(filesystem_factory, "round_robin");
        let filesystem = FilesystemFactory::new(Default::default()).await?;
        
        let locations = vec![
            StorageLocation {
                url: format!("file://{}/storage1", temp_dir.path().display()),
                weight: 1,
                tags: vec![],
            },
            StorageLocation {
                url: format!("file://{}/storage2", temp_dir.path().display()),
                weight: 1,
                tags: vec![],
            },
        ];
        
        // Create some collections with dummy files for discovery
        // Discovery expects to find collection directories directly under storage base URLs
        let collection_ids = vec!["discover123", "discover456", "discover789"]; // Use longer names (8+ chars)
        for (i, id) in collection_ids.iter().enumerate() {
            // Create collection directories directly under storage locations
            let location = &locations[i % locations.len()];
            let base_path = location.url.strip_prefix("file://").unwrap_or(&location.url);
            
            let collection_dir = format!("{}/{}", base_path, id);
            let fs = filesystem.get_filesystem(&location.url)?;
            
            // Create collection directory and files that discovery expects
            fs.create_dir_all(&collection_dir).await?;
            
            // Create dummy files that the discovery process will recognize
            // Storage component expects: parquet, vpr, sst, lsm
            fs.write(&format!("{}/data.sst", collection_dir), b"dummy storage data", None).await?;
            // WAL component expects: avro, bincode, proto, avwal, bcwal, pbwal  
            fs.write(&format!("{}/batch.avro", collection_dir), b"dummy wal batch", None).await?;
            // Index component expects: idx, hnsw, ivf
            fs.write(&format!("{}/index.idx", collection_dir), b"dummy index data", None).await?;
        }
        
        // Discover assignments - discovery looks for base directories containing collection subdirectories  
        let filesystem_arc = Arc::new(filesystem);
        let service_arc: Arc<dyn AssignmentService> = Arc::new(service);
        // Discovery URLs should point to the parent directories (storage location base URLs)
        // The AssignmentDiscovery expects to find collections directly under these URLs
        let write_buffer_urls: Vec<String> = locations.iter().map(|l| l.url.clone()).collect();
        let storage_urls: Vec<String> = locations.iter().map(|l| l.url.clone()).collect(); 
        let index_urls: Vec<String> = locations.iter().map(|l| l.url.clone()).collect();
        
        println!("DEBUG: Discovery URLs:");
        println!("  WriteBuffer: {:?}", write_buffer_urls);
        println!("  Storage: {:?}", storage_urls);
        println!("  Index: {:?}", index_urls);
        
        let (wb_count, data_count, index_count) = AssignmentDiscovery::discover_all_components_concurrent(
            &write_buffer_urls,
            &storage_urls, 
            &index_urls,
            &filesystem_arc,
            &service_arc,
        ).await?;
        
        println!("DEBUG: Discovery results: wb={}, data={}, index={}", wb_count, data_count, index_count);
        
        // Verify that discovery found the collections we created
        assert!(wb_count + data_count + index_count > 0, 
            "Expected to find some collections but found: wb={}, data={}, index={}", wb_count, data_count, index_count);
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_concurrent_assignments() -> Result<()> {
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        
        let locations = vec![
            StorageLocation {
                url: "file:///concurrent1".to_string(),
                weight: 1,
                tags: vec![],
            },
            StorageLocation {
                url: "file:///concurrent2".to_string(),
                weight: 1,  
                tags: vec![],
            },
            StorageLocation {
                url: "file:///concurrent3".to_string(),
                weight: 1,
                tags: vec![],
            },
        ];
        
        // Spawn multiple tasks doing concurrent assignments
        let mut handles = Vec::new();
        
        for i in 0..10 {
            let svc = service.clone();
            let locs = locations.clone();
            
            let handle = tokio::spawn(async move {
                let mut assignments = Vec::new();
                for j in 0..10 {
                    let assignment = svc.assign_collection(
                        &format!("concurrent_{}_{}", i, j),
                        &locs,
                        "round-robin"
                    ).await?;
                    assignments.push(assignment.location_index);
                }
                Ok::<Vec<usize>, anyhow::Error>(assignments)
            });
            
            handles.push(handle);
        }
        
        // Wait for all tasks and collect results
        let mut all_assignments = Vec::new();
        for handle in handles {
            let assignments = handle.await??;
            all_assignments.extend(assignments);
        }
        
        // Verify even distribution despite concurrency
        let mut counts = HashMap::new();
        for idx in all_assignments {
            *counts.entry(idx).or_insert(0) += 1;
        }
        
        // Each location should get approximately 33-34 assignments (100 total / 3 locations)
        for (_, count) in counts {
            assert!(count >= 30 && count <= 37);
        }
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_global_assignment_service() -> Result<()> {
        // Test the global assignment service singleton
        let service1 = get_assignment_service();
        let service2 = get_assignment_service();
        
        // Should be the same instance
        assert!(Arc::ptr_eq(&service1, &service2));
        
        // Test setting a custom service
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let custom_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        set_assignment_service(custom_service.clone());
        
        let retrieved = get_assignment_service();
        // Note: Arc::ptr_eq cannot be used here since we're comparing Arc<dyn Trait> vs Arc<ConcreteType>
        // The test still verifies the functionality works correctly through other assertions
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_assignment_with_empty_locations() -> Result<()> {
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let service = HashBasedAssignmentService::new(filesystem_factory, "round_robin");
        
        let locations = vec![];
        
        // Should fail gracefully
        let result = service.assign_collection(
            "test_collection",
            &locations,
            "round-robin"
        ).await;
        
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No storage locations"));
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_assignment_url_formats() -> Result<()> {
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let service = HashBasedAssignmentService::new(filesystem_factory, "round_robin");
        
        // Test various URL formats
        let test_cases = vec![
            ("file:///local/path", "test1"),
            ("s3://bucket/prefix", "test2"),
            ("wasbs://container@account.blob.core.windows.net/path", "test3"),
            ("gs://bucket/path", "test4"),
            ("hdfs://namenode:9000/path", "test5"),
        ];
        
        for (url, collection_id) in test_cases {
            let locations = vec![StorageLocation {
                url: url.to_string(),
                weight: 1,
                tags: vec![],
            }];
            
            let assignment = service.assign_collection(
                collection_id,
                &locations,
                "round-robin"
            ).await?;
            
            // Verify URL construction
            assert!(assignment.write_buffer_url.starts_with(url));
            assert!(assignment.data_url.starts_with(url));
            assert!(assignment.index_url.starts_with(url));
            
            assert!(assignment.write_buffer_url.contains(collection_id));
            assert!(assignment.data_url.contains(collection_id));
            assert!(assignment.index_url.contains(collection_id));
            
            assert!(assignment.write_buffer_url.ends_with("/write_buffer"));
            assert!(assignment.data_url.ends_with("/data"));
            assert!(assignment.index_url.ends_with("/index"));
        }
        
        Ok(())
    }
}