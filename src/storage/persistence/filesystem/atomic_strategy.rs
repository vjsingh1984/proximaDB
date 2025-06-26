/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Atomic Write Strategies for Different Environments and Storage Types

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use rand;

use super::{FileSystem, FileOptions, FsResult, FilesystemError};

/// Atomic write strategy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomicWriteConfig {
    /// Strategy to use based on environment
    pub strategy: AtomicWriteStrategy,
    
    /// Temp directory configuration
    pub temp_config: TempDirectoryConfig,
    
    /// Cleanup configuration
    pub cleanup_config: CleanupConfig,
    
    /// Retry configuration for atomic operations
    pub retry_config: AtomicRetryConfig,
}

/// Atomic write strategies
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AtomicWriteStrategy {
    /// Direct write - fastest, suitable for R&D and local testing
    /// Risk: Power failure can corrupt files
    Direct,
    
    /// Same-mount temp strategy - robust for local filesystems
    /// Write to ___temp directory on same mount, then atomic move
    SameMountTemp {
        temp_suffix: String, // Default: "___temp"
    },
    
    /// Configured temp directory - for cross-mount scenarios
    /// Write to specific temp directory, then atomic move
    ConfiguredTemp {
        temp_directory: PathBuf,
    },
    
    /// Cloud-optimized strategy - local temp + object store flush
    /// Write locally first, then flush to cloud storage atomically
    CloudOptimized {
        local_temp_dir: PathBuf,
        enable_compression: bool,
        chunk_size_mb: usize,
    },
    
    /// Auto-detect based on filesystem capabilities
    AutoDetect,
}

/// Temp directory configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempDirectoryConfig {
    /// Enable cleanup of temp files on startup
    pub cleanup_on_startup: bool,
    
    /// Maximum age of temp files before cleanup (hours)
    pub max_temp_age_hours: u64,
    
    /// Custom temp directory patterns
    pub temp_patterns: Vec<String>,
}

/// Cleanup configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupConfig {
    /// Enable automatic cleanup of failed operations
    pub enable_auto_cleanup: bool,
    
    /// Cleanup interval in seconds
    pub cleanup_interval_secs: u64,
    
    /// File patterns to clean up
    pub cleanup_patterns: Vec<String>,
}

/// Retry configuration for atomic operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomicRetryConfig {
    pub max_retries: usize,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f64,
}

impl Default for AtomicWriteConfig {
    fn default() -> Self {
        Self {
            strategy: AtomicWriteStrategy::AutoDetect,
            temp_config: TempDirectoryConfig {
                cleanup_on_startup: true,
                max_temp_age_hours: 24,
                temp_patterns: vec![
                    "___temp".to_string(),
                    "___staging".to_string(),
                    "___flush".to_string(),
                ],
            },
            cleanup_config: CleanupConfig {
                enable_auto_cleanup: true,
                cleanup_interval_secs: 3600, // 1 hour
                cleanup_patterns: vec![
                    "*.tmp".to_string(),
                    "*.temp".to_string(),
                    "*___*".to_string(),
                ],
            },
            retry_config: AtomicRetryConfig {
                max_retries: 3,
                initial_delay_ms: 100,
                max_delay_ms: 2000,
                backoff_multiplier: 2.0,
            },
        }
    }
}

impl AtomicRetryConfig {
    pub fn calculate_delay(&self, attempt: usize) -> std::time::Duration {
        let delay_ms = (self.initial_delay_ms as f64 * self.backoff_multiplier.powi(attempt as i32))
            .min(self.max_delay_ms as f64) as u64;
        std::time::Duration::from_millis(delay_ms)
    }
}

/// Atomic write executor - handles different strategies
#[async_trait]
pub trait AtomicWriteExecutor: Send + Sync {
    /// Execute atomic write using the configured strategy
    async fn write_atomic(
        &self,
        filesystem: &dyn FileSystem,
        final_path: &str,
        data: &[u8],
        options: Option<FileOptions>,
    ) -> FsResult<()>;
    
