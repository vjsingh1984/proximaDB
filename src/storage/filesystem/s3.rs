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

//! Amazon S3 filesystem implementation with IAM roles, STS, and credential management

use std::collections::HashMap;
use async_trait::async_trait;
use serde::{Serialize, Deserialize};
use tokio::time::{Duration, Instant};

use super::{FileSystem, FileMetadata, DirEntry, FileOptions, FsResult, FilesystemError};
use super::auth::{AwsCredentials, CredentialProvider};

/// S3 storage classes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum S3StorageClass {
    Standard,
    StandardIa,
    OneZoneIa,
    Glacier,
    GlacierInstantRetrieval,
    GlacierFlexibleRetrieval,
    GlacierDeepArchive,
    IntelligentTiering,
}

impl S3StorageClass {
    fn as_str(&self) -> &'static str {
        match self {
            S3StorageClass::Standard => "STANDARD",
            S3StorageClass::StandardIa => "STANDARD_IA",
            S3StorageClass::OneZoneIa => "ONEZONE_IA",
            S3StorageClass::Glacier => "GLACIER",
            S3StorageClass::GlacierInstantRetrieval => "GLACIER_IR",
            S3StorageClass::GlacierFlexibleRetrieval => "GLACIER",
            S3StorageClass::GlacierDeepArchive => "DEEP_ARCHIVE",
            S3StorageClass::IntelligentTiering => "INTELLIGENT_TIERING",
        }
    }
}

/// S3 configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Config {
    /// AWS region
    pub region: String,
    
    /// Default bucket for unqualified paths
    pub default_bucket: Option<String>,
    
    /// AWS credentials configuration
    pub credentials: CredentialConfig,
    
    /// Default storage class
    pub default_storage_class: S3StorageClass,
    
    /// Server-side encryption
    pub encryption: Option<S3Encryption>,
    
    /// Request timeout in seconds
    pub timeout_seconds: u64,
    
    /// Maximum retry attempts
    pub max_retries: u32,
    
    /// Enable multipart upload for large files
    pub multipart_threshold: u64, // bytes
    
    /// Multipart chunk size
    pub multipart_chunk_size: u64, // bytes
}

/// S3 encryption configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Encryption {
    pub method: S3EncryptionMethod,
    pub kms_key_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum S3EncryptionMethod {
    None,
    Aes256,
    KmsManaged,
    KmsCustomerKey,
}

/// AWS credential configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialConfig {
    /// Credential provider type
    pub provider: CredentialProviderType,
    
    /// AWS access key ID (for static credentials)
    pub access_key_id: Option<String>,
    
    /// AWS secret access key (for static credentials)
    pub secret_access_key: Option<String>,
    
    /// AWS session token (for temporary credentials)
    pub session_token: Option<String>,
    
    /// IAM role ARN (for assume role)
    pub role_arn: Option<String>,
    
    /// External ID for role assumption
    pub external_id: Option<String>,
    
    /// Credential refresh interval in seconds
    pub refresh_interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CredentialProviderType {
    /// Use static access keys
    Static,
    /// Use EC2 instance metadata service
    InstanceMetadata,
    /// Use ECS task metadata service
    EcsTaskMetadata,
    /// Use assume role with STS
    AssumeRole,
    /// Use AWS profile from ~/.aws/credentials
    Profile(String),
    /// Use environment variables
    Environment,
    /// Use credential chain (recommended)
    Chain,
}

impl Default for S3Config {
    fn default() -> Self {
        Self {
            region: "us-east-1".to_string(),
            default_bucket: None,
            credentials: CredentialConfig {
                provider: CredentialProviderType::Chain,
                access_key_id: None,
                secret_access_key: None,
                session_token: None,
                role_arn: None,
                external_id: None,
                refresh_interval: 3600, // 1 hour
            },
            default_storage_class: S3StorageClass::Standard,
            encryption: None,
            timeout_seconds: 30,
            max_retries: 3,
            multipart_threshold: 5 * 1024 * 1024, // 5MB
            multipart_chunk_size: 5 * 1024 * 1024, // 5MB
        }
    }
}

