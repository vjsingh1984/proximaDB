//! Filesystem Migration Compatibility Test Suite
//!
//! This test suite ensures that the new UnifiedCachingFilesystem maintains
//! backward compatibility with existing IntelligentFilesystem and ZeroCopyFilesystem
//! behaviors during the migration period.

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;

use proximadb::storage::persistence::filesystem::{
    FileSystem, FileOptions, FilesystemFactory,
    intelligent_filesystem::IntelligentFilesystem,
    // TODO: Uncomment when UnifiedCachingFilesystem is implemented
    // unified::UnifiedCachingFilesystem,
};

/// Test data for filesystem operations
fn create_test_data(size: usize) -> Vec<u8> {
    vec![42u8; size]
}

/// Verify that basic filesystem operations work identically
mod basic_operations {
    use super::*;

    #[tokio::test]
    async fn test_write_read_compatibility() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        // Test with IntelligentFilesystem (current)
        let factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let intelligent_fs = Arc::new(IntelligentFilesystem::new(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
        ));

        // Write test data
        let test_data = create_test_data(1024);
        let test_path = "test_file.dat";
        intelligent_fs.write(test_path, &test_data, None).await?;

        // Read and verify
        let read_data = intelligent_fs.read(test_path).await?;
        assert_eq!(test_data, read_data, "Data should match after write/read");

        // TODO: Test with UnifiedCachingFilesystem when implemented
        // let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
        //     base_fs.clone(),
        //     "test_collection".to_string(),
        //     "test_engine".to_string(),
        // ));
        //
        // // Should be able to read data written by IntelligentFilesystem
        // let unified_read = unified_fs.read(test_path).await?;
        // assert_eq!(test_data, unified_read, "Unified should read Intelligent's data");

        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_compatibility() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let intelligent_fs = Arc::new(IntelligentFilesystem::new(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
        ));

        // Create a file
        let test_path = "metadata_test.dat";
        intelligent_fs.write(test_path, &create_test_data(512), None).await?;

        // Check metadata
        let exists = intelligent_fs.exists(test_path).await?;
        assert!(exists, "File should exist");

        let metadata = intelligent_fs.metadata(test_path).await?;
        assert_eq!(metadata.size, 512, "File size should match");

        // TODO: Verify UnifiedCachingFilesystem returns same metadata

        Ok(())
    }
}

/// Test cache behavior compatibility
mod cache_behavior {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn test_cache_hit_performance() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let intelligent_fs = Arc::new(IntelligentFilesystem::new(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
        ));

        // Write test file
        let test_data = create_test_data(1024 * 1024); // 1MB
        let test_path = "cache_test.dat";
        intelligent_fs.write(test_path, &test_data, None).await?;

        // First read (cache miss)
        let start = Instant::now();
        let _ = intelligent_fs.read(test_path).await?;
        let first_read_time = start.elapsed();

        // Second read (cache hit)
        let start = Instant::now();
        let _ = intelligent_fs.read(test_path).await?;
        let second_read_time = start.elapsed();

        // Cache hit should be significantly faster
        assert!(
            second_read_time < first_read_time / 2,
            "Cache hit should be at least 2x faster"
        );

        // TODO: Verify UnifiedCachingFilesystem has similar cache performance

        Ok(())
    }

    #[tokio::test]
    async fn test_cache_invalidation() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let intelligent_fs = Arc::new(IntelligentFilesystem::new(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
        ));

        let test_path = "invalidation_test.dat";

        // Write initial data
        let data_v1 = vec![1u8; 256];
        intelligent_fs.write(test_path, &data_v1, None).await?;

        // Read to populate cache
        let read_v1 = intelligent_fs.read(test_path).await?;
        assert_eq!(data_v1, read_v1);

        // Write new data (should invalidate cache)
        let data_v2 = vec![2u8; 256];
        intelligent_fs.write(test_path, &data_v2, None).await?;

        // Read should get new data
        let read_v2 = intelligent_fs.read(test_path).await?;
        assert_eq!(data_v2, read_v2, "Cache should be invalidated after write");

        // TODO: Verify UnifiedCachingFilesystem handles invalidation correctly

        Ok(())
    }
}

/// Test migration scenarios
mod migration_scenarios {
    use super::*;

    #[tokio::test]
    async fn test_concurrent_access_different_implementations() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;

        // Create two different filesystem wrappers
        let intelligent_fs = Arc::new(IntelligentFilesystem::new(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
        ));

        // TODO: Create UnifiedCachingFilesystem instance
        // let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
        //     base_fs.clone(),
        //     "test_collection".to_string(),
        //     "test_engine".to_string(),
        // ));