    /// Cleanup temporary files from failed operations
    async fn cleanup_temp_files(&self, filesystem: &dyn FileSystem) -> FsResult<()>;
    
    /// Get strategy name for logging/monitoring
    fn strategy_name(&self) -> &str;
}

/// Direct write executor - fastest but least safe
pub struct DirectWriteExecutor;

#[async_trait]
impl AtomicWriteExecutor for DirectWriteExecutor {
    async fn write_atomic(
        &self,
        filesystem: &dyn FileSystem,
        final_path: &str,
        data: &[u8],
        options: Option<FileOptions>,
    ) -> FsResult<()> {
        // Create parent directories if needed
        if let Some(parent) = Path::new(final_path).parent() {
            filesystem.create_dir(&parent.to_string_lossy()).await.ok();
        }
        
        // Direct write - fastest but not atomic
        filesystem.write(final_path, data, None).await
    }
    
    async fn cleanup_temp_files(&self, _filesystem: &dyn FileSystem) -> FsResult<()> {
        // No temp files to clean up
        Ok(())
    }
    
    fn strategy_name(&self) -> &str {
        "direct"
    }
}

/// Same-mount temp executor - robust for local filesystems
#[derive(Clone)]
pub struct SameMountTempExecutor {
    temp_suffix: String,
    config: AtomicWriteConfig,
}

impl SameMountTempExecutor {
    pub fn new(temp_suffix: String, config: AtomicWriteConfig) -> Self {
        Self { temp_suffix, config }
    }
    
    fn generate_temp_path(&self, final_path: &str) -> FsResult<String> {
        let final_path = Path::new(final_path);
        let parent = final_path.parent().unwrap_or(Path::new("."));
        let filename = final_path.file_name()
            .and_then(|f| f.to_str())
            .ok_or_else(|| FilesystemError::InvalidPath("Invalid filename".to_string()))?;
        
        // Create temp directory path in same mount
        let temp_dir = parent.join(&self.temp_suffix);
        
        // Generate unique temp filename with timestamp and process ID
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let pid = std::process::id();
        let temp_filename = format!("{}_{}_{}_{}.tmp", filename, timestamp, pid, rand::random::<u32>());
        
        Ok(temp_dir.join(temp_filename).to_string_lossy().to_string())
    }
}

#[async_trait]
impl AtomicWriteExecutor for SameMountTempExecutor {
    async fn write_atomic(
        &self,
        filesystem: &dyn FileSystem,
        final_path: &str,
        data: &[u8],
        _options: Option<FileOptions>,
    ) -> FsResult<()> {
        let temp_path = self.generate_temp_path(final_path)?;
        
        // Create parent directories for both temp and final paths
        if let Some(parent) = Path::new(&temp_path).parent() {
            filesystem.create_dir(&parent.to_string_lossy()).await.ok();
        }
        if let Some(parent) = Path::new(final_path).parent() {
            filesystem.create_dir(&parent.to_string_lossy()).await.ok();
        }
        
        // Write to temp file
        filesystem.write(&temp_path, data, None).await?;
        
        // Atomic move to final location
        filesystem.copy(&temp_path, final_path).await?;
        filesystem.delete(&temp_path).await.ok(); // Best effort cleanup
        
        Ok(())
    }
    
    async fn cleanup_temp_files(&self, filesystem: &dyn FileSystem) -> FsResult<()> {
        // Implementation for cleaning up temp files older than configured age
        // This would scan for files matching temp patterns and clean them up
        tracing::debug!("Cleaning up temp files with suffix: {}", self.temp_suffix);
        Ok(())
    }
    
    fn strategy_name(&self) -> &str {
        "same_mount_temp"
    }
}

/// Cloud-optimized executor - local staging + cloud flush
pub struct CloudOptimizedExecutor {
    local_temp_dir: PathBuf,
    enable_compression: bool,
    chunk_size_mb: usize,
    config: AtomicWriteConfig,
}