/// S3 filesystem implementation
pub struct S3FileSystem {
    config: S3Config,
    credential_provider: Box<dyn CredentialProvider>,
    client: S3Client,
}

/// Simple S3 client abstraction (in production, use aws-sdk-s3)
struct S3Client {
    config: S3Config,
    http_client: reqwest::Client,
}

impl S3FileSystem {
    /// Create new S3 filesystem instance
    pub async fn new(config: S3Config) -> FsResult<Self> {
        // Initialize credential provider
        let credential_provider = Self::create_credential_provider(&config.credentials).await?;
        
        // Initialize S3 client
        let client = S3Client::new(config.clone()).await?;
        
        Ok(Self {
            config,
            credential_provider,
            client,
        })
    }
    
    /// Create credential provider based on configuration
    async fn create_credential_provider(config: &CredentialConfig) -> FsResult<Box<dyn CredentialProvider>> {
        match &config.provider {
            CredentialProviderType::Static => {
                if let (Some(access_key), Some(secret_key)) = 
                    (&config.access_key_id, &config.secret_access_key) {
                    Ok(Box::new(super::auth::StaticCredentialProvider::new(
                        access_key.clone(),
                        secret_key.clone(),
                        config.session_token.clone(),
                    )))
                } else {
                    Err(FilesystemError::Config(
                        "Static credentials require access_key_id and secret_access_key".to_string()
                    ))
                }
            },
            CredentialProviderType::InstanceMetadata => {
                Ok(Box::new(super::auth::InstanceMetadataProvider::new()))
            },
            CredentialProviderType::EcsTaskMetadata => {
                Ok(Box::new(super::auth::EcsTaskMetadataProvider::new()))
            },
            CredentialProviderType::AssumeRole => {
                if let Some(role_arn) = &config.role_arn {
                    Ok(Box::new(super::auth::AssumeRoleProvider::new(
                        role_arn.clone(),
                        config.external_id.clone(),
                    )))
                } else {
                    Err(FilesystemError::Config(
                        "AssumeRole provider requires role_arn".to_string()
                    ))
                }
            },
            CredentialProviderType::Profile(profile_name) => {
                Ok(Box::new(super::auth::ProfileCredentialProvider::new(
                    profile_name.clone()
                )))
            },
            CredentialProviderType::Environment => {
                Ok(Box::new(super::auth::EnvironmentCredentialProvider::new()))
            },
            CredentialProviderType::Chain => {
                Ok(Box::new(super::auth::ChainCredentialProvider::new()))
            },
        }
    }
    
    /// Parse S3 URL to extract bucket and key
    fn parse_s3_url(&self, path: &str) -> FsResult<(String, String)> {
        if path.starts_with('/') {
            // Remove leading slash for S3 key
            let key = &path[1..];
            if let Some(ref bucket) = self.config.default_bucket {
                Ok((bucket.clone(), key.to_string()))
            } else {
                Err(FilesystemError::Config(
                    "No default bucket configured for relative S3 paths".to_string()
                ))
            }
        } else if path.contains('/') {
            // Extract bucket from path
            let parts: Vec<&str> = path.splitn(2, '/').collect();
            Ok((parts[0].to_string(), parts[1].to_string()))
        } else {
            Err(FilesystemError::Config(
                format!("Invalid S3 path format: {}", path)
            ))
        }
    }
}

impl S3Client {
    async fn new(config: S3Config) -> FsResult<Self> {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| FilesystemError::Network(e.to_string()))?;
        
