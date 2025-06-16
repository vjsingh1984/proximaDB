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

use std::path::{Path, PathBuf};
use std::io::{Result as IoResult, Error as IoError, ErrorKind};
use std::collections::HashMap;
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use url::Url;
use serde::{Serialize, Deserialize};

pub mod local;
pub mod s3;
pub mod azure;
pub mod gcs;
pub mod hdfs;
pub mod auth;

use local::LocalFileSystem;
use s3::S3FileSystem;
use azure::AzureFileSystem;
use gcs::GcsFileSystem;
use hdfs::HdfsFileSystem;

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
    pub etag: Option<String>, // For cloud storage
    pub storage_class: Option<String>, // For cloud storage
}

/// Directory listing entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub metadata: FileMetadata,
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
    StsAssumeRole { role_arn: String, session_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AzureAuthMethod {
    /// Use Azure Managed Identity
    ManagedIdentity,
    /// Use Azure Service Principal
    ServicePrincipal { client_id: String, tenant_id: String },
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

/// Abstract filesystem trait for strategy pattern
#[async_trait]
pub trait FileSystem: Send + Sync {
    /// Read file contents
    async fn read(&self, path: &str) -> FsResult<Vec<u8>>;
    
    /// Write file contents
    async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()>;
    
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
    
    /// Sync/flush operations to storage
    async fn sync(&self) -> FsResult<()>;
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
        }
    }
}

/// Abstract factory for creating filesystem instances
pub struct FilesystemFactory {
    config: FilesystemConfig,
    filesystems: HashMap<String, Box<dyn FileSystem>>,
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
        // Initialize local filesystem
        if let Some(local_config) = &self.config.local {
            let local_fs = LocalFileSystem::new(local_config.clone()).await?;
            self.filesystems.insert("file".to_string(), Box::new(local_fs));
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
            self.filesystems.insert("adls".to_string(), Box::new(azure_fs_adls));
            // ABFS is the same as ADLS Gen2 - just different URL scheme
            self.filesystems.insert("abfs".to_string(), Box::new(azure_fs_abfs));
        }
        
        // Initialize GCS filesystem
        if let Some(gcs_config) = &self.config.gcs {
            let gcs_fs = GcsFileSystem::new(gcs_config.clone()).await?;
            self.filesystems.insert("gcs".to_string(), Box::new(gcs_fs));
        }
        
        // Initialize HDFS filesystem
        if let Some(hdfs_config) = &self.config.hdfs {
            let hdfs_fs = HdfsFileSystem::new(hdfs_config.clone()).await?;
            self.filesystems.insert("hdfs".to_string(), Box::new(hdfs_fs));
        }
        
        Ok(())
    }
    
    /// Get filesystem instance for URL scheme
    pub fn get_filesystem(&self, url: &str) -> FsResult<&dyn FileSystem> {
        let scheme = self.extract_scheme(url)?;
        
        self.filesystems.get(&scheme)
            .map(|fs| fs.as_ref())
            .ok_or_else(|| FilesystemError::UnsupportedScheme(scheme))
    }
    
    /// Extract scheme from URL
    fn extract_scheme(&self, url: &str) -> FsResult<String> {
        if url.contains("://") {
            let parsed = Url::parse(url)?;
            Ok(parsed.scheme().to_string())
        } else {
            // Use default filesystem for unqualified paths
            if let Some(default_fs) = &self.config.default_fs {
                let parsed = Url::parse(default_fs)?;
                Ok(parsed.scheme().to_string())
            } else {
                Ok("file".to_string()) // Default to local filesystem
            }
        }
    }
    
    /// Get the path component from URL
    pub fn extract_path(&self, url: &str) -> FsResult<String> {
        if url.contains("://") {
            let parsed = Url::parse(url)?;
            Ok(parsed.path().to_string())
        } else {
            // Treat as local path if no scheme
            Ok(url.to_string())
        }
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
        tracing::debug!("📖 Routing to {} filesystem for path: {}", fs.filesystem_type(), path);
        let result = fs.read(&path).await;
        
        match &result {
            Ok(data) => tracing::debug!("✅ Read {} bytes successfully from {}", data.len(), url),
            Err(e) => tracing::error!("❌ Read failed from {}: {}", url, e),
        }
        
        result
    }
    
    pub async fn write(&self, url: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()> {
        tracing::debug!("📝 FilesystemFactory::write() - URL: {} ({} bytes)", url, data.len());
        let fs = self.get_filesystem(url)?;
        let path = self.extract_path(url)?;
        tracing::debug!("💾 Routing to {} filesystem for path: {}", fs.filesystem_type(), path);
        let result = fs.write(&path, data, options).await;
        
        match &result {
            Ok(_) => tracing::debug!("✅ Wrote {} bytes successfully to {}", data.len(), url),
            Err(e) => tracing::error!("❌ Write failed to {}: {}", url, e),
        }
        
        result
    }
    
    pub async fn append(&self, url: &str, data: &[u8]) -> FsResult<()> {
        tracing::debug!("➕ FilesystemFactory::append() - URL: {} ({} bytes)", url, data.len());
        let fs = self.get_filesystem(url)?;
        let path = self.extract_path(url)?;
        tracing::debug!("📎 Routing to {} filesystem for path: {}", fs.filesystem_type(), path);
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
        tracing::debug!("🚮 Routing to {} filesystem for path: {}", fs.filesystem_type(), path);
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
mod tests {
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
        
        assert_eq!(factory.extract_scheme("file:///tmp/test.txt").unwrap(), "file");
        assert_eq!(factory.extract_scheme("s3://bucket/key").unwrap(), "s3");
        assert_eq!(factory.extract_scheme("adls://account/container/path").unwrap(), "adls");
        assert_eq!(factory.extract_scheme("abfs://container@account/path").unwrap(), "abfs");
        assert_eq!(factory.extract_scheme("gcs://bucket/object").unwrap(), "gcs");
        assert_eq!(factory.extract_scheme("hdfs://namenode:9000/path").unwrap(), "hdfs");
    }
    
    #[tokio::test]
    async fn test_path_extraction() {
        let config = FilesystemConfig::default();
        let factory = FilesystemFactory::new(config).await.unwrap();
        
        assert_eq!(factory.extract_path("file:///tmp/test.txt").unwrap(), "/tmp/test.txt");
        assert_eq!(factory.extract_path("s3://bucket/key").unwrap(), "/key");
        assert_eq!(factory.extract_path("/local/path").unwrap(), "/local/path");
    }
}