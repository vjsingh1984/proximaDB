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

//! # Filesystem Abstraction Layer - Unified Storage Interface
//!
//! This module provides ProximaDB's cloud-native filesystem abstraction that enables
//! seamless operation across local and cloud storage systems. It implements the Strategy
//! Pattern with Abstract Factory for automatic backend selection based on URL schemes.
//!
//! ## Role in ProximaDB Architecture
//!
//! The filesystem layer provides storage-agnostic operations:
//! ```text
//! Storage Engines → Filesystem API → Backend Selection
//!                                           ↓
//!                    ┌──────────────────────┴──────────────────┐
//!                    │         Storage Backends                 │
//!                    ├───────────────────────────────────────────┤
//!                    │ Local │ S3 │ Azure │ GCS │ HDFS │       │
//!                    └───────────────────────────────────────────┘
//!                                           ↓
//!                          Zero-Copy I/O + Caching Layer
//! ```
//!
//! ## Supported Storage Backends
//!
//! | Scheme | Backend | Features | Use Case |
//! |--------|---------|----------|----------|
//! | `file://` | Local filesystem | Direct I/O, memory mapping | Development, single-node |
//! | `s3://` | Amazon S3 | IAM roles, STS, multipart | AWS deployments |
//! | `adls://` | Azure Data Lake | Managed identity, SAS | Azure deployments |
//! | `gcs://` | Google Cloud Storage | Service accounts, ADC | GCP deployments |
//! | `hdfs://` | Hadoop HDFS | Kerberos, HA namenode | Big data clusters |
//!
//! ## Key Features
//!
//! ### 1. **Transparent Backend Selection**
//! Automatic routing based on URL scheme:
//! ```rust,ignore
//! use std::sync::Arc;
//! // Create a factory and get a filesystem by URL
//! let factory = Arc::new(FilesystemFactory::create_default().await?);
//! let fs = factory.get_filesystem("s3://bucket/path")?; // S3 backend
//! let local = factory.get_filesystem("file:///data/vectors")?; // Local backend
//! ```
//!
//! ### 2. **Atomic Operations**
//! Configurable strategies for data consistency:
//! - **DirectWrite**: For filesystems with native atomicity
//! - **SameDirectory**: Temp files in `___temp/` subdirectory
//! - **ConfiguredTemp**: User-specified temp location
//! - **SystemTemp**: Fallback to `/tmp` for development
//!
//! ### 3. **Zero-Copy I/O System**
//! High-performance I/O with intelligent caching:
//! - Memory-mapped files for local storage
//! - Bandwidth optimization for cloud
//! - Prefetching and read-ahead
//! - LRU cache with TTL support
//!
//! ### 4. **Cloud-Native Authentication**
//! Automatic credential management:
//! - **AWS**: IAM roles, instance profiles, STS
//! - **Azure**: Managed identity, service principals
//! - **GCS**: Service accounts, application default
//! - **HDFS**: Kerberos, simple auth
//!
//! ## Performance Characteristics
//!
//! - **Local I/O**: < 1ms latency, GB/s throughput
//! - **S3 Operations**: 10-50ms latency, 100MB/s throughput
//! - **Cache Hit Rate**: 80-95% for hot data
//! - **Memory Mapping**: Zero-copy for local files
//! - **Multipart Upload**: Parallel chunks for large files
//!
//! ## Configuration
//!
//! ```toml
//! [storage.filesystem]
//! # Default filesystem URL
//! default_url = "file:///data"
//!
//! # Atomic write strategy
//! temp_strategy = "same_directory"
//!
//! # Zero-copy configuration
//! enable_mmap = true
//! cache_size_mb = 1024
//! prefetch_size_kb = 256
//!
//! # S3 specific
//! [storage.filesystem.s3]
//! region = "us-west-2"
//! max_connections = 100
//! multipart_threshold_mb = 64
//!
//! # Azure specific
//! [storage.filesystem.azure]
//! account = "myaccount"
//! use_managed_identity = true
//! ```
//!
//! ## Module Organization
//!
//! - **`local.rs`**: Local filesystem implementation
//! - **`s3.rs`**: Amazon S3 backend
//! - **`azure.rs`**: Azure Data Lake Storage
//! - **`gcs.rs`**: Google Cloud Storage
//! - **`hdfs.rs`**: Hadoop HDFS support
//! - **`zero_copy_filesystem.rs`**: High-performance I/O layer
//! - **`atomic_strategy.rs`**: Atomic write implementations
//! - **`manager.rs`**: Filesystem factory and routing
//! - **`auth/`**: Authentication providers
//!
//! ## Usage Examples
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use proximadb::storage::persistence::filesystem::{FilesystemFactory, FileOptions};
//!
//! // Create factory and filesystem
//! let factory = Arc::new(FilesystemFactory::create_default().await?);
//! let fs = factory.get_filesystem("s3://my-bucket/vectors")?;
//!
//! // Write with atomic guarantees
//! fs.write_atomic(
//!     "collection/segment.parquet",
//!     data,
//!     FileOptions::default()
//! ).await?;
//!
//! // Read with caching
//! let content = fs.read("collection/segment.parquet").await?;
//!
//! // List directory
//! let entries = fs.list("collection/").await?;
//! ```
//!
//! ## Error Handling
//!
//! The module provides detailed error types:
//! - `FilesystemError::Io`: Low-level I/O failures
//! - `FilesystemError::Auth`: Authentication issues
//! - `FilesystemError::Network`: Connection problems
//! - `FilesystemError::NotFound`: Missing files/paths
//!
//! ## Cloud Cost Optimization
//!
//! Built-in features to minimize cloud storage costs:
//! - Intelligent caching reduces API calls
//! - Batch operations for list/delete
//! - Storage class transitions (S3 IA, Glacier)
//! - Bandwidth optimization with compression

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Error as IoError;
use std::sync::Arc;
use tracing::{debug, error, info, trace};
use url::Url;

pub mod atomic_strategy;
pub mod auth;
// intelligent_filesystem removed - using UnifiedCachingFilesystem instead
pub mod local;
pub mod manager;
pub mod scheme_validation;
pub mod write_strategy;
// zero_copy_filesystem removed - functionality integrated into UnifiedCachingFilesystem

// Unified filesystem modules
pub mod access_tracker;
pub mod cache_config;
pub mod cache_metrics;
pub mod caching_filesystem;
pub mod disk_cache;
pub mod metadata_cache;
pub mod metadata_traits;
pub mod orchestrator_integration;
pub mod prefetch_engine;
pub mod range_optimizer;
pub mod smart_io;

// Filesystem implementations
pub use local::LocalFileSystem;

// Unified caching filesystem
pub use caching_filesystem::UnifiedCachingFilesystem;

// Re-export centralized scheme validation functions
pub use scheme_validation::{
    FilesystemScheme, extract_scheme, is_supported_scheme, normalize_url, validate_url,
};

/// Filesystem operation result type
pub type FsResult<T> = Result<T, FilesystemError>;

/// Filesystem error types
#[derive(Debug, thiserror::Error)]
pub enum FilesystemError {
    #[error("IO error: {0}")]
    Io(#[from] IoError),

    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Unsupported filesystem scheme: {0}")]
    UnsupportedScheme(String),

    #[error("File not found: {0}")]
    NotFound(String),

    #[error("Already exists: {0}")]
    AlreadyExists(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
}

/// File metadata information
#[derive(Debug, Clone, Default)]
pub struct FileMetadata {
    pub path: String,
    pub size: u64,
    pub created: Option<chrono::DateTime<chrono::Utc>>,
    pub modified: Option<chrono::DateTime<chrono::Utc>>,
    pub is_directory: bool,
    pub permissions: Option<String>,
    pub etag: Option<String>,          // For cloud storage
    pub storage_class: Option<String>, // For cloud storage
}

/// Directory listing entry (stateless design - contains full URL)
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub url: String, // Full URL instead of relative path
    pub metadata: FileMetadata,
}

/// Temporary directory strategy for atomic operations
#[derive(Debug, Clone, Default)]
pub enum TempStrategy {
    /// Direct write (no temp files) - for local filesystem with atomic guarantees
    DirectWrite,