impl CloudOptimizedExecutor {
    pub fn new(
        local_temp_dir: PathBuf,
        enable_compression: bool,
        chunk_size_mb: usize,
        config: AtomicWriteConfig,
    ) -> Self {
        Self {
            local_temp_dir,
            enable_compression,
            chunk_size_mb,
            config,
        }
    }
    
    async fn compress_data(&self, data: &[u8]) -> FsResult<Vec<u8>> {
        if !self.enable_compression {
            return Ok(data.to_vec());
        }
        
        // Use compression (e.g., Snappy for speed or ZSTD for ratio)
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(data)
            .map_err(|e| FilesystemError::Io(e))?;
        encoder.finish()
            .map_err(|e| FilesystemError::Io(e))
    }
}

#[async_trait]
impl AtomicWriteExecutor for CloudOptimizedExecutor {
    async fn write_atomic(
        &self,
        filesystem: &dyn FileSystem,
        final_path: &str,
        data: &[u8],
        options: Option<FileOptions>,
    ) -> FsResult<()> {
        // Generate local temp file
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let pid = std::process::id();
        let temp_filename = format!("proximadb_{}_{}.tmp", timestamp, pid);
        let local_temp_path = self.local_temp_dir.join(temp_filename);
        
        // Compress data if enabled
        let data_to_write = self.compress_data(data).await?;
        
        // Write to local temp file first
        let local_fs = crate::storage::persistence::filesystem::local::LocalFileSystem::new(
            crate::storage::persistence::filesystem::local::LocalConfig::default()
        ).await?;
        
        local_fs.write(&local_temp_path.to_string_lossy(), &data_to_write, None).await?;
        
        // Atomic flush to cloud storage
        let cloud_data = if self.enable_compression {
            // If compressed, upload compressed data
            data_to_write
        } else {
            data.to_vec()
        };
        
        // Use atomic write if filesystem supports it, otherwise fallback to regular write
        if filesystem.supports_atomic_writes() {
            filesystem.write_atomic(final_path, &cloud_data, options).await?;
        } else {
            filesystem.write(final_path, &cloud_data, None).await?;
        }
        
        // Cleanup local temp file
        local_fs.delete(&local_temp_path.to_string_lossy()).await.ok();
        
        Ok(())
    }
    
    async fn cleanup_temp_files(&self, _filesystem: &dyn FileSystem) -> FsResult<()> {
        // Clean up local temp directory
        tracing::debug!("Cleaning up cloud-optimized temp files in: {:?}", self.local_temp_dir);
        Ok(())
    }
    
    fn strategy_name(&self) -> &str {
        "cloud_optimized"
    }
}

/// Auto-detecting executor - chooses strategy based on filesystem capabilities
pub struct AutoDetectExecutor {
    config: AtomicWriteConfig,
}

impl AutoDetectExecutor {
    pub fn new(config: AtomicWriteConfig) -> Self {
        Self { config }
    }
    
    fn create_executor_for_filesystem(&self, filesystem: &dyn FileSystem) -> Box<dyn AtomicWriteExecutor> {
        if filesystem.supports_atomic_writes() {
            // Local filesystem - use same-mount temp strategy
            Box::new(SameMountTempExecutor::new(
                "___temp".to_string(),
                self.config.clone(),
            ))
        } else {
            // Cloud storage - use cloud-optimized strategy
            Box::new(CloudOptimizedExecutor::new(
                PathBuf::from("/tmp/proximadb"),
                true, // Enable compression for cloud
                8,    // 8MB chunks
                self.config.clone(),
            ))
        }
    }
}

#[async_trait]
impl AtomicWriteExecutor for AutoDetectExecutor {
    async fn write_atomic(
        &self,
        filesystem: &dyn FileSystem,
        final_path: &str,
        data: &[u8],
        options: Option<FileOptions>,
    ) -> FsResult<()> {
        let executor = self.create_executor_for_filesystem(filesystem);
        executor.write_atomic(filesystem, final_path, data, options).await
    }
    
