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
use rand;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;

use super::{FileOptions, FileSystem, FilesystemError, FsResult};

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
    ConfiguredTemp { temp_directory: PathBuf },

    /// Cloud-optimized strategy - local temp + object store flush
    /// Write locally first, then flush to cloud storage atomically
    CloudOptimized {
        local_temp_dir: PathBuf,
        compression: bool,
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
        _options: Option<FileOptions>,
    ) -> FsResult<()> {
        // Create parent directories if needed
        if let Some(parent) = Path::new(final_path).parent() {
            if let Err(e) = filesystem.create_dir_all(&parent.to_string_lossy()).await {
                return Err(FilesystemError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to create parent directory {}: {}", parent.display(), e)
                )));
            }
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
        Self {
            temp_suffix,
            config,
        }
    }

    fn generate_temp_path(&self, final_path: &str) -> FsResult<String> {
        // Use centralized URL path extraction for consistent handling
        use super::FilesystemFactory;
        
        let (base_url, path_part) = if final_path.contains("://") {
            // Extract scheme for URL reconstruction
            use url::Url;
            let parsed_url = Url::parse(final_path)?;
            let scheme = parsed_url.scheme();
            
            // Use centralized path extraction (handles relative paths correctly)
            let path = FilesystemFactory::resolve_path(final_path)?;
            
            (format!("{}://", scheme), path)
        } else {
            // Not a URL, treat as regular path
            ("".to_string(), final_path.to_string())
        };
        
        let final_path = Path::new(&path_part);
        let parent = final_path.parent();
        let filename = final_path
            .file_name()
            .and_then(|f| f.to_str())
            .ok_or_else(|| FilesystemError::InvalidPath("Invalid filename".to_string()))?;

        // Create temp directory path in same mount
        let temp_dir = parent.join(&self.temp_suffix);

        // Generate unique temp filename with timestamp and process ID
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .clone()
            .as_millis();
        let pid = std::process::id();
        let temp_filename = format!(
            "{}_{}_{}_{}.tmp",
            filename,
            timestamp,
            pid,
            rand::random::<u32>()
        );

        // Reconstruct the full URL if needed
        let temp_path = temp_dir.join(temp_filename).to_string_lossy().to_string();
        if !base_url.is_empty() {
            Ok(format!("{}{}", base_url, temp_path))
        } else {
            Ok(temp_path)
        }
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
        // Extract parent directory handling URLs properly
        let temp_parent = if temp_path.contains("://") {
            // For URLs, find the parent directory part
            if let Some(last_slash) = temp_path.rfind('/') {
                Some(temp_path[..last_slash].to_string())
            } else {
                None
            }
        } else {
            Path::new(&temp_path).parent().map(|p| p.to_string_lossy().to_string())
        };
        
        if let Some(parent) = temp_parent {
            if let Err(e) = filesystem.create_dir_all(&parent).await {
                tracing::warn!("Failed to create temp parent directory {}: {}", parent, e);
                // Try to continue, as the directory might already exist
            }
        }
        
        let final_parent = if final_path.contains("://") {
            // For URLs, find the parent directory part
            if let Some(last_slash) = final_path.rfind('/') {
                Some(final_path[..last_slash].to_string())
            } else {
                None
            }
        } else {
            Path::new(final_path).parent().map(|p| p.to_string_lossy().to_string())
        };
        
        if let Some(parent) = final_parent {
            if let Err(e) = filesystem.create_dir_all(&parent).await {
                return Err(FilesystemError::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to create parent directory {}: {}", parent, e)
                )));
            }
        }

        // Write to temp file
        debug!("🔍 DEBUG ATOMIC: Writing to temp_path: {}", temp_path);
        filesystem.write(&temp_path, data, None).await?;
        debug!("🔍 DEBUG ATOMIC: Successfully wrote to temp_path: {}", temp_path);

        // Atomic move to final location (move is atomic, copy is not)
        debug!("🔍 DEBUG ATOMIC: Moving from temp_path: {} to final_path: {}", temp_path, final_path);
        filesystem.move_file(&temp_path, final_path).await?;
        debug!("🔍 DEBUG ATOMIC: Successfully moved to final_path: {}", final_path);

        Ok(())
    }

    async fn cleanup_temp_files(&self, _filesystem: &dyn FileSystem) -> FsResult<()> {
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
    compression: bool,
    chunk_size_mb: usize,
    config: AtomicWriteConfig,
}