    /// Write to ___temp subdirectory in same location (same mount point)
    /// Ensures move operations are filesystem renames, not copies
    #[default]
    SameDirectory,

    /// Write to user-configured temp directory
    /// Falls back to system /tmp if not configured (R&D mode)
    ConfiguredTemp {
        /// Custom temp directory path (optional)
        temp_dir: Option<String>,
    },

    /// Write to system /tmp directory (fallback for R&D)
    SystemTemp,
}

/// File operation options
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileOptions {
    pub create_dirs: bool,
    pub overwrite: bool,
    pub buffer_size: Option<usize>,
    pub encryption: Option<String>,
    pub storage_class: Option<String>, // For cloud storage
    pub metadata: Option<HashMap<String, String>>,

    /// Pre-computed temp path (cached for performance)
    /// None means direct write, Some means atomic write-temp-rename
    pub temp_path: Option<String>,
}

/// Authentication configuration for cloud providers
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// AWS authentication method
    pub aws_auth: Option<AwsAuthMethod>,

    /// Azure authentication method
    pub azure_auth: Option<AzureAuthMethod>,

    /// GCS authentication method
    pub gcs_auth: Option<GcsAuthMethod>,

    /// Enable credential caching
    pub enable_credential_caching: bool,

    /// Credential refresh interval (seconds)
    pub credential_refresh_interval_seconds: u64,
}

#[derive(Debug, Clone)]
pub enum AwsAuthMethod {
    /// Use AWS IAM roles (recommended for EC2/ECS)
    IamRole,
    /// Use AWS credentials file
    CredentialsFile { profile: Option<String> },
    /// Use environment variables
    Environment,
    /// Use STS temporary credentials
    StsAssumeRole {
        role_arn: String,
        session_name: String,
    },
}

#[derive(Debug, Clone)]
pub enum AzureAuthMethod {
    /// Use Azure Managed Identity
    ManagedIdentity,
    /// Use Azure Service Principal
    ServicePrincipal {
        client_id: String,
        tenant_id: String,
    },
    /// Use Azure CLI authentication
    AzureCli,
    /// Use environment variables
    Environment,
}

#[derive(Debug, Clone)]
pub enum GcsAuthMethod {
    /// Use Application Default Credentials
    ApplicationDefault,
    /// Use service account file
    ServiceAccountFile { path: String },
    /// Use service account key
    ServiceAccountKey { key_json: String },
    /// Use environment variables
    Environment,
}

/// Retry configuration for operations
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries
    pub max_retries: u32,
    /// Initial delay between retries (ms)
    pub initial_delay_ms: u64,
    /// Maximum delay between retries (ms)
    pub max_delay_ms: u64,
    /// Backoff multiplier for exponential backoff
    pub backoff_multiplier: f64,
}

/// Storage tier type for intelligent data placement
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileStorageTier {
    /// In-memory storage (fastest)
    Memory,
    /// NVMe SSD storage (microsecond latency)
    NVMe,
    /// SSD storage (millisecond latency)
    SSD,
    /// HDD storage (10-20ms latency)
    HDD,
    /// S3 Express One Zone (single-digit ms)
    S3Express,
    /// S3 Standard (10-100ms)
    S3Standard,
    /// S3 Glacier Instant (100ms+)
    S3GlacierInstant,
    /// Azure Premium SSD
    AzurePremium,
    /// Azure Standard SSD
    AzureStandard,
    /// Google Cloud SSD
    GcsSSD,
    /// Google Cloud HDD
    GcsHDD,
}

impl FileStorageTier {
    /// Get expected latency in microseconds
    pub fn expected_latency_us(&self) -> u64 {
        match self {
            FileStorageTier::Memory => 1,                 // <1μs
            FileStorageTier::NVMe => 100,                 // 100μs
            FileStorageTier::SSD => 1_000,                // 1ms
            FileStorageTier::HDD => 10_000,               // 10ms
            FileStorageTier::S3Express => 5_000,          // 5ms
            FileStorageTier::S3Standard => 50_000,        // 50ms
            FileStorageTier::S3GlacierInstant => 100_000, // 100ms
            FileStorageTier::AzurePremium => 500,         // 500μs
            FileStorageTier::AzureStandard => 2_000,      // 2ms
            FileStorageTier::GcsSSD => 800,               // 800μs
            FileStorageTier::GcsHDD => 15_000,            // 15ms
        }
    }

    /// Get optimal I/O size in bytes for this tier
    pub fn optimal_io_size(&self) -> usize {
        match self {
            FileStorageTier::Memory => 64 * 1024,                 // 64KB
            FileStorageTier::NVMe => 128 * 1024,                  // 128KB
            FileStorageTier::SSD => 256 * 1024,                   // 256KB
            FileStorageTier::HDD => 1024 * 1024,                  // 1MB
            FileStorageTier::S3Express => 512 * 1024,             // 512KB
            FileStorageTier::S3Standard => 1024 * 1024,           // 1MB
            FileStorageTier::S3GlacierInstant => 4 * 1024 * 1024, // 4MB
            FileStorageTier::AzurePremium => 256 * 1024,          // 256KB
            FileStorageTier::AzureStandard => 512 * 1024,         // 512KB
            FileStorageTier::GcsSSD => 256 * 1024,                // 256KB
            FileStorageTier::GcsHDD => 2 * 1024 * 1024,           // 2MB
        }
    }

    /// Check if this tier is faster than another
    pub fn is_faster_than(&self, other: &FileStorageTier) -> bool {
        self.expected_latency_us() < other.expected_latency_us()
    }
}

/// Tier-specific storage configuration
#[derive(Debug, Clone)]
pub struct TierConfig {
    /// Storage tier type
    pub tier: FileStorageTier,

    /// Base URL for this tier (e.g., "file:///mnt/nvme", "s3://bucket")
    pub base_url: String,

    /// Maximum capacity in bytes (None = unlimited)
    pub max_capacity_bytes: Option<u64>,

    /// Current usage in bytes (tracked runtime)
    pub current_usage_bytes: u64,

    /// Enable compression for this tier
    pub compression: bool,

    /// Custom I/O size override (uses tier default if None)
    pub io_size_override: Option<usize>,
}

/// Filesystem performance configuration
#[derive(Debug, Clone)]
pub struct FilesystemPerformanceConfig {
    /// Connection pool size per backend
    pub connection_pool_size: usize,

    /// Enable connection keep-alive
    pub enable_keep_alive: bool,

    /// Request timeout (seconds)
    pub request_timeout_seconds: u64,

    /// Enable compression for network transfers
    pub compression: bool,

    /// Retry configuration
    pub retry_config: RetryConfig,

    /// Buffer size for operations (bytes)
    pub buffer_size: usize,

    /// Enable parallel operations
    pub enable_parallel_ops: bool,

    /// Maximum concurrent operations
    pub max_concurrent_ops: usize,

    /// Tier-specific configurations
    pub tier_configs: Vec<TierConfig>,
}

/// File handle trait for streaming operations on large files
/// Provides async read/write capabilities similar to tokio::fs::File
#[async_trait]
pub trait FilesystemFile: Send + Sync + std::fmt::Debug {
    /// Read data from current position
    async fn read(&mut self, buf: &mut [u8]) -> FsResult<usize>;

    /// Write data at current position
    async fn write(&mut self, buf: &[u8]) -> FsResult<usize>;

    /// Flush any buffered writes
    async fn flush(&mut self) -> FsResult<()>;

    /// Seek to position (if supported)
    async fn seek(&mut self, pos: u64) -> FsResult<u64>;