        // Test concurrent writes don't corrupt data
        let test_path1 = "concurrent1.dat";
        let test_path2 = "concurrent2.dat";

        let data1 = vec![1u8; 512];
        let data2 = vec![2u8; 512];

        // Write from IntelligentFilesystem
        intelligent_fs.write(test_path1, &data1, None).await?;

        // TODO: Write from UnifiedCachingFilesystem
        // unified_fs.write(test_path2, &data2, None).await?;

        // Both should be able to read each other's files
        let read1 = intelligent_fs.read(test_path1).await?;
        assert_eq!(data1, read1);

        // TODO: Cross-read verification
        // let cross_read1 = unified_fs.read(test_path1).await?;
        // assert_eq!(data1, cross_read1);

        Ok(())
    }

    #[tokio::test]
    async fn test_configuration_compatibility() -> Result<()> {
        // Test that configuration from old systems can be migrated

        // Old IntelligentFilesystem config
        let _old_config = proximadb::storage::persistence::filesystem::intelligent_filesystem::CacheConfig {
            max_memory_mb: 512,
            max_disk_gb: 10,
            metadata_ttl_secs: 300,
            enable_prefetch: true,
            enable_learning: true,
        };

        // TODO: Test migration to UnifiedCacheConfig
        // let unified_config = UnifiedCacheConfig::from_legacy(old_config)?;
        // assert_eq!(unified_config.memory_cache_mb, 512);

        Ok(())
    }
}

/// Performance regression tests
mod performance_regression {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn test_no_performance_regression() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let intelligent_fs = Arc::new(IntelligentFilesystem::new(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
        ));

        // Measure current implementation performance
        let iterations = 100;
        let test_data = create_test_data(4096);

        let start = Instant::now();
        for i in 0..iterations {
            let path = format!("perf_test_{}.dat", i);
            intelligent_fs.write(&path, &test_data, None).await?;
            let _ = intelligent_fs.read(&path).await?;
        }
        let intelligent_time = start.elapsed();

        // TODO: Measure UnifiedCachingFilesystem performance
        // let unified_fs = Arc::new(UnifiedCachingFilesystem::new(...));
        // let start = Instant::now();
        // for i in 0..iterations {
        //     let path = format!("perf_test_unified_{}.dat", i);
        //     unified_fs.write(&path, &test_data, None).await?;
        //     let _ = unified_fs.read(&path).await?;
        // }
        // let unified_time = start.elapsed();

        // // Ensure no significant regression (allow 10% variance)
        // assert!(
        //     unified_time <= intelligent_time * 1.1,
        //     "UnifiedCachingFilesystem should not be slower than IntelligentFilesystem"
        // );

        println!("IntelligentFilesystem: {:?} for {} iterations", intelligent_time, iterations);

        Ok(())
    }
}

/// Edge cases and error handling
mod edge_cases {
    use super::*;

    #[tokio::test]
    async fn test_large_file_handling() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let intelligent_fs = Arc::new(IntelligentFilesystem::new(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
        ));

        // Test with 10MB file
        let large_data = create_test_data(10 * 1024 * 1024);
        let test_path = "large_file.dat";

        intelligent_fs.write(test_path, &large_data, None).await?;
        let read_data = intelligent_fs.read(test_path).await?;

        assert_eq!(large_data.len(), read_data.len(), "Large file should be handled correctly");

        // TODO: Test with UnifiedCachingFilesystem

        Ok(())
    }

    #[tokio::test]
    async fn test_nonexistent_file_handling() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let intelligent_fs = Arc::new(IntelligentFilesystem::new(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
        ));

        // Try to read non-existent file
        let result = intelligent_fs.read("nonexistent.dat").await;
        assert!(result.is_err(), "Should error on non-existent file");

        // Check exists returns false
        let exists = intelligent_fs.exists("nonexistent.dat").await?;
        assert!(!exists, "Non-existent file should return false");

        // TODO: Test with UnifiedCachingFilesystem

        Ok(())
    }
}

#[cfg(test)]
mod test_helpers {
    use super::*;

    /// Helper to create both filesystem implementations for comparison
    pub async fn create_filesystem_pair(
        base_path: &str,
    ) -> Result<(Arc<IntelligentFilesystem>, Arc<dyn FileSystem>)> {
        let factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let base_fs = factory.get_filesystem(base_path)?;

        let intelligent = Arc::new(IntelligentFilesystem::new(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
        ));

        // TODO: Create UnifiedCachingFilesystem when available
        // let unified = Arc::new(UnifiedCachingFilesystem::new(
        //     base_fs.clone(),
        //     "test_collection".to_string(),
        //     "test_engine".to_string(),
        // ));

        Ok((intelligent, base_fs))
    }
}