impl CloudOptimizedExecutor {
    pub fn new(
        local_temp_dir: PathBuf,
        compression: bool,
        chunk_size_mb: usize,
        config: AtomicWriteConfig,
    ) -> Self {
        Self {
            local_temp_dir,
            compression,
            chunk_size_mb,
            config,
        }
    }

    async fn compress_data(&self, data: &[u8]) -> FsResult<Vec<u8>> {
        if !self.compression {
            return Ok(data.to_vec());
        }

        // Use compression (e.g., Snappy for speed or ZSTD for ratio)
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder
            .write_all(data)
            .map_err(|e| FilesystemError::Io(e))?;
        encoder.finish().map_err(|e| FilesystemError::Io(e))
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
            .clone()
            .as_millis();
        let pid = std::process::id();
        let temp_filename = format!("proximadb_{}_{}.tmp", timestamp, pid);
        let local_temp_path = self.local_temp_dir.join(temp_filename);

        // Compress data if enabled
        let data_to_write = self.compress_data(data).await?;

        // Write to local temp file first
        let local_fs = crate::storage::persistence::filesystem::local::LocalFileSystem::new(
            crate::storage::persistence::filesystem::local::LocalConfig::default(),
        )
        .await?;

        local_fs
            .write(&local_temp_path.to_string_lossy(), &data_to_write, None)
            .await?;

        // Atomic flush to cloud storage
        let cloud_data = if self.compression {
            // If compressed, upload compressed data
            data_to_write
        } else {
            data.to_vec()
        };

        // Use atomic write if filesystem supports it, otherwise fallback to regular write
        if filesystem.supports_atomic_writes() {
            filesystem
                .write_atomic(final_path, &cloud_data, options)
                .await?;
        } else {
            filesystem.write(final_path, &cloud_data, None).await?;
        }

        // Cleanup local temp file
        local_fs
            .delete(&local_temp_path.to_string_lossy())
            .await
            .ok();

        Ok(())
    }

    async fn cleanup_temp_files(&self, _filesystem: &dyn FileSystem) -> FsResult<()> {
        // Clean up local temp directory
        tracing::debug!(
            "Cleaning up cloud-optimized temp files in: {:?}",
            self.local_temp_dir
        );
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

    fn create_executor_for_filesystem(
        &self,
        filesystem: &dyn FileSystem,
    ) -> Box<dyn AtomicWriteExecutor> {
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
        executor
            .write_atomic(filesystem, final_path, data, options)
            .await
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
            AtomicWriteStrategy::Direct => Box::new(DirectWriteExecutor),
            AtomicWriteStrategy::SameMountTemp { temp_suffix } => Box::new(
                SameMountTempExecutor::new(temp_suffix.clone(), config.clone()),
            ),
            AtomicWriteStrategy::ConfiguredTemp { temp_directory } => {
                Box::new(CloudOptimizedExecutor::new(
                    temp_directory.clone(),
                    false, // No compression for configured temp
                    8,
                    config.clone(),
                ))
            }
            AtomicWriteStrategy::CloudOptimized {
                local_temp_dir,
                compression,
                chunk_size_mb,
            } => Box::new(CloudOptimizedExecutor::new(
                local_temp_dir.clone(),
                *compression,
                *chunk_size_mb,
                config.clone(),
            )),
            AtomicWriteStrategy::AutoDetect => Box::new(AutoDetectExecutor::new(config.clone())),
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
                compression: true,
                chunk_size_mb: 8,
            },
            ..Default::default()
        };
        Box::new(CloudOptimizedExecutor::new(local_temp_dir, true, 8, config))
    }
}