    /// Get current position
    async fn position(&self) -> FsResult<u64>;

    /// Get file size
    async fn file_size(&self) -> FsResult<u64>;

    /// Sync data to underlying storage
    async fn sync_all(&mut self) -> FsResult<()>;
}

/// Abstract filesystem trait for strategy pattern
#[async_trait]
pub trait FileSystem: Send + Sync + std::fmt::Debug {
    /// Get self as Any for downcasting to concrete types
    fn as_any(&self) -> &dyn std::any::Any;

    /// Read file contents
    async fn read(&self, path: &str) -> FsResult<Vec<u8>>;

    /// Get memory-mapped access to a file (only supported for local filesystem)
    /// Returns None if memory mapping is not supported (e.g., cloud storage)
    /// The returned mmap is read-only and safe for concurrent access
    async fn get_mmap(&self, _path: &str) -> FsResult<Option<memmap2::Mmap>> {
        // Default implementation returns None (not supported)
        // LocalFileSystem will override this to provide actual memory mapping
        Ok(None)
    }

    /// Check if this filesystem supports memory mapping
    fn supports_mmap(&self) -> bool {
        false // Default: most filesystems don't support mmap
    }

    /// Read specific byte range from file (for efficient cloud storage access)
    /// Returns the requested bytes. Default implementation reads entire file and slices.
    async fn read_range(&self, path: &str, offset: u64, length: u64) -> FsResult<Vec<u8>> {
        // Default implementation for backwards compatibility
        let data = self.read(path).await?;
        let start = offset as usize;
        let end = (offset + length) as usize;

        if start >= data.len() {
            return Ok(vec![]);
        }

        let end = end.min(data.len());
        Ok(data[start..end].to_vec())
    }

    /// Read multiple byte ranges from file in a single operation
    /// Optimizes for cloud storage by batching requests
    async fn read_ranges(
        &self,
        path: &str,
        ranges: Vec<std::ops::Range<u64>>,
    ) -> FsResult<Vec<Vec<u8>>> {
        // Default implementation calls read_range for each range
        let mut results = Vec::with_capacity(ranges.len());
        for range in ranges {
            let length = range.end - range.start;
            results.push(self.read_range(path, range.start, length).await?);
        }
        Ok(results)
    }

    /// Write file contents
    async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()>;

    /// Sync file data to disk (fsync/fdatasync)
    /// Ensures data durability after write operations
    /// Returns Ok(()) if sync is not supported by the filesystem
    async fn sync_file(&self, _path: &str) -> FsResult<()> {
        // Default implementation - no sync
        // Filesystems that support sync should override
        Ok(())
    }

    /// Append to file
    async fn append(&self, path: &str, data: &[u8]) -> FsResult<()>;

    /// Delete file or directory
    async fn delete(&self, path: &str) -> FsResult<()>;

    /// Check if file exists
    async fn exists(&self, path: &str) -> FsResult<bool>;

    /// Get file metadata
    async fn metadata(&self, path: &str) -> FsResult<FileMetadata>;

    /// List directory contents
    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>>;

    /// Create directory
    async fn create_dir(&self, path: &str) -> FsResult<()>;

    /// Create directory and all parent directories
    async fn create_dir_all(&self, path: &str) -> FsResult<()>;

    /// Copy file
    async fn copy(&self, from: &str, to: &str) -> FsResult<()>;

    /// Move/rename file
    async fn move_file(&self, from: &str, to: &str) -> FsResult<()>;

    /// Get filesystem type identifier
    fn filesystem_type(&self) -> &'static str;

    /// Check if filesystem supports atomic writes natively
    /// Local filesystems can write directly, object stores need atomic pattern
    fn supports_atomic_writes(&self) -> bool {
        match self.filesystem_type() {
            "local" => true, // Local filesystem supports atomic writes natively
            _ => false,      // Object stores (S3, ADLS, GCS) need write-temp-rename pattern
        }
    }

    /// Generate temporary file path based on strategy (called once during setup)
    /// Ensures optimal temp location for each filesystem type
    fn generate_temp_path(&self, final_path: &str, strategy: &TempStrategy) -> FsResult<String> {
        use std::env;
        use std::path::{Path, PathBuf};

        let final_path = Path::new(final_path);
        let filename = final_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| FilesystemError::InvalidPath("Invalid filename".to_string()))?;

        match strategy {
            TempStrategy::DirectWrite => {
                // No temp file needed for direct writes
                Err(FilesystemError::InvalidOperation(
                    "DirectWrite strategy should not generate temp paths".to_string(),
                ))
            }

            TempStrategy::SameDirectory => {
                // Create ___temp subdirectory in same location (same mount point)
                let parent = final_path.parent();
                let temp_dir = parent.map_or_else(
                    || std::path::PathBuf::from("___temp"),
                    |p| p.join("___temp"),
                );
                let temp_file = temp_dir.join(format!("{}.{}", filename, std::process::id())); // Add PID for uniqueness
                Ok(temp_file.to_string_lossy().to_string())
            }

            TempStrategy::ConfiguredTemp { temp_dir } => {
                // Use configured temp dir or fall back to system temp
                let temp_base = if let Some(dir) = temp_dir {
                    PathBuf::from(dir)
                } else {
                    // Fallback to system temp for R&D mode
                    env::temp_dir()
                };
                let temp_file =
                    temp_base.join(format!("proximadb_{}.{}", filename, std::process::id()));
                Ok(temp_file.to_string_lossy().to_string())
            }

            TempStrategy::SystemTemp => {
                // Use system /tmp directory
                let temp_file =
                    env::temp_dir().join(format!("proximadb_{}.{}", filename, std::process::id()));
                Ok(temp_file.to_string_lossy().to_string())
            }
        }
    }

    /// Fast atomic write using pre-computed temp path (performance optimized)
    /// Called during actual write operations with cached temp strategy
    async fn write_atomic(
        &self,
        final_path: &str,
        data: &[u8],
        options: Option<FileOptions>,
    ) -> FsResult<()> {
        let temp_path_opt = options.as_ref().and_then(|o| o.temp_path.clone());

        match temp_path_opt {
            None => {
                // Direct write (optimal for local filesystem)
                self.write(final_path, data, options).await
            }
            Some(temp_path_str) => {
                // Atomic write-temp-rename (optimal for object stores)
                let temp_path = std::path::Path::new(&temp_path_str);

                // Ensure temp directory exists
                if let Some(temp_parent) = temp_path.parent() {
                    self.create_dir_all(&temp_parent.to_string_lossy()).await?;
                }

                // Write to temp location
                let temp_opts = options.map(|o| FileOptions {
                    temp_path: None, // Prevent recursion
                    ..o
                });
                self.write(&temp_path_str, data, temp_opts).await?;

                // Atomic move (rename on same mount point)
                self.move_file(&temp_path_str, final_path).await
            }
        }
    }

    /// Sync/flush operations to storage
    async fn sync(&self) -> FsResult<()>;

    /// Read file as string (UTF-8) - convenience method for text files
    async fn read_to_string(&self, path: &str) -> FsResult<String> {
        let bytes = self.read(path).await?;
        String::from_utf8(bytes)
            .map_err(|e| FilesystemError::InvalidOperation(format!("Invalid UTF-8: {}", e)))
    }

    /// Write string to file - convenience method for text files
    async fn write_string(
        &self,
        path: &str,
        content: &str,
        options: Option<FileOptions>,
    ) -> FsResult<()> {
        self.write(path, content.as_bytes(), options).await
    }

    /// Remove directory and all contents recursively
    async fn remove_dir_all(&self, path: &str) -> FsResult<()> {
        // Default implementation using list and delete
        let entries = self.list(path).await?;

        for entry in entries {
            if entry.metadata.is_directory {
                self.remove_dir_all(&entry.url).await?;
            } else {
                self.delete(&entry.url).await?;
            }
        }

        self.delete(path).await
    }

    /// Create a file handle for streaming operations (for large files)
    /// Returns a file handle that implements AsyncRead + AsyncWrite
    async fn open_file(&self, path: &str, create: bool) -> FsResult<Box<dyn FilesystemFile>>;
}

