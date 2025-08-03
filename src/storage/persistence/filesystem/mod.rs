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

//! Filesystem Abstraction Layer with Abstract Factory Pattern
//!
//! Provides a unified filesystem interface supporting multiple storage backends:
//! - file:// - Local filesystem (Windows, Linux, etc.)
//! - s3://   - Amazon S3 (with IAM roles, STS temp credentials)
//! - adls:// - Azure Data Lake Storage (with managed identity, SAS tokens)
//! - gcs://  - Google Cloud Storage (with service accounts, ADC)
//!
//! Uses Strategy Pattern for backend implementations with automatic URL-based routing.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Error as IoError;
use url::Url;
use tracing::{info, error};

pub mod atomic_strategy;
pub mod auth;
pub mod azure;
pub mod gcs;
pub mod hdfs;
pub mod local;
pub mod manager;
pub mod s3;
pub mod write_strategy;

#[cfg(test)]
pub mod tests;

use azure::AzureFileSystem;
use gcs::GcsFileSystem;
use hdfs::HdfsFileSystem;
use local::LocalFileSystem;
use s3::S3FileSystem;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub url: String,  // Full URL instead of relative path
    pub metadata: FileMetadata,
}

/// Temporary directory strategy for atomic operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TempStrategy {
    /// Direct write (no temp files) - for local filesystem with atomic guarantees
    DirectWrite,

    /// Write to ___temp subdirectory in same location (same mount point)
    /// Ensures move operations are filesystem renames, not copies
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

