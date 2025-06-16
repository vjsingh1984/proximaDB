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

//! Google Cloud Storage (GCS) filesystem implementation with service accounts and ADC

use std::collections::HashMap;
use async_trait::async_trait;
use serde::{Serialize, Deserialize};
use tokio::time::{Duration, Instant};

use super::{FileSystem, FileMetadata, DirEntry, FileOptions, FsResult, FilesystemError};
use super::auth::{GcsCredentials, GcsCredentialProvider};

/// GCS storage classes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GcsStorageClass {
    Standard,
    Nearline,
    Coldline,
    Archive,
}

impl GcsStorageClass {
    fn as_str(&self) -> &'static str {
        match self {
            GcsStorageClass::Standard => "STANDARD",
            GcsStorageClass::Nearline => "NEARLINE",
            GcsStorageClass::Coldline => "COLDLINE",
            GcsStorageClass::Archive => "ARCHIVE",
        }
    }
}

/// GCS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcsConfig {
    /// Google Cloud project ID
    pub project_id: String,
    
    /// Default bucket for unqualified paths
    pub default_bucket: Option<String>,
    
    /// GCS credentials configuration
    pub credentials: GcsCredentialConfig,
    
    /// Default storage class
    pub default_storage_class: GcsStorageClass,
    
    /// Request timeout in seconds
    pub timeout_seconds: u64,
    
    /// Maximum retry attempts
    pub max_retries: u32,
    
    /// Enable resumable uploads for large files
    pub resumable_threshold: u64, // bytes
    
    /// Upload chunk size for resumable uploads
    pub upload_chunk_size: u64, // bytes
}

/// GCS credential configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcsCredentialConfig {
    /// Credential provider type
    pub provider: GcsCredentialProviderType,
    
    /// Service account key file path (for service account auth)
    pub service_account_key_file: Option<String>,
    
    /// Service account key JSON (for service account auth) 
    pub service_account_key_json: Option<String>,
    
    /// Credential refresh interval in seconds
    pub refresh_interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GcsCredentialProviderType {
    /// Use Application Default Credentials (ADC)
    ApplicationDefault,
    /// Use service account key file
    ServiceAccountKey,
    /// Use service account key JSON
    ServiceAccountJson,
    /// Use compute engine metadata service
    ComputeEngine,
    /// Use environment variables
    Environment,
}

impl Default for GcsConfig {
    fn default() -> Self {
        Self {
            project_id: "default-project".to_string(),
            default_bucket: None,
            credentials: GcsCredentialConfig {
                provider: GcsCredentialProviderType::ApplicationDefault,
                service_account_key_file: None,
                service_account_key_json: None,
                refresh_interval: 3600, // 1 hour
            },
            default_storage_class: GcsStorageClass::Standard,
            timeout_seconds: 30,
            max_retries: 3,
            resumable_threshold: 5 * 1024 * 1024, // 5MB
            upload_chunk_size: 1 * 1024 * 1024, // 1MB
        }
    }
}

/// GCS filesystem implementation
pub struct GcsFileSystem {
    config: GcsConfig,
    credential_provider: Box<dyn GcsCredentialProvider>,
    client: GcsClient,
}

/// Simple GCS client abstraction (in production, use google-cloud-storage)
struct GcsClient {
    config: GcsConfig,
    http_client: reqwest::Client,
}

impl GcsFileSystem {
    /// Create new GCS filesystem instance
    pub async fn new(config: GcsConfig) -> FsResult<Self> {
        tracing::debug!("🔧 Initializing GCS filesystem for project: {}", config.project_id);
        
        // Initialize credential provider
        let credential_provider = Self::create_credential_provider(&config.credentials).await?;
        
        // Initialize GCS client
        let client = GcsClient::new(config.clone()).await?;
        
        tracing::info!("✅ GCS filesystem initialized successfully");
        
        Ok(Self {
            config,
            credential_provider,
            client,
        })
    }
    