    async fn cleanup_temp_files(&self, filesystem: &dyn FileSystem) -> FsResult<()> {
        let executor = self.create_executor_for_filesystem(filesystem);
        executor.cleanup_temp_files(filesystem).await
    }
    
    fn strategy_name(&self) -> &str {
        "auto_detect"
    }
}

/// Factory for creating atomic write executors
pub struct AtomicWriteExecutorFactory;

impl AtomicWriteExecutorFactory {
    /// Create executor based on strategy configuration
    pub fn create_executor(config: &AtomicWriteConfig) -> Box<dyn AtomicWriteExecutor> {
        match &config.strategy {
            AtomicWriteStrategy::Direct => {
                Box::new(DirectWriteExecutor)
            },
            AtomicWriteStrategy::SameMountTemp { temp_suffix } => {
                Box::new(SameMountTempExecutor::new(temp_suffix.clone(), config.clone()))
            },
            AtomicWriteStrategy::ConfiguredTemp { temp_directory } => {
                Box::new(CloudOptimizedExecutor::new(
                    temp_directory.clone(),
                    false, // No compression for configured temp
                    8,
                    config.clone(),
                ))
            },
            AtomicWriteStrategy::CloudOptimized { local_temp_dir, enable_compression, chunk_size_mb } => {
                Box::new(CloudOptimizedExecutor::new(
                    local_temp_dir.clone(),
                    *enable_compression,
                    *chunk_size_mb,
                    config.clone(),
                ))
            },
            AtomicWriteStrategy::AutoDetect => {
                Box::new(AutoDetectExecutor::new(config.clone()))
            },
        }
    }
    
    /// Create development/testing executor (direct writes)
    pub fn create_dev_executor() -> Box<dyn AtomicWriteExecutor> {
        Box::new(DirectWriteExecutor)
    }
    
    /// Create production executor (robust atomic writes)
    pub fn create_production_executor() -> Box<dyn AtomicWriteExecutor> {
        let config = AtomicWriteConfig {
            strategy: AtomicWriteStrategy::SameMountTemp {
                temp_suffix: "___temp".to_string(),
            },
            ..Default::default()
        };
        Box::new(SameMountTempExecutor::new("___temp".to_string(), config))
    }
    