impl Default for TempStrategy {
    fn default() -> Self {
        // Default to same directory strategy for optimal performance
        TempStrategy::SameDirectory
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Filesystem performance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemPerformanceConfig {
    /// Connection pool size per backend
    pub connection_pool_size: usize,

    /// Enable connection keep-alive
    pub enable_keep_alive: bool,

    /// Request timeout (seconds)
    pub request_timeout_seconds: u64,

    /// Enable compression for network transfers
    pub enable_compression: bool,

    /// Retry configuration
    pub retry_config: RetryConfig,

    /// Buffer size for operations (bytes)
    pub buffer_size: usize,

    /// Enable parallel operations
    pub enable_parallel_ops: bool,

    /// Maximum concurrent operations
    pub max_concurrent_ops: usize,
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
    /// Read file contents
    async fn read(&self, path: &str) -> FsResult<Vec<u8>>;

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
    async fn read_ranges(&self, path: &str, ranges: Vec<std::ops::Range<u64>>) -> FsResult<Vec<Vec<u8>>> {
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
    async fn sync_file(&self, path: &str) -> FsResult<()> {
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
                let parent = final_path.parent().unwrap_or(Path::new("."));
                let temp_dir = parent.join("___temp");
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
        let opts = options.unwrap_or_default();

        match &opts.temp_path {
            None => {
                // Direct write (optimal for local filesystem)
                self.write(final_path, data, Some(opts)).await
            }
            Some(temp_path_str) => {
                // Atomic write-temp-rename (optimal for object stores)
                let temp_path = std::path::Path::new(temp_path_str);

                // Ensure temp directory exists
                if let Some(temp_parent) = temp_path.parent() {
                    self.create_dir_all(&temp_parent.to_string_lossy()).await?;
                }

                // Write to temp location
                let temp_opts = FileOptions {
                    temp_path: None, // Prevent recursion
                    ..opts.clone()
                };
                self.write(temp_path_str, data, Some(temp_opts)).await?;

                // Atomic move (rename on same mount point)
                self.move_file(temp_path_str, final_path).await
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
    async fn write_string(&self, path: &str, content: &str, options: Option<FileOptions>) -> FsResult<()> {
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemConfig {
    /// Default filesystem URL for unqualified paths
    pub default_fs: Option<String>,

    /// AWS S3 configuration
    pub s3: Option<s3::S3Config>,

    /// Azure Data Lake Storage configuration
    pub azure: Option<azure::AzureConfig>,

    /// Google Cloud Storage configuration
    pub gcs: Option<gcs::GcsConfig>,

    /// Local filesystem configuration
    pub local: Option<local::LocalConfig>,

    /// HDFS configuration
    pub hdfs: Option<hdfs::HdfsConfig>,

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
            enable_compression: true,
            retry_config: RetryConfig {
                max_retries: 3,
                initial_delay_ms: 100,
                max_delay_ms: 5000,
                backoff_multiplier: 2.0,
            },
            buffer_size: 8 * 1024 * 1024, // 8MB
            enable_parallel_ops: true,
            max_concurrent_ops: 100,
        }
    }
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            default_fs: Some("file://".to_string()),
            s3: None,
            azure: None,
            gcs: None,
            local: Some(local::LocalConfig::default()),
            hdfs: None,
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
    filesystems: HashMap<String, Box<dyn FileSystem>>,
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
    /// Create new filesystem factory with configuration
    pub async fn new(config: FilesystemConfig) -> FsResult<Self> {
        let mut factory = Self {
            config,
            filesystems: HashMap::new(),
        };

        // Pre-initialize configured filesystems
        factory.initialize_filesystems().await?;

        Ok(factory)
    }

    /// Initialize all configured filesystem backends
    async fn initialize_filesystems(&mut self) -> FsResult<()> {
        // Initialize local filesystem with root directory resolution
        if let Some(local_config) = &self.config.local {
            let local_fs = LocalFileSystem::new(local_config.clone()).await?;
            self.filesystems
                .insert("file".to_string(), Box::new(local_fs));
        } else {
            // Create default local filesystem without root restriction
            let default_config = local::LocalConfig::default();
            let local_fs = LocalFileSystem::new(default_config).await?;
            self.filesystems
                .insert("file".to_string(), Box::new(local_fs));
        }

        // Initialize S3 filesystem
        if let Some(s3_config) = &self.config.s3 {
            let s3_fs = S3FileSystem::new(s3_config.clone()).await?;
            self.filesystems.insert("s3".to_string(), Box::new(s3_fs));
        }

        // Initialize Azure filesystem
        if let Some(azure_config) = &self.config.azure {
            let azure_fs_adls = AzureFileSystem::new(azure_config.clone()).await?;
            let azure_fs_abfs = AzureFileSystem::new(azure_config.clone()).await?;
            self.filesystems
                .insert("adls".to_string(), Box::new(azure_fs_adls));
            // ABFS is the same as ADLS Gen2 - just different URL scheme
            self.filesystems
                .insert("abfs".to_string(), Box::new(azure_fs_abfs));
        }

        // Initialize GCS filesystem
        if let Some(gcs_config) = &self.config.gcs {
            // Create two instances for both schemes
            let gcs_fs1 = GcsFileSystem::new(gcs_config.clone()).await?;
            let gcs_fs2 = GcsFileSystem::new(gcs_config.clone()).await?;
            // Register under both "gcs" and "gs" schemes for compatibility
            self.filesystems.insert("gcs".to_string(), Box::new(gcs_fs1));
            self.filesystems.insert("gs".to_string(), Box::new(gcs_fs2));
        }

        // Initialize HDFS filesystem
        if let Some(hdfs_config) = &self.config.hdfs {
            let hdfs_fs = HdfsFileSystem::new(hdfs_config.clone()).await?;
            self.filesystems
                .insert("hdfs".to_string(), Box::new(hdfs_fs));
        }

        Ok(())
    }

    /// Get filesystem instance for URL scheme (cached instances)
    pub fn get_filesystem(&self, url: &str) -> FsResult<&dyn FileSystem> {
        // Handle URLs without schemes by prepending file://
        let normalized_url = if !url.contains("://") {
            format!("file://{}", url)
        } else {
            url.to_string()
        };
        
        let scheme = self.extract_scheme(&normalized_url)?;

        self.filesystems
            .get(&scheme)
            .map(|fs| fs.as_ref())
            .ok_or_else(|| FilesystemError::UnsupportedScheme(scheme))
    }

    /// Cross-storage atomic operations - handles full URLs for source and destination
    pub async fn copy_atomic(&self, from_url: &str, to_url: &str) -> FsResult<()> {
        info!("📋 copy_atomic START");
        info!("    from_url: {}", from_url);
        info!("    to_url: {}", to_url);
        
        let from_fs = self.get_filesystem(from_url)?;
        let to_fs = self.get_filesystem(to_url)?;

        // Extract paths from URLs
        let from_path = self.extract_path_from_url(from_url)?;
        let to_path = self.extract_path_from_url(to_url)?;
        
        info!("    from_path: {}", from_path);
        info!("    to_path: {}", to_path);

        // Read from source
        info!("    📖 Reading source file...");
        let data = from_fs.read(&from_path).await?;
        info!("    ✅ Read {} bytes", data.len());

        // Write to destination atomically
        info!("    💾 Writing to destination atomically...");
        to_fs.write_atomic(&to_path, &data, None).await?;
        info!("    ✅ Write complete");

        info!("📋 copy_atomic COMPLETE");
        Ok(())
    }

    /// Move operation with atomic cross-storage support
    pub async fn move_atomic(&self, from_url: &str, to_url: &str) -> FsResult<()> {
        info!("🚚 move_atomic START");
        info!("    from_url: {}", from_url);
        info!("    to_url: {}", to_url);
        
        // Copy first
        info!("    📋 Copying file atomically...");
        self.copy_atomic(from_url, to_url).await?;
        info!("    ✅ Copy successful");

        // Delete source after successful copy
        info!("    🗑️ Deleting source file...");
        let from_fs = self.get_filesystem(from_url)?;
        let from_path = self.extract_path_from_url(from_url)?;
        info!("    from_path extracted: {}", from_path);
        from_fs.delete(&from_path).await?;
        info!("    ✅ Delete successful");

        info!("🚚 move_atomic COMPLETE");
        Ok(())
    }

    /// Validate URL format for supported cloud providers
    pub fn validate_url(&self, url: &str) -> FsResult<()> {
        // Handle URLs without schemes by prepending file://
        let normalized_url = if !url.contains("://") {
            format!("file://{}", url)
        } else {
            url.to_string()
        };
        
        let parsed_url = Url::parse(&normalized_url)?;
        
        match parsed_url.scheme() {
            "file" => {
                // File URLs must have absolute paths
                if !parsed_url.path().starts_with('/') {
                    return Err(FilesystemError::InvalidPath(
                        "File URLs must have absolute paths".to_string()
                    ));
                }
            }
            "s3" => {
                // S3 URLs must have bucket name
                if parsed_url.host_str().is_none() || parsed_url.host_str().unwrap().is_empty() {
                    return Err(FilesystemError::InvalidPath(
                        "S3 URLs must specify bucket name".to_string()
                    ));
                }
            }
            "gs" => {
                // GCS URLs must have bucket name
                if parsed_url.host_str().is_none() || parsed_url.host_str().unwrap().is_empty() {
                    return Err(FilesystemError::InvalidPath(
                        "GCS URLs must specify bucket name".to_string()
                    ));
                }
            }
            "adls" => {
                // ADLS URLs must have account and container
                let path_parts: Vec<&str> = parsed_url.path().trim_start_matches('/').split('/').collect();
                if path_parts.len() < 2 || path_parts[0].is_empty() || path_parts[1].is_empty() {
                    return Err(FilesystemError::InvalidPath(
                        "ADLS URLs must specify account and container".to_string()
                    ));
                }
            }
            "abfs" => {
                // ABFS URLs must have container@account format
                if parsed_url.host_str().is_none() || !parsed_url.host_str().unwrap().contains('@') {
                    return Err(FilesystemError::InvalidPath(
                        "ABFS URLs must use container@account format".to_string()
                    ));
                }
            }
            "hdfs" => {
                // HDFS URLs must have namenode host
                if parsed_url.host_str().is_none() || parsed_url.host_str().unwrap().is_empty() {
                    return Err(FilesystemError::InvalidPath(
                        "HDFS URLs must specify namenode host".to_string()
                    ));
                }
            }
            _ => {
                return Err(FilesystemError::UnsupportedScheme(
                    parsed_url.scheme().to_string()
                ));
            }
        }
        
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
        
        let parsed_url = Url::parse(&normalized_url)?;
        
        match parsed_url.scheme() {
            "s3" | "gcs" | "gs" => {
                // Bucket is the hostname
                Ok(parsed_url.host_str().map(|s| s.to_string()))
            }
            "adls" => {
                // Container is the second path segment
                let path_parts: Vec<&str> = parsed_url.path().trim_start_matches('/').split('/').collect();
                if path_parts.len() >= 2 {
                    Ok(Some(path_parts[1].to_string()))
                } else {
                    Ok(None)
                }
            }
            "abfs" => {
                // Container is before @ in hostname
                if let Some(host) = parsed_url.host_str() {
                    if let Some(at_pos) = host.find('@') {
                        return Ok(Some(host[..at_pos].to_string()));
                    }
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
        
        let parsed_url = Url::parse(&normalized_url)?;
        
        match parsed_url.scheme() {
            "adls" => {
                // Account is the first path segment
                let path_parts: Vec<&str> = parsed_url.path().trim_start_matches('/').split('/').collect();
                if !path_parts.is_empty() && !path_parts[0].is_empty() {
                    Ok(Some(path_parts[0].to_string()))
                } else {
                    Ok(None)
                }
            }
            "abfs" => {
                // Account is after @ in hostname
                if let Some(host) = parsed_url.host_str() {
                    if let Some(at_pos) = host.find('@') {
                        return Ok(Some(host[at_pos + 1..].to_string()));
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Extract relative path from URL (removes base path configured for the storage)
    pub fn extract_path_from_url(&self, url: &str) -> FsResult<String> {
        info!("🔍 extract_path_from_url: {}", url);
        
        // Handle URLs without schemes by prepending file://
        let normalized_url = if !url.contains("://") {
            format!("file://{}", url)
        } else {
            url.to_string()
        };
        
        // Log the normalized URL for debugging
        info!("    normalized_url: {}", normalized_url);
        
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
            "file" => {
                // For file URLs, return the full absolute path
                info!("    scheme: file, returning path as-is");
                Ok(path.to_string())
            }
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
        if url.contains("://") {
            let parsed = Url::parse(url)?;
            let raw_scheme = parsed.scheme().to_string();
            
            // Check for scheme mapping (e.g., gs -> gcs)
            let mapped_scheme = self.config.scheme_mapping
                .get(&raw_scheme)
                .unwrap_or(&raw_scheme);
            
            Ok(mapped_scheme.clone())
        } else {
            // No scheme present - assume local file
            Ok("file".to_string())
        }
    }

    /// Centralized URL path extraction utility (handles relative paths correctly)
    /// This method should be used throughout the filesystem layer for consistent URL parsing
    pub fn extract_path_from_url_safe(url: &str) -> FsResult<String> {
        if !url.contains("://") {
            // No scheme present - return as-is
            return Ok(url.to_string());
        }
        
        let parsed_url = Url::parse(url)?;
        let path = parsed_url.path();
        
        // Handle relative paths that were converted to absolute by URL parser
        // The URL parser converts file://./relative/path to path="/relative/path"
        // We need to preserve the relative nature for local filesystem
        if parsed_url.scheme() == "file" && url.starts_with("file://./") {
            // Extract the relative part after "file://"
            let relative_path = &url[7..]; // Remove "file://" prefix
            Ok(relative_path.to_string())
        } else {
            Ok(path.to_string())
        }
    }

    /// Get the path component from URL (uses centralized utility)
    pub fn extract_path(&self, url: &str) -> FsResult<String> {
        Self::extract_path_from_url_safe(url)
    }

    /// List all available filesystem types
    pub fn available_filesystems(&self) -> Vec<&str> {
        self.filesystems.keys().map(|s| s.as_str()).collect()
    }

    /// Unified filesystem operations - automatically route to correct backend
    pub async fn read(&self, url: &str) -> FsResult<Vec<u8>> {
        tracing::debug!("🔍 FilesystemFactory::read() - URL: {}", url);
        let fs = self.get_filesystem(url)?;
        let path = self.extract_path(url)?;
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
        let path = self.extract_path(url)?;
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
        let path = self.extract_path(url)?;
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
        let path = self.extract_path(url)?;
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
        let path = self.extract_path(url)?;
        let result = fs.exists(&path).await;

        match &result {
            Ok(exists) => tracing::trace!("✅ Exists check for {}: {}", url, exists),
            Err(e) => tracing::error!("❌ Exists check failed for {}: {}", url, e),
        }

        result
    }

    pub async fn metadata(&self, url: &str) -> FsResult<FileMetadata> {
        let fs = self.get_filesystem(url)?;
        let path = self.extract_path(url)?;
        fs.metadata(&path).await
    }

    pub async fn list(&self, url: &str) -> FsResult<Vec<DirEntry>> {
        let fs = self.get_filesystem(url)?;
        let path = self.extract_path(url)?;
        fs.list(&path).await
    }

    pub async fn create_dir(&self, url: &str) -> FsResult<()> {
        let fs = self.get_filesystem(url)?;
        let path = self.extract_path(url)?;
        fs.create_dir(&path).await
    }

    pub async fn create_dir_all(&self, url: &str) -> FsResult<()> {
        let fs = self.get_filesystem(url)?;
        let path = self.extract_path(url)?;
        fs.create_dir_all(&path).await
    }

    pub async fn copy(&self, from_url: &str, to_url: &str) -> FsResult<()> {
        // Handle cross-filesystem copies
        let from_scheme = self.extract_scheme(from_url)?;
        let to_scheme = self.extract_scheme(to_url)?;

        if from_scheme == to_scheme {
            // Same filesystem - use native copy
            let fs = self.get_filesystem(from_url)?;
            let from_path = self.extract_path(from_url)?;
            let to_path = self.extract_path(to_url)?;
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
            let from_path = self.extract_path(from_url)?;
            let to_path = self.extract_path(to_url)?;
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
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[tokio::test]
    async fn test_filesystem_factory_creation() {
        let config = FilesystemConfig::default();
        let factory = FilesystemFactory::new(config).await.unwrap();

        // Should have local filesystem by default
        assert!(factory.available_filesystems().contains(&"file"));
    }

    #[tokio::test]
    async fn test_url_scheme_extraction() {
        let config = FilesystemConfig::default();
        let factory = FilesystemFactory::new(config).await.unwrap();

        assert_eq!(
            factory.extract_scheme("file:///tmp/test.txt").unwrap(),
            "file"
        );
        assert_eq!(factory.extract_scheme("s3://bucket/key").unwrap(), "s3");
        assert_eq!(
            factory
                .extract_scheme("adls://account/container/path")
                .unwrap(),
            "adls"
        );
        assert_eq!(
            factory
                .extract_scheme("abfs://container@account/path")
                .unwrap(),
            "abfs"
        );
        assert_eq!(
            factory.extract_scheme("gcs://bucket/object").unwrap(),
            "gcs"
        );
        // Test gs:// scheme mapping to gcs
        assert_eq!(
            factory.extract_scheme("gs://bucket/object").unwrap(),
            "gcs"
        );
        assert_eq!(
            factory.extract_scheme("hdfs://namenode:9000/path").unwrap(),
            "hdfs"
        );
    }

    #[tokio::test]
    async fn test_path_extraction() {
        let config = FilesystemConfig::default();
        let factory = FilesystemFactory::new(config).await.unwrap();

        assert_eq!(
            factory.extract_path("file:///tmp/test.txt").unwrap(),
            "/tmp/test.txt"
        );
        assert_eq!(factory.extract_path("s3://bucket/key").unwrap(), "/key");
        assert_eq!(factory.extract_path("/local/path").unwrap(), "/local/path");
    }
}

#[cfg(test)]
mod comprehensive_tests;