    /// Create credential provider based on configuration
    async fn create_credential_provider(config: &GcsCredentialConfig) -> FsResult<Box<dyn GcsCredentialProvider>> {
        tracing::debug!("🔐 Creating GCS credential provider: {:?}", config.provider);
        
        match &config.provider {
            GcsCredentialProviderType::ApplicationDefault => {
                Ok(Box::new(super::auth::GcsApplicationDefaultProvider::new()))
            },
            GcsCredentialProviderType::ServiceAccountKey => {
                if let Some(key_file_path) = &config.service_account_key_file {
                    Ok(Box::new(ServiceAccountKeyFileProvider::new(key_file_path.clone())))
                } else {
                    Err(FilesystemError::Config(
                        "Service account key provider requires service_account_key_file".to_string()
                    ))
                }
            },
            GcsCredentialProviderType::ServiceAccountJson => {
                if let Some(key_json) = &config.service_account_key_json {
                    Ok(Box::new(ServiceAccountKeyJsonProvider::new(key_json.clone())))
                } else {
                    Err(FilesystemError::Config(
                        "Service account JSON provider requires service_account_key_json".to_string()
                    ))
                }
            },
            GcsCredentialProviderType::ComputeEngine => {
                Ok(Box::new(super::auth::GcsApplicationDefaultProvider::new()))
            },
            GcsCredentialProviderType::Environment => {
                Ok(Box::new(super::auth::GcsApplicationDefaultProvider::new()))
            },
        }
    }
    
    /// Parse GCS URL to extract bucket and object path
    fn parse_gcs_url(&self, path: &str) -> FsResult<(String, String)> {
        tracing::trace!("🔍 Parsing GCS URL: {}", path);
        
        if path.starts_with('/') {
            // Remove leading slash for object path
            let object_path = &path[1..];
            if let Some(ref bucket) = self.config.default_bucket {
                tracing::trace!("✅ Using default bucket: {} for path: {}", bucket, object_path);
                Ok((bucket.clone(), object_path.to_string()))
            } else {
                Err(FilesystemError::Config(
                    "No default bucket configured for relative GCS paths".to_string()
                ))
            }
        } else if path.contains('/') {
            // Extract bucket from path
            let parts: Vec<&str> = path.splitn(2, '/').collect();
            tracing::trace!("✅ Extracted bucket: {} and object: {}", parts[0], parts[1]);
            Ok((parts[0].to_string(), parts[1].to_string()))
        } else {
            Err(FilesystemError::Config(
                format!("Invalid GCS path format: {}", path)
            ))
        }
    }
}

impl GcsClient {
    async fn new(config: GcsConfig) -> FsResult<Self> {
        tracing::debug!("🌐 Creating GCS HTTP client with timeout: {}s", config.timeout_seconds);
        
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| FilesystemError::Network(e.to_string()))?;
        