/// Filesystem factory configuration
#[derive(Debug, Clone)]
pub struct FilesystemConfig {
    /// Default filesystem URL for unqualified paths
    pub default_fs: Option<String>,

    /// Local filesystem configuration
    pub local: Option<local::LocalConfig>,

    /// Global filesystem options
    pub global_options: FileOptions,

    /// Authentication configuration
    pub auth_config: Option<AuthConfig>,

    /// Performance optimization settings
    pub performance_config: FilesystemPerformanceConfig,

    /// Scheme mapping for URL scheme overrides (e.g., "gs" -> "gcs")
    pub scheme_mapping: HashMap<String, String>,
}

impl Default for FilesystemPerformanceConfig {
    fn default() -> Self {
        Self {
            connection_pool_size: 10,
            enable_keep_alive: true,
            request_timeout_seconds: 30,
            compression: true,
            retry_config: RetryConfig {
                max_retries: 3,
                initial_delay_ms: 100,
                max_delay_ms: 5000,
                backoff_multiplier: 2.0,
            },
            buffer_size: 8 * 1024 * 1024, // 8MB
            enable_parallel_ops: true,
            max_concurrent_ops: 100,
            tier_configs: vec![],
        }
    }
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            default_fs: Some("file://".to_string()),
            local: Some(local::LocalConfig::default()),
            global_options: FileOptions::default(),
            auth_config: None,
            performance_config: FilesystemPerformanceConfig::default(),
            scheme_mapping: {
                let mut mapping = HashMap::new();
                mapping.insert("gs".to_string(), "gcs".to_string()); // Support Google Cloud gs:// scheme
                mapping
            },
        }
    }
}

/// Abstract factory for creating filesystem instances
pub struct FilesystemFactory {
    config: FilesystemConfig,
    filesystems: HashMap<String, Arc<dyn FileSystem>>,
    tier_mapping: HashMap<FileStorageTier, String>,
}

impl std::fmt::Debug for FilesystemFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesystemFactory")
            .field("config", &self.config)
            .field("filesystems_count", &self.filesystems.len())
            .finish()
    }
}

impl FilesystemFactory {
    fn maybe_wrap_with_encryption(
        &self,
        filesystem: Arc<dyn FileSystem>,
    ) -> FsResult<Arc<dyn FileSystem>> {
        let Some(env_var) = self.config.global_options.encryption.as_deref() else {
            return Ok(filesystem);
        };

        let env_var = env_var.trim();
        if env_var.is_empty() {
            return Err(FilesystemError::Config(
                "Filesystem encryption env var name cannot be empty".to_string(),
            ));
        }

        let key_manager =
            crate::storage::encryption::KeyManager::from_env(env_var).map_err(|e| {
                FilesystemError::Config(format!(
                    "Failed to load filesystem encryption key from {}: {}",
                    env_var, e
                ))
            })?;
        let version_manager = Arc::new(crate::storage::encryption::KeyVersionManager::new(
            Arc::new(key_manager),
        ));

        Ok(Arc::new(
            crate::storage::encryption::EncryptedFilesystem::new(filesystem, version_manager, true),
        ))
    }