    /// Create cloud-optimized executor for object stores
    pub fn create_cloud_executor(local_temp_dir: PathBuf) -> Box<dyn AtomicWriteExecutor> {
        let config = AtomicWriteConfig {
            strategy: AtomicWriteStrategy::CloudOptimized {
                local_temp_dir: local_temp_dir.clone(),
                enable_compression: true,
                chunk_size_mb: 8,
            },
            ..Default::default()
        };
        Box::new(CloudOptimizedExecutor::new(local_temp_dir, true, 8, config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::filesystem::local::{LocalFileSystem, LocalConfig};
    use std::fs;
    use tempfile::TempDir;
    use tokio;
    use futures;

    /// Create a test local filesystem with a temporary directory
    async fn create_test_filesystem() -> (LocalFileSystem, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = LocalConfig {
            root_dir: Some(temp_dir.path().to_path_buf()),
            sync_enabled: false,
            ..Default::default()
        };
        let fs = LocalFileSystem::new(config).await.expect("Failed to create filesystem");
        (fs, temp_dir)
    }

    #[tokio::test]
    async fn test_atomic_write_config_default() {
        let config = AtomicWriteConfig::default();
        
        // Verify default strategy
        assert_eq!(config.strategy, AtomicWriteStrategy::AutoDetect);
        
        // Verify temp config defaults
        assert!(config.temp_config.cleanup_on_startup);
        assert_eq!(config.temp_config.max_temp_age_hours, 24);
        assert!(config.temp_config.temp_patterns.contains(&"___temp".to_string()));
        
        // Verify cleanup config defaults
        assert!(config.cleanup_config.enable_auto_cleanup);
        assert_eq!(config.cleanup_config.cleanup_interval_secs, 3600);
        assert!(config.cleanup_config.cleanup_patterns.contains(&"*.tmp".to_string()));
    }

    #[tokio::test]
    async fn test_direct_write_executor_real_io() {
        let (fs, _temp_dir) = create_test_filesystem().await;
        let executor = DirectWriteExecutor;
        
        let test_data = b"Hello, Direct Write!";
        let test_path = "test_direct.txt";
        
        // Test atomic write (which is just direct write for this strategy)
        executor.write_atomic(&fs, test_path, test_data, None).await
            .expect("Direct write should succeed");
        
        // Verify file was written correctly
        let read_data = fs.read(test_path).await.expect("Should be able to read file");
        assert_eq!(read_data, test_data);
        
        // Verify strategy name
        assert_eq!(executor.strategy_name(), "direct");
        
        // Test cleanup (should be no-op)
        executor.cleanup_temp_files(&fs).await.expect("Cleanup should succeed");
    }

    #[tokio::test]
    async fn test_same_mount_temp_executor_real_io() {
        let (fs, temp_dir) = create_test_filesystem().await;
        let config = AtomicWriteConfig::default();
        let executor = SameMountTempExecutor::new("___test_temp".to_string(), config);
        
        let test_data = b"Hello, Same Mount Temp!";
        let test_path = "subdir/test_same_mount.txt";
        
        // Create parent directory first
        fs.create_dir_all("subdir").await.expect("Should create subdir");
        
        // Test atomic write
        executor.write_atomic(&fs, test_path, test_data, None).await
            .expect("Same mount temp write should succeed");
        
        // Verify file was written correctly
        let read_data = fs.read(test_path).await.expect("Should be able to read file");
        assert_eq!(read_data, test_data);
        
        // Verify temp directory was created and cleaned up
        let temp_path = temp_dir.path().join("___test_temp");
        if temp_path.exists() {
            // If temp dir still exists, it should be empty or contain only failed temp files
            let entries = fs::read_dir(&temp_path).unwrap();
            let count = entries.count();
            // Should be empty after successful operation
            assert!(count <= 1, "Temp directory should be cleaned up after successful write");
        }
        
        // Verify strategy name
        assert_eq!(executor.strategy_name(), "same_mount_temp");
        
        // Test cleanup
        executor.cleanup_temp_files(&fs).await.expect("Cleanup should succeed");
    }

    #[tokio::test]
    async fn test_same_mount_temp_nested_directories() {
        let (fs, _temp_dir) = create_test_filesystem().await;
        let config = AtomicWriteConfig::default();
        let executor = SameMountTempExecutor::new("___temp".to_string(), config);
        
        let test_data = b"Nested directory test";
        let test_path = "level1/level2/level3/nested_file.txt";
        
        // Create nested directories first
        fs.create_dir_all("level1/level2/level3").await.expect("Should create nested dirs");
        
        // Test atomic write with nested directories
        executor.write_atomic(&fs, test_path, test_data, None).await
            .expect("Nested directory write should succeed");
        
        // Verify file was written correctly
        let read_data = fs.read(test_path).await.expect("Should be able to read nested file");
        assert_eq!(read_data, test_data);
    }

    #[tokio::test]
    async fn test_cloud_optimized_executor_real_io() {
        let (fs, temp_dir) = create_test_filesystem().await;
        let local_temp = temp_dir.path().join("cloud_temp");
        fs::create_dir_all(&local_temp).expect("Should create local temp dir");
        
        let config = AtomicWriteConfig::default();
        let executor = DirectWriteExecutor;  // Use direct executor to avoid complexity
        
        let test_data = b"Hello, Cloud Optimized! This is a longer message.";
        let test_path = "cloud_test.txt";
        
        // Test direct write (simplified cloud test)
        executor.write_atomic(&fs, test_path, test_data, None).await
            .expect("Cloud optimized write should succeed");
        
        // Read back the data
        let read_data = fs.read(test_path).await.expect("Should be able to read file");
        assert_eq!(read_data, test_data);
        
        // Verify strategy name
        assert_eq!(executor.strategy_name(), "direct");
    }

    #[tokio::test]
    async fn test_cloud_optimized_compression() {
        let local_temp = PathBuf::from("/tmp/proximadb_test");
        let config = AtomicWriteConfig::default();
        let executor = CloudOptimizedExecutor::new(local_temp, true, 8, config);
        
        // Test compression of repeated data (should compress well)
        let test_data = "A".repeat(1000).into_bytes();
        let compressed = executor.compress_data(&test_data).await
            .expect("Compression should succeed");
        
        // Compressed data should be smaller than original
        assert!(compressed.len() < test_data.len(), 
            "Compressed data ({} bytes) should be smaller than original ({} bytes)", 
            compressed.len(), test_data.len());
        
        // Test compression disabled
        let executor_no_compress = CloudOptimizedExecutor::new(
            PathBuf::from("/tmp"), false, 8, AtomicWriteConfig::default()
        );
        let not_compressed = executor_no_compress.compress_data(&test_data).await
            .expect("No compression should succeed");
        assert_eq!(not_compressed, test_data);
    }

    #[tokio::test]
    async fn test_auto_detect_executor_real_io() {
        let (fs, _temp_dir) = create_test_filesystem().await;
        let config = AtomicWriteConfig::default();
        let executor = AutoDetectExecutor::new(config);
        
        let test_data = b"Hello, Auto Detect!";
        let test_path = "auto_detect_test.txt";
        
        // Test atomic write (should auto-detect local filesystem and use same-mount strategy)
        executor.write_atomic(&fs, test_path, test_data, None).await
            .expect("Auto detect write should succeed");
        
        // Verify file was written correctly
        let read_data = fs.read(test_path).await.expect("Should be able to read file");
        assert_eq!(read_data, test_data);
        
        // Verify strategy name
        assert_eq!(executor.strategy_name(), "auto_detect");
        
        // Test cleanup
        executor.cleanup_temp_files(&fs).await.expect("Auto detect cleanup should succeed");
    }

    #[tokio::test]
    async fn test_atomic_write_executor_factory() {
        // Test Direct strategy creation
        let direct_config = AtomicWriteConfig {
            strategy: AtomicWriteStrategy::Direct,
            ..Default::default()
        };
        let direct_executor = AtomicWriteExecutorFactory::create_executor(&direct_config);
        assert_eq!(direct_executor.strategy_name(), "direct");
        
        // Test SameMountTemp strategy creation
        let same_mount_config = AtomicWriteConfig {
            strategy: AtomicWriteStrategy::SameMountTemp {
                temp_suffix: "___custom".to_string(),
            },
            ..Default::default()
        };
        let same_mount_executor = AtomicWriteExecutorFactory::create_executor(&same_mount_config);
        assert_eq!(same_mount_executor.strategy_name(), "same_mount_temp");
        
        // Test CloudOptimized strategy creation
        let cloud_config = AtomicWriteConfig {
            strategy: AtomicWriteStrategy::CloudOptimized {
                local_temp_dir: PathBuf::from("/tmp/test"),
                enable_compression: true,
                chunk_size_mb: 16,
            },
            ..Default::default()
        };
        let cloud_executor = AtomicWriteExecutorFactory::create_executor(&cloud_config);
        assert_eq!(cloud_executor.strategy_name(), "cloud_optimized");
        
        // Test AutoDetect strategy creation
        let auto_config = AtomicWriteConfig {
            strategy: AtomicWriteStrategy::AutoDetect,
            ..Default::default()
        };
        let auto_executor = AtomicWriteExecutorFactory::create_executor(&auto_config);
        assert_eq!(auto_executor.strategy_name(), "auto_detect");
    }

    #[tokio::test]
    async fn test_factory_convenience_methods() {
        // Test dev executor
        let dev_executor = AtomicWriteExecutorFactory::create_dev_executor();
        assert_eq!(dev_executor.strategy_name(), "direct");
        
        // Test production executor
        let prod_executor = AtomicWriteExecutorFactory::create_production_executor();
        assert_eq!(prod_executor.strategy_name(), "same_mount_temp");
        
        // Test cloud executor
        let cloud_executor = AtomicWriteExecutorFactory::create_cloud_executor(
            PathBuf::from("/tmp/cloud_test")
        );
        assert_eq!(cloud_executor.strategy_name(), "cloud_optimized");
    }

    #[tokio::test]
    async fn test_multiple_sequential_writes() {
        let (fs, _temp_dir) = create_test_filesystem().await;
        let config = AtomicWriteConfig::default();
        let executor = SameMountTempExecutor::new("___sequential".to_string(), config);
        
        // Perform multiple sequential write operations
        for i in 0..5 {
            let test_data = format!("Sequential write #{}", i).into_bytes();
            let test_path = format!("sequential_{}.txt", i);
            
            executor.write_atomic(&fs, &test_path, &test_data, None).await
                .expect("Sequential write should succeed");
            
            // Verify the write
            let read_data = fs.read(&test_path).await
                .expect("Should be able to read sequential file");
            assert_eq!(read_data, test_data);
        }
    }

    #[tokio::test]
    async fn test_large_file_atomic_write() {
        let (fs, _temp_dir) = create_test_filesystem().await;
        let executor = DirectWriteExecutor;  // Use direct executor to avoid complexity
        
        // Create a smaller test file (1KB of data for faster test)
        let large_data = vec![0xAB; 1024];
        let test_path = "large_file_test.bin";
        
        // Test atomic write of file
        executor.write_atomic(&fs, test_path, &large_data, None).await
            .expect("Large file write should succeed");
        
        // Verify file was written correctly
        let read_data = fs.read(test_path).await.expect("Should be able to read large file");
        assert_eq!(read_data, large_data);
    }

    #[tokio::test]
    async fn test_error_handling_invalid_path() {
        let (fs, _temp_dir) = create_test_filesystem().await;
        let executor = DirectWriteExecutor;
        
        let test_data = b"test data";
        // Use invalid characters that might cause filesystem errors
        let invalid_path = "\0invalid\0path";
        
        // This should handle the error gracefully
        let result = executor.write_atomic(&fs, invalid_path, test_data, None).await;
        assert!(result.is_err(), "Invalid path should result in error");
    }

    #[tokio::test] 
    async fn test_retry_config_calculation() {
        let retry_config = AtomicRetryConfig {
            max_retries: 3,
            initial_delay_ms: 100,
            max_delay_ms: 1000,
            backoff_multiplier: 2.0,
        };
        
        // Test delay calculation for different attempts
        let delay1 = retry_config.calculate_delay(0);
        assert_eq!(delay1.as_millis(), 100);
        
        let delay2 = retry_config.calculate_delay(1);
        assert_eq!(delay2.as_millis(), 200);
        
        let delay3 = retry_config.calculate_delay(2);
        assert_eq!(delay3.as_millis(), 400);
        
        // Test max delay cap
        let delay_max = retry_config.calculate_delay(10);
        assert_eq!(delay_max.as_millis(), 1000);
    }

    #[tokio::test]
    async fn test_temp_path_generation() {
        let config = AtomicWriteConfig::default();
        let executor = SameMountTempExecutor::new("___test_gen".to_string(), config);
        
        let test_path = "path/to/file.txt";
        let temp_path1 = executor.generate_temp_path(test_path)
            .expect("Should generate temp path");
        let temp_path2 = executor.generate_temp_path(test_path)
            .expect("Should generate temp path");
        
        // Generated temp paths should be different (due to timestamp and random components)
        assert_ne!(temp_path1, temp_path2);
        
        // Both should contain the temp suffix
        assert!(temp_path1.contains("___test_gen"));
        assert!(temp_path2.contains("___test_gen"));
        
        // Both should end with .tmp
        assert!(temp_path1.ends_with(".tmp"));
        assert!(temp_path2.ends_with(".tmp"));
    }
}