        Ok(Self {
            config,
            http_client,
        })
    }
    
    /// Get GCS object
    async fn get_object(&self, bucket: &str, object_path: &str, credentials: &GcsCredentials) -> FsResult<Vec<u8>> {
        let url = format!("https://storage.googleapis.com/storage/v1/b/{}/o/{}?alt=media", 
                         bucket, urlencoding::encode(object_path));
        
        tracing::debug!("📥 Getting GCS object: {}", url);
        
        let response = self.http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", credentials.access_token))
            .send()
            .await
            .map_err(|e| {
                tracing::error!("❌ GCS GET request failed: {}", e);
                FilesystemError::Network(e.to_string())
            })?;
        
        if response.status().is_success() {
            let data = response.bytes().await
                .map(|b| b.to_vec())
                .map_err(|e| FilesystemError::Network(e.to_string()))?;
            
            tracing::debug!("✅ Successfully retrieved {} bytes from GCS", data.len());
            Ok(data)
        } else if response.status().as_u16() == 404 {
            tracing::warn!("🔍 GCS object not found: gcs://{}/{}", bucket, object_path);
            Err(FilesystemError::NotFound(format!("gcs://{}/{}", bucket, object_path)))
        } else {
            tracing::error!("❌ GCS GET error: {}", response.status());
            Err(FilesystemError::Network(
                format!("GCS error: {}", response.status())
            ))
        }
    }
    
    /// Put GCS object
    async fn put_object(&self, bucket: &str, object_path: &str, data: &[u8], credentials: &GcsCredentials, options: Option<FileOptions>) -> FsResult<()> {
        let url = format!("https://storage.googleapis.com/upload/storage/v1/b/{}/o?uploadType=media&name={}", 
                         bucket, urlencoding::encode(object_path));
        
        tracing::debug!("📤 Uploading {} bytes to GCS: {}", data.len(), url);
        
        let options = options.unwrap_or_default();
        
        let mut request = self.http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", credentials.access_token))
            .header("Content-Type", "application/octet-stream")
            .body(data.to_vec());
        
        // Add storage class if specified
        if let Some(storage_class) = options.storage_class {
            request = request.header("x-goog-storage-class", storage_class);
        } else {
            request = request.header("x-goog-storage-class", self.config.default_storage_class.as_str());
        }
        
        // Add metadata if specified
        if let Some(metadata) = options.metadata {
            for (key, value) in metadata {
                request = request.header(format!("x-goog-meta-{}", key), value);
            }
        }
        
        let response = request.send().await
            .map_err(|e| {
                tracing::error!("❌ GCS PUT request failed: {}", e);
                FilesystemError::Network(e.to_string())
            })?;
        
        if response.status().is_success() {
            tracing::debug!("✅ Successfully uploaded to GCS");
            Ok(())
        } else {
            tracing::error!("❌ GCS PUT error: {}", response.status());
            Err(FilesystemError::Network(
                format!("GCS PUT error: {}", response.status())
            ))
        }
    }
}