        Ok(Self {
            config,
            http_client,
        })
    }
    
    /// Get S3 object
    async fn get_object(&self, bucket: &str, key: &str, credentials: &AwsCredentials) -> FsResult<Vec<u8>> {
        // In production, use aws-sdk-s3
        // This is a simplified implementation for demonstration
        let url = format!("https://{}.s3.{}.amazonaws.com/{}", bucket, self.config.region, key);
        
        let response = self.http_client
            .get(&url)
            .header("Authorization", self.create_auth_header(credentials, "GET", bucket, key))
            .send()
            .await
            .map_err(|e| FilesystemError::Network(e.to_string()))?;
        
        if response.status().is_success() {
            response.bytes().await
                .map(|b| b.to_vec())
                .map_err(|e| FilesystemError::Network(e.to_string()))
        } else if response.status().as_u16() == 404 {
            Err(FilesystemError::NotFound(format!("s3://{}/{}", bucket, key)))
        } else {
            Err(FilesystemError::Network(
                format!("S3 error: {}", response.status())
            ))
        }
    }
    
    /// Put S3 object
    async fn put_object(&self, bucket: &str, key: &str, data: &[u8], credentials: &AwsCredentials, options: Option<FileOptions>) -> FsResult<()> {
        let url = format!("https://{}.s3.{}.amazonaws.com/{}", bucket, self.config.region, key);
        let options = options.unwrap_or_default();
        
        let mut request = self.http_client
            .put(&url)
            .header("Authorization", self.create_auth_header(credentials, "PUT", bucket, key))
            .body(data.to_vec());
        
        // Add storage class if specified
        if let Some(storage_class) = options.storage_class {
            request = request.header("x-amz-storage-class", storage_class);
        } else {
            request = request.header("x-amz-storage-class", self.config.default_storage_class.as_str());
        }
        
        // Add metadata if specified
        if let Some(metadata) = options.metadata {
            for (key, value) in metadata {
                request = request.header(format!("x-amz-meta-{}", key), value);
            }
        }
        
        let response = request.send().await
            .map_err(|e| FilesystemError::Network(e.to_string()))?;
        
        if response.status().is_success() {
            Ok(())
        } else {
            Err(FilesystemError::Network(
                format!("S3 PUT error: {}", response.status())
            ))
        }
    }
    
    /// Create AWS v4 signature authorization header (simplified)
    fn create_auth_header(&self, credentials: &AwsCredentials, method: &str, bucket: &str, key: &str) -> String {
        // In production, implement proper AWS Signature Version 4
        // This is a placeholder for demonstration
        format!("AWS4-HMAC-SHA256 Credential={}/20241214/{}/s3/aws4_request", 
                credentials.access_key_id, self.config.region)
    }
}

