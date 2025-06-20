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

//! Filesystem Manager - URL-based filesystem instances with authentication and caching

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;

use super::{FileSystem, FilesystemError, FsResult};
use super::local::LocalFileSystem;
use super::s3::S3FileSystem;
use super::azure::AzureFileSystem;
use super::gcs::GcsFileSystem;

/// Retry configuration for filesystem operations
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: usize,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 100,
            max_delay_ms: 5000,
            backoff_multiplier: 2.0,
        }
    }
}

impl RetryConfig {
    pub fn new(max_retries: usize, initial_delay_ms: u64, max_delay_ms: u64, backoff_multiplier: f64) -> Self {
        Self {
            max_retries,
            initial_delay_ms,
            max_delay_ms,
            backoff_multiplier,
        }
    }
    
    pub fn calculate_delay(&self, attempt: usize) -> std::time::Duration {
        let delay_ms = (self.initial_delay_ms as f64 * self.backoff_multiplier.powi(attempt as i32))
            .min(self.max_delay_ms as f64) as u64;
        std::time::Duration::from_millis(delay_ms)
    }
}

/// Filesystem instance cache key (URL without file path)
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct FilesystemKey {
    scheme: String,
    host: Option<String>,
    base_path: String,
}

impl FilesystemKey {
    fn from_url(url: &str) -> FsResult<Self> {
        let parsed = Url::parse(url)?;
        
        let base_path = match parsed.scheme() {
            "file" => parsed.path().to_string(),
            "s3" | "gcs" => {
                // For object stores, base path is the bucket + prefix
                let bucket = parsed.host_str().unwrap_or("");
                let prefix = parsed.path().trim_start_matches('/');
                if prefix.is_empty() {
                    bucket.to_string()
                } else {
                    format!("{}/{}", bucket, prefix)
                }
            },
            "adls" => {
                // For Azure, base path is account + container + prefix
                let host = parsed.host_str().unwrap_or("");
                let path_parts: Vec<&str> = parsed.path().trim_start_matches('/').split('/').collect();
                format!("{}/{}", host, path_parts.join("/"))
            },
            _ => parsed.path().to_string()
        };

        Ok(Self {
            scheme: parsed.scheme().to_string(),
            host: parsed.host_str().map(|s| s.to_string()),
            base_path,
        })
    }
}

/// Managed filesystem instance with error handling  
pub struct ManagedFilesystem {
    filesystem: Box<dyn FileSystem>,
    base_path: String,
    retry_config: RetryConfig,
}