#[async_trait]
impl FileSystem for GcsFileSystem {
    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        tracing::debug!("📖 GCS read: {}", path);
        let (bucket, object_path) = self.parse_gcs_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;
        self.client.get_object(&bucket, &object_path, &credentials).await
    }
    
    async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()> {
        tracing::debug!("📝 GCS write: {} ({} bytes)", path, data.len());
        let (bucket, object_path) = self.parse_gcs_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;
        
        if data.len() as u64 > self.config.resumable_threshold {
            tracing::debug!("🔄 Using resumable upload for large file");
            // Use resumable upload for large files
            self.resumable_upload(&bucket, &object_path, data, &credentials, options).await
        } else {
            self.client.put_object(&bucket, &object_path, data, &credentials, options).await
        }
    }
    
    async fn append(&self, path: &str, data: &[u8]) -> FsResult<()> {
        tracing::debug!("➕ GCS append: {} ({} bytes)", path, data.len());
        // GCS doesn't support append operations directly
        // We need to read, concatenate, and write back
        let existing_data = match self.read(path).await {
            Ok(data) => {
                tracing::debug!("📚 Found existing data: {} bytes", data.len());
                data
            },
            Err(FilesystemError::NotFound(_)) => {
                tracing::debug!("🆕 Creating new file for append");
                Vec::new()
            },
            Err(e) => return Err(e),
        };
        
        let mut combined_data = existing_data;
        combined_data.extend_from_slice(data);
        
        tracing::debug!("📝 Writing combined data: {} bytes", combined_data.len());
        self.write(path, &combined_data, None).await
    }
    
    async fn delete(&self, path: &str) -> FsResult<()> {
        tracing::debug!("🗑️ GCS delete: {}", path);
        let (bucket, object_path) = self.parse_gcs_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;
        
        let url = format!("https://storage.googleapis.com/storage/v1/b/{}/o/{}", 
                         bucket, urlencoding::encode(&object_path));
        
        let response = self.client.http_client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", credentials.access_token))
            .send()
            .await
            .map_err(|e| {
                tracing::error!("❌ GCS DELETE request failed: {}", e);
                FilesystemError::Network(e.to_string())
            })?;
        
        if response.status().is_success() || response.status().as_u16() == 404 {
            tracing::debug!("✅ GCS delete successful");
            Ok(())
        } else {
            tracing::error!("❌ GCS DELETE error: {}", response.status());
            Err(FilesystemError::Network(
                format!("GCS DELETE error: {}", response.status())
            ))
        }
    }
    
    async fn exists(&self, path: &str) -> FsResult<bool> {
        tracing::trace!("🔍 GCS exists check: {}", path);
        match self.metadata(path).await {
            Ok(_) => {
                tracing::trace!("✅ GCS object exists");
                Ok(true)
            },
            Err(FilesystemError::NotFound(_)) => {
                tracing::trace!("❌ GCS object does not exist");
                Ok(false)
            },
            Err(e) => Err(e),
        }
    }
    
    async fn metadata(&self, path: &str) -> FsResult<FileMetadata> {
        tracing::debug!("📊 GCS metadata: {}", path);
        let (bucket, object_path) = self.parse_gcs_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;
        
        let url = format!("https://storage.googleapis.com/storage/v1/b/{}/o/{}", 
                         bucket, urlencoding::encode(&object_path));
        
        let response = self.client.http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", credentials.access_token))
            .send()
            .await
            .map_err(|e| {
                tracing::error!("❌ GCS metadata request failed: {}", e);
                FilesystemError::Network(e.to_string())
            })?;
        
        if response.status().is_success() {
            let metadata_json: serde_json::Value = response.json().await
                .map_err(|e| FilesystemError::Network(e.to_string()))?;
            
            let size = metadata_json["size"].as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            
            let etag = metadata_json["etag"].as_str().map(|s| s.to_string());
            let storage_class = metadata_json["storageClass"].as_str().map(|s| s.to_string());
            
            tracing::debug!("✅ GCS metadata retrieved: {} bytes", size);
            
            Ok(FileMetadata {
                path: format!("gcs://{}/{}", bucket, object_path),
                size,
                created: None, // Would need to parse timeCreated field
                modified: None, // Would need to parse updated field
                is_directory: false, // GCS objects are always files
                permissions: None,
                etag,
                storage_class,
            })
        } else if response.status().as_u16() == 404 {
            tracing::warn!("🔍 GCS metadata not found: gcs://{}/{}", bucket, object_path);
            Err(FilesystemError::NotFound(format!("gcs://{}/{}", bucket, object_path)))
        } else {
            tracing::error!("❌ GCS metadata error: {}", response.status());
            Err(FilesystemError::Network(
                format!("GCS metadata error: {}", response.status())
            ))
        }
    }
    
    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        tracing::debug!("📋 GCS list: {}", path);
        // GCS list objects implementation
        // This would use Objects: list API in production
        let (bucket, prefix) = self.parse_gcs_url(path)?;
        let _credentials = self.credential_provider.get_credentials().await?;
        
        tracing::debug!("📋 Listing objects in bucket: {} with prefix: {}", bucket, prefix);
        
        // Simplified implementation - in production use proper GCS Objects: list
        Ok(Vec::new()) // Placeholder
    }
    
    async fn create_dir(&self, _path: &str) -> FsResult<()> {
        tracing::trace!("📁 GCS create_dir (no-op)");
        // GCS doesn't have directories, this is a no-op
        Ok(())
    }
    
    async fn create_dir_all(&self, _path: &str) -> FsResult<()> {
        tracing::trace!("📁 GCS create_dir_all (no-op)");
        // GCS doesn't have directories, this is a no-op
        Ok(())
    }
    
    async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        tracing::debug!("📋 GCS copy: {} → {}", from, to);
        // GCS copy object implementation
        let data = self.read(from).await?;
        self.write(to, &data, None).await
    }
    
    async fn move_file(&self, from: &str, to: &str) -> FsResult<()> {
        tracing::debug!("🔄 GCS move: {} → {}", from, to);
        self.copy(from, to).await?;
        self.delete(from).await
    }
    
    fn filesystem_type(&self) -> &'static str {
        "gcs"
    }
    
    async fn sync(&self) -> FsResult<()> {
        tracing::trace!("🔄 GCS sync (no-op)");
        // GCS operations are immediately durable
        Ok(())
    }
}