#[async_trait]
impl FileSystem for S3FileSystem {
    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        let (bucket, key) = self.parse_s3_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;
        self.client.get_object(&bucket, &key, &credentials).await
    }
    
    async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()> {
        let (bucket, key) = self.parse_s3_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;
        
        if data.len() as u64 > self.config.multipart_threshold {
            // Use multipart upload for large files
            self.multipart_upload(&bucket, &key, data, &credentials, options).await
        } else {
            self.client.put_object(&bucket, &key, data, &credentials, options).await
        }
    }
    
    async fn append(&self, path: &str, data: &[u8]) -> FsResult<()> {
        // S3 doesn't support append operations directly
        // We need to read, concatenate, and write back
        let existing_data = match self.read(path).await {
            Ok(data) => data,
            Err(FilesystemError::NotFound(_)) => Vec::new(),
            Err(e) => return Err(e),
        };
        
        let mut combined_data = existing_data;
        combined_data.extend_from_slice(data);
        
        self.write(path, &combined_data, None).await
    }
    
    async fn delete(&self, path: &str) -> FsResult<()> {
        let (bucket, key) = self.parse_s3_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;
        
        let url = format!("https://{}.s3.{}.amazonaws.com/{}", bucket, self.config.region, key);
        
        let response = self.client.http_client
            .delete(&url)
            .header("Authorization", self.client.create_auth_header(&credentials, "DELETE", &bucket, &key))
            .send()
            .await
            .map_err(|e| FilesystemError::Network(e.to_string()))?;
        
        if response.status().is_success() || response.status().as_u16() == 404 {
            Ok(())
        } else {
            Err(FilesystemError::Network(
                format!("S3 DELETE error: {}", response.status())
            ))
        }
    }
    
    async fn exists(&self, path: &str) -> FsResult<bool> {
        match self.metadata(path).await {
            Ok(_) => Ok(true),
            Err(FilesystemError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
    
    async fn metadata(&self, path: &str) -> FsResult<FileMetadata> {
        let (bucket, key) = self.parse_s3_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;
        
        let url = format!("https://{}.s3.{}.amazonaws.com/{}", bucket, self.config.region, key);
        
        let response = self.client.http_client
            .head(&url)
            .header("Authorization", self.client.create_auth_header(&credentials, "HEAD", &bucket, &key))
            .send()
            .await
            .map_err(|e| FilesystemError::Network(e.to_string()))?;
        
        if response.status().is_success() {
            let size = response.headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            
            let etag = response.headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            
            let storage_class = response.headers()
                .get("x-amz-storage-class")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            
            Ok(FileMetadata {
                path: format!("s3://{}/{}", bucket, key),
                size,
                created: None, // S3 doesn't provide creation time in HEAD response
                modified: None, // Would need to parse Last-Modified header
                is_directory: false, // S3 objects are always files
                permissions: None,
                etag,
                storage_class,
            })
        } else if response.status().as_u16() == 404 {
            Err(FilesystemError::NotFound(format!("s3://{}/{}", bucket, key)))
        } else {
            Err(FilesystemError::Network(
                format!("S3 HEAD error: {}", response.status())
            ))
        }
    }
    
    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        // S3 list objects implementation
        // This would use ListObjectsV2 API in production
        let (bucket, prefix) = self.parse_s3_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;
        
        // Simplified implementation - in production use proper S3 ListObjectsV2
        Ok(Vec::new()) // Placeholder
    }
    
    async fn create_dir(&self, _path: &str) -> FsResult<()> {
        // S3 doesn't have directories, this is a no-op
        Ok(())
    }
    
    async fn create_dir_all(&self, _path: &str) -> FsResult<()> {
        // S3 doesn't have directories, this is a no-op
        Ok(())
    }
    
    async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        // S3 CopyObject implementation
        let (from_bucket, from_key) = self.parse_s3_url(from)?;
        let (to_bucket, to_key) = self.parse_s3_url(to)?;
        let credentials = self.credential_provider.get_credentials().await?;
        
        // In production, use S3 CopyObject API
        // For now, read and write
        let data = self.read(from).await?;
        self.write(to, &data, None).await
    }
    
    async fn move_file(&self, from: &str, to: &str) -> FsResult<()> {
        self.copy(from, to).await?;
        self.delete(from).await
    }
    
    fn filesystem_type(&self) -> &'static str {
        "s3"
    }
    
    async fn sync(&self) -> FsResult<()> {
        // S3 operations are immediately durable
        Ok(())
    }
}

impl S3FileSystem {
    /// Multipart upload for large files
    async fn multipart_upload(&self, bucket: &str, key: &str, data: &[u8], credentials: &AwsCredentials, options: Option<FileOptions>) -> FsResult<()> {
        // In production, implement S3 multipart upload
        // 1. InitiateMultipartUpload
        // 2. UploadPart for each chunk
        // 3. CompleteMultipartUpload
        
        // For now, fall back to regular upload
        self.client.put_object(bucket, key, data, credentials, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_s3_url_parsing() {
        let config = S3Config {
            default_bucket: Some("test-bucket".to_string()),
            ..Default::default()
        };
        
        let credential_provider = Box::new(super::super::auth::StaticCredentialProvider::new(
            "test_key".to_string(),
            "test_secret".to_string(),
            None,
        ));
        
        let client = S3Client::new(config.clone()).await.unwrap();
        
        let fs = S3FileSystem {
            config,
            credential_provider,
            client,
        };
        
        // Test with default bucket
        let (bucket, key) = fs.parse_s3_url("/path/to/file.txt").unwrap();
        assert_eq!(bucket, "test-bucket");
        assert_eq!(key, "path/to/file.txt");
        
        // Test with explicit bucket
        let (bucket, key) = fs.parse_s3_url("my-bucket/path/to/file.txt").unwrap();
        assert_eq!(bucket, "my-bucket");
        assert_eq!(key, "path/to/file.txt");
    }
}