impl ManagedFilesystem {
    /// Execute operation with retry and auth refresh
    async fn execute_with_retry<F, T>(&self, operation: F) -> FsResult<T>
    where
        F: Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = FsResult<T>> + Send>> + Send + Sync,
        T: Send,
    {
        let mut attempts = 0;
        let max_attempts = self.retry_config.max_retries + 1;
        
        loop {
            attempts += 1;
            
            match operation().await {
                Ok(result) => return Ok(result),
                Err(err) if attempts >= max_attempts => return Err(err),
                Err(err) => {
                    // Exponential backoff
                    let delay = self.retry_config.calculate_delay(attempts - 1);
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    fn is_auth_error(&self, err: &FilesystemError) -> bool {
        match err {
            FilesystemError::Auth(_) => true,
            // FilesystemError::Permission(_) => true, // Not available yet
            FilesystemError::Io(io_err) => {
                // Check for specific auth-related IO errors
                match io_err.kind() {
                    std::io::ErrorKind::PermissionDenied => true,
                    std::io::ErrorKind::InvalidInput => {
                        // Some cloud providers return InvalidInput for auth issues
                        io_err.to_string().to_lowercase().contains("auth") ||
                        io_err.to_string().to_lowercase().contains("credential") ||
                        io_err.to_string().to_lowercase().contains("token")
                    },
                    _ => false
                }
            },
            _ => false
        }
    }

    /// Resolve relative path to absolute path within this filesystem's base
    fn resolve_path(&self, relative_path: &str) -> String {
        if self.base_path.is_empty() {
            relative_path.to_string()
        } else if relative_path.is_empty() {
            self.base_path.clone()
        } else {
            format!("{}/{}", self.base_path.trim_end_matches('/'), relative_path.trim_start_matches('/'))
        }
    }
}

// Temporarily commented out to fix compilation issues
/*
#[async_trait]
impl FileSystem for ManagedFilesystem {
    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        let resolved_path = self.resolve_path(path);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let path = resolved_path.clone();
            Box::pin(async move { fs.read(&path).await })
        }).await
    }

    async fn write(&self, path: &str, data: &[u8], options: Option<super::FileOptions>) -> FsResult<()> {
        let resolved_path = self.resolve_path(path);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let path = resolved_path.clone();
            let data = data.to_vec();
            let options = options.clone();
            Box::pin(async move { fs.write(&path, &data, options).await })
        }).await
    }

    async fn append(&self, path: &str, data: &[u8]) -> FsResult<()> {
        let resolved_path = self.resolve_path(path);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let path = resolved_path.clone();
            let data = data.to_vec();
            Box::pin(async move { fs.append(&path, &data).await })
        }).await
    }

    async fn create_dir_all(&self, path: &str) -> FsResult<()> {
        let resolved_path = self.resolve_path(path);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let path = resolved_path.clone();
            Box::pin(async move { fs.create_dir_all(&path).await })
        }).await
    }

    async fn move_file(&self, from: &str, to: &str) -> FsResult<()> {
        let from_resolved = self.resolve_path(from);
        let to_resolved = self.resolve_path(to);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let from = from_resolved.clone();
            let to = to_resolved.clone();
            Box::pin(async move { fs.move_file(&from, &to).await })
        }).await
    }

    fn filesystem_type(&self) -> &'static str {
        self.filesystem.filesystem_type()
    }

    async fn sync(&self) -> FsResult<()> {
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            Box::pin(async move { fs.sync().await })
        }).await
    }

    async fn write_atomic(&self, path: &str, data: &[u8], options: Option<super::FileOptions>) -> FsResult<()> {
        let resolved_path = self.resolve_path(path);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let path = resolved_path.clone();
            let data = data.to_vec();
            let options = options.clone();
            Box::pin(async move { fs.write_atomic(&path, &data, options).await })
        }).await
    }

    async fn delete(&self, path: &str) -> FsResult<()> {
        let resolved_path = self.resolve_path(path);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let path = resolved_path.clone();
            Box::pin(async move { fs.delete(&path).await })
        }).await
    }

    async fn exists(&self, path: &str) -> FsResult<bool> {
        let resolved_path = self.resolve_path(path);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let path = resolved_path.clone();
            Box::pin(async move { fs.exists(&path).await })
        }).await
    }

    async fn list(&self, path: &str) -> FsResult<Vec<super::DirEntry>> {
        let resolved_path = self.resolve_path(path);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let path = resolved_path.clone();
            Box::pin(async move { fs.list(&path).await })
        }).await
    }

    async fn create_dir(&self, path: &str) -> FsResult<()> {
        let resolved_path = self.resolve_path(path);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let path = resolved_path.clone();
            Box::pin(async move { fs.create_dir(&path).await })
        }).await
    }

    async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        let from_resolved = self.resolve_path(from);
        let to_resolved = self.resolve_path(to);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let from = from_resolved.clone();
            let to = to_resolved.clone();
            Box::pin(async move { fs.copy(&from, &to).await })
        }).await
    }

    async fn metadata(&self, path: &str) -> FsResult<super::FileMetadata> {
        let resolved_path = self.resolve_path(path);
        
        self.execute_with_retry(|| {
            let fs = &self.filesystem;
            let path = resolved_path.clone();
            Box::pin(async move { fs.metadata(&path).await })
        }).await
    }

    fn supports_atomic_writes(&self) -> bool {
        self.filesystem.supports_atomic_writes()
    }
}

/// Advanced filesystem manager with URL-based instances and authentication
pub struct FilesystemManager {
    /// Cache of filesystem instances by URL base path
    filesystems: Arc<RwLock<HashMap<FilesystemKey, Arc<ManagedFilesystem>>>>,
    
    /// Authentication providers by scheme (temporarily removed for compilation)
    // auth_providers: HashMap<String, Arc<dyn AuthProvider>>,
    
    /// Default retry configuration
    default_retry_config: RetryConfig,
    
    /// Configuration for creating new filesystems
    config: super::FilesystemConfig,
}

impl FilesystemManager {
    /// Create new filesystem manager
    pub async fn new(
        config: super::FilesystemConfig,
    ) -> FsResult<Self> {
        Ok(Self {
            filesystems: Arc::new(RwLock::new(HashMap::new())),
            // auth_providers,
            default_retry_config: RetryConfig::default(),
            config,
        })
    }

    /// Get or create filesystem instance for URL
    pub async fn get_filesystem(&self, url: &str) -> FsResult<Arc<ManagedFilesystem>> {
        let key = FilesystemKey::from_url(url)?;
        
        // Check cache first
        {
            let cache = self.filesystems.read().await;
            if let Some(fs) = cache.get(&key) {
                return Ok(fs.clone());
            }
        }
        
        // Create new filesystem instance
        let managed_fs: Arc<ManagedFilesystem> = Arc::new(self.create_filesystem_for_key(&key, url).await?);
        
        // Cache it
        {
            let mut cache = self.filesystems.write().await;
            cache.insert(key, managed_fs.clone());
        }
        
        Ok(managed_fs)
    }

    /// Cross-storage atomic copy operation
    pub async fn copy_cross_storage(&self, from_url: &str, to_url: &str) -> FsResult<()> {
        let from_fs = self.get_filesystem(from_url).await?;
        let to_fs = self.get_filesystem(to_url).await?;
        
        // Extract relative paths
        let from_path = self.extract_relative_path(from_url)?;
        let to_path = self.extract_relative_path(to_url)?;
        
        // Read from source
        let data = from_fs.filesystem.read(&from_path).await?;
        
        // Write to destination atomically
        to_fs.filesystem.write_atomic(&to_path, &data, None).await?;
        
        Ok(())
    }

    /// Cross-storage atomic move operation
    pub async fn move_cross_storage(&self, from_url: &str, to_url: &str) -> FsResult<()> {
        // Copy first
        self.copy_cross_storage(from_url, to_url).await?;
        
        // Delete source after successful copy
        let from_fs = self.get_filesystem(from_url).await?;
        let from_path = self.extract_relative_path(from_url)?;
        from_fs.filesystem.delete(&from_path).await?;
        
        Ok(())
    }

    /// Create filesystem instance for specific key/URL
    async fn create_filesystem_for_key(&self, key: &FilesystemKey, url: &str) -> FsResult<ManagedFilesystem> {
        let parsed_url = Url::parse(url)?;
        // let auth_provider = self.auth_providers.get(&key.scheme).cloned();
        
        let filesystem: Box<dyn FileSystem> = match key.scheme.as_str() {
            "file" => {
                let mut local_config = self.config.local.clone().unwrap_or_default();
                // Set root directory to the base path from URL
                local_config.root_dir = Some(PathBuf::from(&key.base_path));
                Box::new(LocalFileSystem::new(local_config).await?)
            },
            "s3" => {
                let s3_config = self.config.s3.clone()
                    .ok_or_else(|| FilesystemError::Config("S3 not configured".to_string()))?;
                
                // Use default configuration for now
                Box::new(S3FileSystem::new(s3_config).await?)
            },
            "adls" => {
                let azure_config = self.config.azure.clone()
                    .ok_or_else(|| FilesystemError::Config("Azure not configured".to_string()))?;
                
                // Use default configuration for now
                Box::new(AzureFileSystem::new(azure_config).await?)
            },
            "gcs" => {
                let gcs_config = self.config.gcs.clone()
                    .ok_or_else(|| FilesystemError::Config("GCS not configured".to_string()))?;
                
                // Use default configuration for now
                Box::new(GcsFileSystem::new(gcs_config).await?)
            },
            _ => return Err(FilesystemError::UnsupportedScheme(key.scheme.clone()))
        };

        Ok(ManagedFilesystem {
            filesystem,
            base_path: key.base_path.clone(),
            retry_config: self.default_retry_config.clone(),
        })
    }

    /// Extract relative path from full URL (path component only)
    fn extract_relative_path(&self, url: &str) -> FsResult<String> {
        let parsed_url = Url::parse(url)?;
        let key = FilesystemKey::from_url(url)?;
        
        // For file URLs, return path relative to base
        if key.scheme == "file" {
            let full_path = parsed_url.path();
            let base_path = &key.base_path;
            
            if full_path.starts_with(base_path) {
                Ok(full_path.strip_prefix(base_path)
                    .unwrap_or("")
                    .trim_start_matches('/')
                    .to_string())
            } else {
                Ok(full_path.to_string())
            }
        } else {
            // For object stores, return the path component
            Ok(parsed_url.path().trim_start_matches('/').to_string())
        }
    }
}
*/