    /// Create filesystem factory with default configuration
    ///
    /// **DEPRECATED**: Use `create_default()` instead. This method creates a non-functional
    /// factory without registered filesystems and exists only for backward compatibility.
    #[deprecated(
        since = "0.1.5",
        note = "Use `create_default()` instead - this creates a broken factory"
    )]
    #[allow(clippy::should_implement_trait)]
    pub fn default() -> Self {
        Self {
            config: FilesystemConfig::default(),
            filesystems: HashMap::new(),
            tier_mapping: HashMap::new(),
        }
    }

    /// Create a fully initialized filesystem factory with default configuration
    ///
    /// This is the preferred way to create a FilesystemFactory. It initializes all
    /// filesystem backends (local, cloud) based on the default configuration.
    ///
    /// # Examples
    /// ```ignore
    /// let factory = FilesystemFactory::create_default().await?;
    /// let fs = factory.get_filesystem("file:///tmp/data")?;
    /// ```text
    pub async fn create_default() -> FsResult<Self> {
        Self::create(FilesystemConfig::default()).await
    }

    /// Create a fully initialized filesystem factory with custom configuration
    ///
    /// This static factory method creates and initializes a FilesystemFactory with
    /// all configured filesystem backends registered and ready to use.
    ///
    /// # Arguments
    /// * `config` - Filesystem configuration specifying which backends to enable
    ///
    /// # Examples
    /// ```ignore
    /// let config = FilesystemConfig {
    ///     local: Some(LocalConfig::default()),
    ///     ..Default::default()
    /// };
    /// let factory = FilesystemFactory::create(config).await?;
    /// ```text
    pub async fn create(config: FilesystemConfig) -> FsResult<Self> {
        let mut factory = Self {
            config,
            filesystems: HashMap::new(),
            tier_mapping: HashMap::new(),
        };

        // Pre-initialize configured filesystems
        factory.initialize_filesystems().await?;

        // Build tier mapping from configuration
        factory.initialize_tier_mapping();

        Ok(factory)
    }

    /// Create new filesystem factory with configuration
    ///
    /// **DEPRECATED**: Use `create()` instead for clearer semantics.
    /// Having `new()` on a factory is confusing - factories should have static
    /// creation methods like `create()`, `create_default()`, etc.
    #[deprecated(
        since = "0.1.5",
        note = "Use `create(config)` instead for clearer factory semantics"
    )]
    pub async fn new(config: FilesystemConfig) -> FsResult<Self> {
        Self::create(config).await
    }

    /// Initialize all configured filesystem backends
    async fn initialize_filesystems(&mut self) -> FsResult<()> {
        // Initialize local filesystem with root directory resolution
        if let Some(local_config) = &self.config.local {
            let local_fs: Arc<dyn FileSystem> =
                Arc::new(LocalFileSystem::new(local_config.clone()).await?);
            self.filesystems.insert(
                "file".to_string(),
                self.maybe_wrap_with_encryption(local_fs)?,
            );
        } else {
            // Create default local filesystem without root restriction
            let default_config = local::LocalConfig::default();
            let local_fs: Arc<dyn FileSystem> =
                Arc::new(LocalFileSystem::new(default_config).await?);
            self.filesystems.insert(
                "file".to_string(),
                self.maybe_wrap_with_encryption(local_fs)?,
            );
        }

        Ok(())
    }

    /// Get filesystem instance for URL scheme (cached instances)
    /// Get filesystem instance for URL scheme (returns Arc for safe sharing).
    ///
    /// Use this when you need raw filesystem access without caching.
    pub fn get_filesystem(&self, url: &str) -> FsResult<Arc<dyn FileSystem>> {
        // Use centralized scheme extraction and validation
        let scheme = extract_scheme(url)?;
        let scheme_str = scheme.as_str();

        self.filesystems
            .get(scheme_str)
            .cloned()
            .ok_or_else(|| FilesystemError::UnsupportedScheme(scheme_str.to_string()))
    }

    /// Get an IntelligentFilesystem with automatic scheme-specific filesystem selection.
    ///
    /// This is the RECOMMENDED method for engines to get filesystems.
    /// It automatically:
    /// 1. Selects the right filesystem based on URL scheme
    /// 2. Wraps it with IntelligentFilesystem for caching
    /// 3. Returns a ready-to-use cached filesystem
    ///
    /// ## Benefits
    ///
    /// - **Cloud Storage**: Dramatically reduces API calls through metadata caching
    /// - **Local Storage**: Adds bloom filter and block caching
    /// - **All Storage**: Access pattern learning and predictive prefetching
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// // Instead of:
    /// let fs = factory.get_filesystem("s3://bucket")?;
    /// let cached_fs = IntelligentFilesystem::new(fs, collection_id, engine_type);
    ///
    /// // Just do:
    /// let cached_fs = factory.get_intelligent_filesystem(
    ///     "s3://bucket",
    ///     collection_id,
    ///     engine_type,
    /// )?;
    /// ```text
    #[deprecated(
        since = "1.0.0",
        note = "Use get_unified_caching_filesystem instead. This method now redirects to it."
    )]
    pub fn get_intelligent_filesystem(
        &self,
        url: &str,
        collection_id: String,
        engine_type: String,
    ) -> FsResult<Arc<dyn FileSystem>> {
        // Redirect to the new unified filesystem
        self.get_unified_caching_filesystem(url, collection_id, engine_type)
    }

    /// Create filesystem with unified caching
    ///
    /// # Example
    /// ```text
    /// let cached_fs = factory.get_unified_caching_filesystem(
    ///     "s3://bucket/collection",
    ///     "collection_123".to_string(),
    ///     "sst".to_string(),
    /// )?;
    /// ```text
    pub fn get_unified_caching_filesystem(
        &self,
        url: &str,
        collection_id: String,
        engine_type: String,
    ) -> FsResult<Arc<dyn FileSystem>> {
        // Get the appropriate filesystem for this URL
        let fs = self.get_filesystem(url)?;

        // Get the metadata serializer for this engine type
        let metadata_serializer: Arc<dyn crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer> =
            match engine_type.as_str() {
                "sst" => Arc::new(crate::storage::engines::core::sst_format_serializer::SstUnifiedMetadataSerializer::new()),
                "viper" => Arc::new(crate::storage::engines::core::parquet_format_serializer::ViperMetadataSerializer::new()),
                "raptor" => Arc::new(crate::storage::engines::core::matrix_trinity_serializer::RaptorUnifiedMetadataSerializer::new()),
                "nova" => Arc::new(crate::storage::engines::core::columnar_format_serializer::NovaUnifiedMetadataSerializer::new()),
                "swift" => Arc::new(crate::storage::engines::core::proximablocks_compact_serializer::SwiftUnifiedMetadataSerializer::new()),
                "helix" => Arc::new(crate::storage::engines::core::proximablocks_format_serializer::HelixUnifiedMetadataSerializer::new()),
                _ => {
                    // Default serializer for other engines
                    #[derive(Debug)]
                    struct DefaultSerializer;
                    impl crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer for DefaultSerializer {
                        fn serialize(&self, _metadata: &dyn std::any::Any) -> anyhow::Result<bytes::Bytes> {
                            Ok(bytes::Bytes::new())
                        }
                        fn deserialize(&self, _bytes: &[u8]) -> anyhow::Result<Box<dyn std::any::Any + Send + Sync>> {
                            Ok(Box::new(()))
                        }
                        fn engine_type(&self) -> &str { "default" }
                        fn extract_cacheable_component(&self, _data: &[u8], _file_path: &str) -> Option<bytes::Bytes> {
                            None
                        }
                        fn should_cache_metadata(&self, _file_path: &str) -> bool { false }
                    }
                    Arc::new(DefaultSerializer)
                }
            };

        // Wrap it with UnifiedCachingFilesystem for caching
        let unified_fs = crate::storage::persistence::filesystem::caching_filesystem::UnifiedCachingFilesystem::with_serializer(
            fs,
            collection_id.to_string(),
            engine_type.to_string(),
            metadata_serializer,
        );

        Ok(Arc::new(unified_fs))
    }

    /// Cross-storage atomic operations - handles full URLs for source and destination
    pub async fn copy_atomic(&self, from_url: &str, to_url: &str) -> FsResult<()> {
        trace!("📋 [] copy_atomic START");
        trace!("from_url: {}", from_url);
        trace!("to_url: {}", to_url);
        debug!("📋 [DEBUG] copy_atomic START");
        debug!("    from_url: {}", from_url);
        debug!("    to_url: {}", to_url);

        let from_fs = self.get_filesystem(from_url)?;
        let to_fs = self.get_filesystem(to_url)?;

        // Extract paths from URLs
        let from_path = Self::resolve_path(from_url)?;
        let to_path = Self::resolve_path(to_url)?;

        trace!("from_path: {}", from_path);
        trace!("to_path: {}", to_path);
        debug!("    [DEBUG] from_path resolved: {}", from_path);
        debug!("    [DEBUG] to_path resolved: {}", to_path);

        if from_fs.filesystem_type() == "encrypted" || to_fs.filesystem_type() == "encrypted" {
            let data = from_fs.read(&from_path).await?;
            to_fs.write_atomic(&to_path, &data, None).await?;
            trace!("Whole-file copy complete");
            debug!("    ✅ [DEBUG] Whole-file copy complete");
            trace!("📋 [] copy_atomic COMPLETE");
            debug!("📋 [DEBUG] copy_atomic COMPLETE");
            return Ok(());
        }

        // Open source and destination files for streaming
        trace!("Opening source file for streaming...");
        let mut source_file = from_fs.open_file(&from_path, false).await?;
        trace!("Opening destination file for streaming...");
        let mut dest_file = to_fs.open_file(&to_path, true).await?;

        // Stream data in chunks
        let mut buffer = vec![0; 8 * 1024 * 1024]; // 8MB buffer
        loop {
            let bytes_read = source_file.read(&mut buffer).await?;
            if bytes_read == 0 {
                break;
            }
            dest_file.write(&buffer[..bytes_read]).await?;
        }

        // Flush and sync destination file
        dest_file.flush().await?;
        dest_file.sync_all().await?;

        trace!("Streaming copy complete");
        debug!("    ✅ [DEBUG] Streaming copy complete");

        trace!("📋 [] copy_atomic COMPLETE");
        debug!("📋 [DEBUG] copy_atomic COMPLETE");
        Ok(())
    }

    /// Move operation with atomic cross-storage support
    pub async fn move_atomic(&self, from_url: &str, to_url: &str) -> FsResult<()> {
        trace!("🚚 [] move_atomic START");
        trace!("from_url: {}", from_url);
        trace!("to_url: {}", to_url);
        debug!("🚚 [DEBUG] move_atomic called:");
        debug!("    from_url: {}", from_url);
        debug!("    to_url: {}", to_url);

        // Copy first using streaming copy
        trace!("Copying file atomically (streaming)...");
        debug!("    📋 [DEBUG] Copying file atomically (streaming)...");
        self.copy_atomic(from_url, to_url).await?;
        debug!("Streaming copy successful");
        debug!("    ✅ [DEBUG] Streaming copy successful");

        // Delete source after successful copy
        trace!("Deleting source file...");
        debug!("    🗑️ [DEBUG] Deleting source file...");
        let from_fs = self.get_filesystem(from_url)?;
        let from_path = Self::resolve_path(from_url)?;
        trace!("from_path extracted: {}", from_path);
        debug!("    [DEBUG] from_path extracted: {}", from_path);
        from_fs.delete(&from_path).await?;
        trace!("Delete successful");
        debug!("    ✅ [DEBUG] Delete successful");

        trace!("🚚 [] move_atomic COMPLETE");
        debug!("🚚 [DEBUG] move_atomic COMPLETE");
        Ok(())
    }

    /// Extract bucket/container name from URL
    pub fn extract_bucket_from_url(&self, url: &str) -> FsResult<Option<String>> {
        // Handle URLs without schemes by prepending file://
        let normalized_url = if !url.contains("://") {
            format!("file://{}", url)
        } else {
            url.to_string()
        };

        // For file:// URLs, there's no bucket
        if normalized_url.starts_with("file://") {
            return Ok(None);
        }

        let parsed_url = Url::parse(&normalized_url)?;

        match parsed_url.scheme() {
            "s3" | "gcs" | "gs" => {
                // Bucket is the hostname
                Ok(parsed_url.host_str().map(|s| s.to_string()))
            }
            "adls" => {
                // Container is the second path segment
                let path_parts: Vec<&str> = parsed_url
                    .path()
                    .trim_start_matches('/')
                    .split('/')
                    .collect();
                if path_parts.len() >= 2 {
                    Ok(Some(path_parts[1].to_string()))
                } else {
                    Ok(None)
                }
            }
            "abfs" => {
                // Container is before @ in hostname
                if let Some(host) = parsed_url.host_str()
                    && let Some(at_pos) = host.find('@')
                {
                    return Ok(Some(host[..at_pos].to_string()));
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Extract account name from URL (for Azure)
    pub fn extract_account_from_url(&self, url: &str) -> FsResult<Option<String>> {
        // Handle URLs without schemes by prepending file://
        let normalized_url = if !url.contains("://") {
            format!("file://{}", url)
        } else {
            url.to_string()
        };

        // For file:// URLs, there's no account
        if normalized_url.starts_with("file://") {
            return Ok(None);
        }

        let parsed_url = Url::parse(&normalized_url)?;

        match parsed_url.scheme() {
            "adls" => {
                // Account is the first path segment
                let path_parts: Vec<&str> = parsed_url
                    .path()
                    .trim_start_matches('/')
                    .split('/')
                    .collect();
                if !path_parts.is_empty() && !path_parts[0].is_empty() {
                    Ok(Some(path_parts[0].to_string()))
                } else {
                    Ok(None)
                }
            }
            "abfs" => {
                // Account is after @ in hostname
                if let Some(host) = parsed_url.host_str()
                    && let Some(at_pos) = host.find('@')
                {
                    return Ok(Some(host[at_pos + 1..].to_string()));
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Extract relative path from URL (removes base path configured for the storage)
    pub fn extract_relative_path(&self, url: &str) -> FsResult<String> {
        info!("🔍 resolve_path: {}", url);

        // Handle URLs without schemes by prepending file://
        let normalized_url = if !url.contains("://") {
            format!("file://{}", url)
        } else {
            url.to_string()
        };

        // Log the normalized URL for debugging
        info!("    normalized_url: {}", normalized_url);

        // Handle file:// URLs specially to avoid URL parsing issues
        if let Some(after_file) = normalized_url.strip_prefix("file://") {
            let path = if let Some(after_slashes) = normalized_url.strip_prefix("file:///") {
                // Absolute path
                format!("/{}", after_slashes)
            } else {
                // Relative or other path
                after_file.to_string()
            };
            info!("    scheme: file, returning path: {}", path);
            return Ok(path);
        }

        // For non-file URLs, use URL parsing
        let parsed_url = match Url::parse(&normalized_url) {
            Ok(url) => url,
            Err(e) => {
                error!("Failed to parse URL '{}': {}", normalized_url, e);
                return Err(FilesystemError::UrlParse(e));
            }
        };
        let path = parsed_url.path();
        info!("    parsed path: {}", path);

        match parsed_url.scheme() {
            "s3" | "gcs" | "gs" => {
                // For object stores, remove the bucket from path
                let path_without_bucket = path.trim_start_matches('/');

                // Skip the bucket name (first path segment)
                if let Some(slash_pos) = path_without_bucket.find('/') {
                    Ok(path_without_bucket[slash_pos + 1..].to_string())
                } else {
                    // No path after bucket, return empty string
                    Ok(String::new())
                }
            }
            "adls" | "abfs" => {
                // For Azure, remove account/container from path
                let path_parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
                if path_parts.len() > 1 {
                    Ok(path_parts[1..].join("/"))
                } else {
                    Ok(String::new())
                }
            }
            "hdfs" => {
                // For HDFS, return the full path
                Ok(path.to_string())
            }
            _ => {
                // For unknown schemes, return path as-is
                Ok(path.to_string())
            }
        }
    }

    /// Extract scheme from URL, handling paths without schemes
    fn extract_scheme(&self, url: &str) -> FsResult<String> {
        debug!("extract_scheme called with URL: {}", url);
        if let Some(scheme_end) = url.find("://") {
            // Extract just the scheme part without parsing the full URL
            let raw_scheme = &url[..scheme_end];
            debug!("extracted scheme: {}", raw_scheme);

            // Check for scheme mapping (e.g., gs -> gcs)
            let mapped_scheme = self.config.scheme_mapping.get(raw_scheme);

            let result = mapped_scheme
                .cloned()
                .unwrap_or_else(|| raw_scheme.to_string());
            debug!("returning scheme: {}", result);
            Ok(result)
        } else {
            // No scheme present - assume local file
            debug!("no scheme found, returning 'file'");
            Ok("file".to_string())
        }
    }

    /// Centralized URL path extraction utility (handles relative paths correctly)
    /// This method should be used throughout the filesystem layer for consistent URL parsing
    /// Unified path extraction from URLs with consistent behavior
    /// This is the SINGLE method that should be used throughout the filesystem layer
    pub fn resolve_path(url: &str) -> FsResult<String> {
        trace!("🔍 [FILESYSTEM] resolve_path: Input URL = '{}'", url);

        // Case 1: No scheme present - return as-is (this preserves relative paths)
        if !url.contains("://") {
            trace!(
                "🔍 [FILESYSTEM] resolve_path: No scheme, returning as-is: '{}'",
                url
            );
            return Ok(url.to_string());
        }

        // Case 2: Handle file:// URLs specially to avoid URL parsing issues
        if let Some(after_file) = url.strip_prefix("file://") {
            // CRITICAL: Avoid URL parsing for file:// to prevent international domain name errors
            // The URL parser can fail on paths that look like domain names
            if url.starts_with("file://./") {
                // Explicit relative path: file://./path/to/file
                let relative_path = after_file; // Keep the "./"
                trace!(
                    "🔍 [FILESYSTEM] resolve_path: Explicit relative path: '{}'",
                    relative_path
                );
                Ok(relative_path.to_string())
            } else if let Some(absolute_path) = url.strip_prefix("file:///") {
                // Absolute path: file:///absolute/path
                // Remove "file:///" prefix
                trace!(
                    "🔍 [FILESYSTEM] resolve_path: Absolute path: '/{}'",
                    absolute_path
                );
                Ok(format!("/{}", absolute_path))
            } else {
                // Implicit relative path: file://relative/path (treat as relative)
                trace!(
                    "🔍 [FILESYSTEM] resolve_path: Implicit relative path: '{}'",
                    after_file
                );
                Ok(after_file.to_string())
            }
        } else {
            // Case 3: Non-file schemes still need URL parsing (s3://, azure://, etc.)
            let parsed_url = Url::parse(url)
                .map_err(|e| FilesystemError::InvalidPath(format!("Invalid URL: {}", e)))?;
            let path = parsed_url.path();
            trace!(
                "🔍 [FILESYSTEM] resolve_path: Non-file scheme path: '{}'",
                path
            );
            Ok(path.to_string())
        }
    }

    /// Initialize tier-to-URL mapping from configuration
    fn initialize_tier_mapping(&mut self) {
        for tier_config in &self.config.performance_config.tier_configs {
            self.tier_mapping
                .insert(tier_config.tier, tier_config.base_url.clone());
        }

        // Add default mappings if not configured
        self.tier_mapping
            .entry(FileStorageTier::Memory)
            .or_insert_with(|| "memory://".to_string());
    }

    /// Get filesystem URL for a specific storage tier
    pub fn get_tier_url(&self, tier: FileStorageTier, relative_path: &str) -> FsResult<String> {
        let base_url = self.tier_mapping.get(&tier).ok_or_else(|| {
            FilesystemError::Config(format!("No filesystem configured for tier {:?}", tier))
        })?;

        // Construct full URL
        if base_url.ends_with('/') {
            Ok(format!("{}{}", base_url, relative_path))
        } else {
            Ok(format!("{}/{}", base_url, relative_path))
        }
    }

    /// Promote data from one tier to another
    pub async fn promote_data(
        &self,
        from_tier: FileStorageTier,
        to_tier: FileStorageTier,
        relative_path: &str,
    ) -> FsResult<()> {
        if !to_tier.is_faster_than(&from_tier) {
            return Err(FilesystemError::InvalidOperation(format!(
                "Cannot promote from {:?} to {:?} (target not faster)",
                from_tier, to_tier
            )));
        }

        let from_url = self.get_tier_url(from_tier, relative_path)?;
        let to_url = self.get_tier_url(to_tier, relative_path)?;

        info!(
            "Promoting data from {:?} to {:?}: {}",
            from_tier, to_tier, relative_path
        );
        self.move_atomic(&from_url, &to_url).await
    }

    /// Demote data from one tier to another
    pub async fn demote_data(
        &self,
        from_tier: FileStorageTier,
        to_tier: FileStorageTier,
        relative_path: &str,
    ) -> FsResult<()> {
        if from_tier.is_faster_than(&to_tier) {
            let from_url = self.get_tier_url(from_tier, relative_path)?;
            let to_url = self.get_tier_url(to_tier, relative_path)?;

            info!(
                "Demoting data from {:?} to {:?}: {}",
                from_tier, to_tier, relative_path
            );
            self.move_atomic(&from_url, &to_url).await
        } else {
            Err(FilesystemError::InvalidOperation(format!(
                "Cannot demote from {:?} to {:?} (target not slower)",
                from_tier, to_tier
            )))
        }
    }

    /// Get optimal tier for data based on access patterns
    pub fn suggest_tier(&self, access_frequency: f64, data_size_bytes: u64) -> FileStorageTier {
        // Simple heuristic: hot data → fast tiers, cold data → slow tiers
        if access_frequency > 100.0 {
            // Very hot: >100 accesses per hour
            if data_size_bytes < 100 * 1024 * 1024 {
                FileStorageTier::Memory
            } else {
                FileStorageTier::NVMe
            }
        } else if access_frequency > 10.0 {
            // Warm: 10-100 accesses per hour
            FileStorageTier::SSD
        } else if access_frequency > 1.0 {
            // Cool: 1-10 accesses per hour
            FileStorageTier::HDD
        } else {
            // Cold: <1 access per hour
            FileStorageTier::S3Standard
        }
    }

    /// List all available filesystem types
    pub fn available_filesystems(&self) -> Vec<&str> {
        self.filesystems.keys().map(|s| s.as_str()).collect()
    }

    /// Create a zero-copy filesystem wrapper for intelligent caching and optimization
    // create_zero_copy_filesystem removed - functionality integrated into get_unified_caching_filesystem
    /// Unified filesystem operations - automatically route to correct backend
    pub async fn read(&self, url: &str) -> FsResult<Vec<u8>> {
        tracing::debug!("🔍 FilesystemFactory::read() - URL: {}", url);
        let fs = self.get_filesystem(url)?;
        let path = Self::resolve_path(url)?;
        tracing::debug!(
            "📖 Routing to {} filesystem for path: {}",
            fs.filesystem_type(),
            path
        );
        let result = fs.read(&path).await;

        match &result {
            Ok(data) => tracing::debug!("✅ Read {} bytes successfully from {}", data.len(), url),
            Err(e) => tracing::error!("❌ Read failed from {}: {}", url, e),
        }

        result
    }

    pub async fn write(
        &self,
        url: &str,
        data: &[u8],
        options: Option<FileOptions>,
    ) -> FsResult<()> {
        tracing::debug!(
            "📝 FilesystemFactory::write() - URL: {} ({} bytes)",
            url,
            data.len()
        );
        let fs = self.get_filesystem(url)?;
        let path = Self::resolve_path(url)?;
        tracing::debug!(
            "💾 Routing to {} filesystem for path: {}",
            fs.filesystem_type(),
            path
        );
        let result = fs.write(&path, data, options).await;

        match &result {
            Ok(_) => tracing::debug!("✅ Wrote {} bytes successfully to {}", data.len(), url),
            Err(e) => tracing::error!("❌ Write failed to {}: {}", url, e),
        }

        result
    }

    pub async fn append(&self, url: &str, data: &[u8]) -> FsResult<()> {
        tracing::debug!(
            "➕ FilesystemFactory::append() - URL: {} ({} bytes)",
            url,
            data.len()
        );
        let fs = self.get_filesystem(url)?;
        let path = Self::resolve_path(url)?;
        tracing::debug!(
            "📎 Routing to {} filesystem for path: {}",
            fs.filesystem_type(),
            path
        );
        let result = fs.append(&path, data).await;

        match &result {
            Ok(_) => tracing::debug!("✅ Appended {} bytes successfully to {}", data.len(), url),
            Err(e) => tracing::error!("❌ Append failed to {}: {}", url, e),
        }

        result
    }

    pub async fn delete(&self, url: &str) -> FsResult<()> {
        tracing::debug!("🗑️ FilesystemFactory::delete() - URL: {}", url);
        let fs = self.get_filesystem(url)?;
        let path = Self::resolve_path(url)?;
        tracing::debug!(
            "🚮 Routing to {} filesystem for path: {}",
            fs.filesystem_type(),
            path
        );
        let result = fs.delete(&path).await;

        match &result {
            Ok(_) => tracing::debug!("✅ Deleted successfully: {}", url),
            Err(e) => tracing::error!("❌ Delete failed for {}: {}", url, e),
        }

        result
    }

    pub async fn exists(&self, url: &str) -> FsResult<bool> {
        tracing::trace!("🔍 FilesystemFactory::exists() - URL: {}", url);
        let fs = self.get_filesystem(url)?;
        let path = Self::resolve_path(url)?;
        let result = fs.exists(&path).await;

        match &result {
            Ok(exists) => tracing::trace!("✅ Exists check for {}: {}", url, exists),
            Err(e) => tracing::error!("❌ Exists check failed for {}: {}", url, e),
        }

        result
    }

    pub async fn metadata(&self, url: &str) -> FsResult<FileMetadata> {
        let fs = self.get_filesystem(url)?;
        let path = Self::resolve_path(url)?;
        fs.metadata(&path).await
    }

    pub async fn list(&self, url: &str) -> FsResult<Vec<DirEntry>> {
        let fs = self.get_filesystem(url)?;
        let path = Self::resolve_path(url)?;
        fs.list(&path).await
    }

    pub async fn create_dir(&self, url: &str) -> FsResult<()> {
        let fs = self.get_filesystem(url)?;
        let path = Self::resolve_path(url)?;
        fs.create_dir(&path).await
    }

    pub async fn create_dir_all(&self, url: &str) -> FsResult<()> {
        let fs = self.get_filesystem(url)?;
        let path = Self::resolve_path(url)?;
        fs.create_dir_all(&path).await
    }

    pub async fn copy(&self, from_url: &str, to_url: &str) -> FsResult<()> {
        // Handle cross-filesystem copies
        let from_scheme = self.extract_scheme(from_url)?;
        let to_scheme = self.extract_scheme(to_url)?;

        if from_scheme == to_scheme {
            // Same filesystem - use native copy
            let fs = self.get_filesystem(from_url)?;
            let from_path = Self::resolve_path(from_url)?;
            let to_path = Self::resolve_path(to_url)?;
            fs.copy(&from_path, &to_path).await
        } else {
            // Cross-filesystem copy - read from source, write to destination
            let data = self.read(from_url).await?;
            self.write(to_url, &data, None).await
        }
    }

    pub async fn move_file(&self, from_url: &str, to_url: &str) -> FsResult<()> {
        // Handle cross-filesystem moves
        let from_scheme = self.extract_scheme(from_url)?;
        let to_scheme = self.extract_scheme(to_url)?;

        if from_scheme == to_scheme {
            // Same filesystem - use native move
            let fs = self.get_filesystem(from_url)?;
            let from_path = Self::resolve_path(from_url)?;
            let to_path = Self::resolve_path(to_url)?;
            fs.move_file(&from_path, &to_path).await
        } else {
            // Cross-filesystem move - copy then delete
            self.copy(from_url, to_url).await?;
            self.delete(from_url).await
        }
    }

    pub async fn sync(&self, url: &str) -> FsResult<()> {
        let fs = self.get_filesystem(url)?;
        fs.sync().await
    }

    /// Create an Arc-wrapped filesystem factory (safe helper)
    ///
    /// This is a convenience helper for creating Arc<FilesystemFactory> with proper error handling.
    /// Use this to avoid panic-prone call sites when creating filesystem factories.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use anyhow::Context;
    ///
    /// // Before (panics on error):
    /// let factory = Arc::new(FilesystemFactory::create(config).await?);
    ///
    /// // After (proper error handling):
    /// let factory = FilesystemFactory::create_arc(config).await
    ///     .context("Failed to create filesystem factory")?;
    /// ```
    pub async fn create_arc(config: FilesystemConfig) -> FsResult<Arc<Self>> {
        let factory = Self::create(config).await?;
        Ok(Arc::new(factory))
    }

    /// Create an Arc-wrapped filesystem factory with default config (safe helper)
    ///
    /// This is a convenience helper for creating Arc<FilesystemFactory> with proper error handling.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use anyhow::Context;
    ///
    /// // Before (panics on error):
    /// let factory = Arc::new(FilesystemFactory::create_default().await?);
    ///
    /// // After (proper error handling):
    /// let factory = FilesystemFactory::create_default_arc().await
    ///     .context("Failed to create filesystem factory")?;
    /// ```
    pub async fn create_default_arc() -> FsResult<Arc<Self>> {
        let factory = Self::create_default().await?;
        Ok(Arc::new(factory))
    }
}

#[cfg(test)]
mod inline_tests {
    use super::*;
    use std::ffi::OsString;
    use tempfile::TempDir;

    struct EnvVarGuard {
        key: String,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self {
                key: key.to_string(),
                previous,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(previous) = &self.previous {
                    std::env::set_var(&self.key, previous);
                } else {
                    std::env::remove_var(&self.key);
                }
            }
        }
    }

    #[tokio::test]
    async fn test_filesystem_factory_creation() {
        let config = FilesystemConfig::default();
        let factory = FilesystemFactory::create(config)
            .await
            .expect("Failed to create filesystem factory with default config");

        // Should have local filesystem by default
        assert!(factory.available_filesystems().contains(&"file"));
    }

    #[tokio::test]
    async fn test_url_scheme_extraction() {
        let config = FilesystemConfig::default();
        let factory = FilesystemFactory::create(config)
            .await
            .expect("Failed to create filesystem factory for URL scheme extraction test");

        assert_eq!(
            factory
                .extract_scheme("file:///tmp/test.txt")
                .expect("Failed to extract scheme from file:// URL"),
            "file"
        );
        assert_eq!(
            factory
                .extract_scheme("s3://bucket/key")
                .expect("Failed to extract scheme from s3:// URL"),
            "s3"
        );
        assert_eq!(
            factory
                .extract_scheme("adls://account/container/path")
                .expect("Failed to extract scheme from adls:// URL"),
            "adls"
        );
        assert_eq!(
            factory
                .extract_scheme("abfs://container@account/path")
                .expect("Failed to extract scheme from abfs:// URL"),
            "abfs"
        );
        assert_eq!(
            factory
                .extract_scheme("gcs://bucket/object")
                .expect("Failed to extract scheme from gcs:// URL"),
            "gcs"
        );
        // Test gs:// scheme mapping to gcs
        assert_eq!(
            factory
                .extract_scheme("gs://bucket/object")
                .expect("Failed to extract scheme from gs:// URL"),
            "gcs"
        );
        assert_eq!(
            factory
                .extract_scheme("hdfs://namenode:9000/path")
                .expect("Failed to extract scheme from hdfs:// URL"),
            "hdfs"
        );
    }

    #[tokio::test]
    async fn test_path_extraction() {
        let _config = FilesystemConfig::default();
        let _factory = FilesystemFactory::create(_config)
            .await
            .expect("Failed to create filesystem factory for path extraction test");

        assert_eq!(
            FilesystemFactory::resolve_path("file:///tmp/test.txt")
                .expect("Failed to resolve path from file:// URL"),
            "/tmp/test.txt"
        );
        assert_eq!(
            FilesystemFactory::resolve_path("s3://bucket/key")
                .expect("Failed to resolve path from s3:// URL"),
            "/key"
        );
        assert_eq!(
            FilesystemFactory::resolve_path("/local/path").expect("Failed to resolve local path"),
            "/local/path"
        );
    }

    #[tokio::test]
    async fn test_filesystem_factory_wraps_local_fs_with_encryption() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let _env = EnvVarGuard::set(
            "TEST_PROXIMADB_FS_FACTORY_KEY",
            "factory-test-master-key-32-bytes!!",
        );

        let config = FilesystemConfig {
            local: Some(local::LocalConfig {
                root_dir: Some(temp_dir.path().to_path_buf()),
                ..Default::default()
            }),
            global_options: FileOptions {
                encryption: Some("TEST_PROXIMADB_FS_FACTORY_KEY".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let factory = FilesystemFactory::create(config)
            .await
            .expect("failed to create encrypted filesystem factory");
        let fs = factory
            .get_filesystem("file:///tmp/proximadb-encrypted")
            .expect("failed to get encrypted filesystem");

        assert_eq!(fs.filesystem_type(), "encrypted");

        let logical_path = temp_dir.path().join("wrapped.bin");
        let logical_path = logical_path.to_string_lossy().to_string();
        let encrypted_path = format!("{}.enc", logical_path);

        fs.write(&logical_path, b"factory-secret", None)
            .await
            .expect("failed to write encrypted file");

        assert!(std::path::Path::new(&encrypted_path).exists());
        assert_eq!(
            fs.read(&logical_path)
                .await
                .expect("failed to read encrypted file"),
            b"factory-secret"
        );
    }

    #[tokio::test]
    async fn test_copy_atomic_falls_back_for_encrypted_filesystems() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let _env = EnvVarGuard::set(
            "TEST_PROXIMADB_FS_COPY_KEY",
            "copy-test-master-key-32-bytes!!!!!",
        );

        let config = FilesystemConfig {
            local: Some(local::LocalConfig {
                root_dir: Some(temp_dir.path().to_path_buf()),
                ..Default::default()
            }),
            global_options: FileOptions {
                encryption: Some("TEST_PROXIMADB_FS_COPY_KEY".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let factory = FilesystemFactory::create(config)
            .await
            .expect("failed to create encrypted filesystem factory");
        let fs = factory
            .get_filesystem("file:///tmp/proximadb-encrypted")
            .expect("failed to get encrypted filesystem");

        let src_url = format!("file://{}", temp_dir.path().join("src.bin").display());
        let dst_url = format!("file://{}", temp_dir.path().join("dst.bin").display());
        let src_path = FilesystemFactory::resolve_path(&src_url).expect("failed to resolve src");
        let dst_path = FilesystemFactory::resolve_path(&dst_url).expect("failed to resolve dst");

        fs.write(&src_path, b"copy-secret", None)
            .await
            .expect("failed to write source file");

        factory
            .copy_atomic(&src_url, &dst_url)
            .await
            .expect("copy_atomic should succeed for encrypted filesystems");

        assert_eq!(
            fs.read(&dst_path)
                .await
                .expect("failed to read copied file"),
            b"copy-secret"
        );
        assert!(std::path::Path::new(&format!("{}.enc", dst_path)).exists());
    }
}

#[cfg(test)]
mod comprehensive_tests;