impl GcsFileSystem {
    /// Resumable upload for large files
    async fn resumable_upload(&self, bucket: &str, object_path: &str, data: &[u8], credentials: &GcsCredentials, options: Option<FileOptions>) -> FsResult<()> {
        // In production, implement GCS resumable upload
        // 1. Initiate resumable upload session
        // 2. Upload data in chunks
        // 3. Finalize upload
        
        tracing::debug!("🔄 Resumable upload not fully implemented, falling back to simple upload");
        
        // For now, fall back to regular upload
        self.client.put_object(bucket, object_path, data, credentials, options).await
    }
}

/// Service account key file credential provider
pub struct ServiceAccountKeyFileProvider {
    key_file_path: String,
    http_client: reqwest::Client,
}

impl ServiceAccountKeyFileProvider {
    pub fn new(key_file_path: String) -> Self {
        Self {
            key_file_path,
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl GcsCredentialProvider for ServiceAccountKeyFileProvider {
    async fn get_credentials(&self) -> FsResult<GcsCredentials> {
        tracing::debug!("🔐 Loading service account key file: {}", self.key_file_path);
        
        let key_data = tokio::fs::read_to_string(&self.key_file_path).await
            .map_err(|e| FilesystemError::Config(format!("Failed to read service account key file: {}", e)))?;
        
        let key_json: serde_json::Value = serde_json::from_str(&key_data)
            .map_err(|e| FilesystemError::Config(format!("Failed to parse service account key: {}", e)))?;
        
        // In production, implement proper JWT-based authentication
        // This is a placeholder implementation
        Err(FilesystemError::Auth("Service account key authentication not fully implemented".to_string()))
    }
    
    async fn refresh_credentials(&self) -> FsResult<GcsCredentials> {
        self.get_credentials().await
    }
}

/// Service account key JSON credential provider
pub struct ServiceAccountKeyJsonProvider {
    key_json: String,
    http_client: reqwest::Client,
}

impl ServiceAccountKeyJsonProvider {
    pub fn new(key_json: String) -> Self {
        Self {
            key_json,
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl GcsCredentialProvider for ServiceAccountKeyJsonProvider {
    async fn get_credentials(&self) -> FsResult<GcsCredentials> {
        tracing::debug!("🔐 Using service account key JSON");
        
        let key_json: serde_json::Value = serde_json::from_str(&self.key_json)
            .map_err(|e| FilesystemError::Config(format!("Failed to parse service account key JSON: {}", e)))?;
        
        // In production, implement proper JWT-based authentication
        // This is a placeholder implementation
        Err(FilesystemError::Auth("Service account JSON authentication not fully implemented".to_string()))
    }
    
    async fn refresh_credentials(&self) -> FsResult<GcsCredentials> {
        self.get_credentials().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_gcs_url_parsing() {
        let config = GcsConfig {
            project_id: "test-project".to_string(),
            default_bucket: Some("test-bucket".to_string()),
            ..Default::default()
        };
        
        let credential_provider = Box::new(super::auth::GcsApplicationDefaultProvider::new());
        let client = GcsClient::new(config.clone()).await.unwrap();
        
        let fs = GcsFileSystem {
            config,
            credential_provider,
            client,
        };
        
        // Test with default bucket
        let (bucket, object_path) = fs.parse_gcs_url("/path/to/file.txt").unwrap();
        assert_eq!(bucket, "test-bucket");
        assert_eq!(object_path, "path/to/file.txt");
        
        // Test with explicit bucket
        let (bucket, object_path) = fs.parse_gcs_url("my-bucket/path/to/file.txt").unwrap();
        assert_eq!(bucket, "my-bucket");
        assert_eq!(object_path, "path/to/file.txt");
    }
}