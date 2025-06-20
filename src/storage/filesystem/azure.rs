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

//! Azure Data Lake Storage (ADLS) filesystem implementation with managed identity and authentication

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, Instant};

use super::auth::{AzureCredentialProvider, AzureCredentials};
use super::{DirEntry, FileMetadata, FileOptions, FileSystem, FilesystemError, FsResult};

/// Azure blob tier options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AzureBlobTier {
    Hot,
    Cool,
    Archive,
}

impl AzureBlobTier {
    fn as_str(&self) -> &'static str {
        match self {
            AzureBlobTier::Hot => "Hot",
            AzureBlobTier::Cool => "Cool",
            AzureBlobTier::Archive => "Archive",
        }
    }
}

/// Azure ADLS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzureConfig {
    /// Azure storage account name
    pub account_name: String,

    /// Default container for unqualified paths
    pub default_container: Option<String>,

    /// Azure credentials configuration
    pub credentials: AzureCredentialConfig,

    /// Default blob tier
    pub default_blob_tier: AzureBlobTier,

    /// Request timeout in seconds
    pub timeout_seconds: u64,

    /// Maximum retry attempts
    pub max_retries: u32,

    /// Enable hierarchical namespace (Data Lake Gen2)
    pub hierarchical_namespace: bool,

    /// Block size for uploads (bytes)
    pub block_size: u64,
}

/// Azure credential configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzureCredentialConfig {
    /// Credential provider type
    pub provider: AzureCredentialProviderType,

    /// Storage account key (for account key auth)
    pub account_key: Option<String>,

    /// SAS token (for SAS auth)
    pub sas_token: Option<String>,

    /// Managed identity client ID (optional)
    pub client_id: Option<String>,

    /// Azure AD tenant ID
    pub tenant_id: Option<String>,

    /// Service principal client ID  
    pub sp_client_id: Option<String>,

    /// Service principal client secret
    pub sp_client_secret: Option<String>,

    /// Credential refresh interval in seconds
    pub refresh_interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AzureCredentialProviderType {
    /// Use storage account key
    AccountKey,
    /// Use SAS token
    SasToken,
    /// Use managed identity (system or user-assigned)
    ManagedIdentity,
    /// Use service principal
    ServicePrincipal,
    /// Use Azure CLI credentials
    AzureCli,
    /// Use environment variables
    Environment,
}

impl Default for AzureConfig {
    fn default() -> Self {
        Self {
            account_name: "default".to_string(),
            default_container: None,
            credentials: AzureCredentialConfig {
                provider: AzureCredentialProviderType::ManagedIdentity,
                account_key: None,
                sas_token: None,
                client_id: None,
                tenant_id: None,
                sp_client_id: None,
                sp_client_secret: None,
                refresh_interval: 3600, // 1 hour
            },
            default_blob_tier: AzureBlobTier::Hot,
            timeout_seconds: 30,
            max_retries: 3,
            hierarchical_namespace: true, // Enable Data Lake Gen2 by default
            block_size: 4 * 1024 * 1024,  // 4MB
        }
    }
}

/// Azure ADLS filesystem implementation
pub struct AzureFileSystem {
    config: AzureConfig,
    credential_provider: Box<dyn AzureCredentialProvider>,
    client: AzureClient,
}

impl std::fmt::Debug for AzureFileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureFileSystem")
            .field("config", &self.config)
            .field("credential_provider", &"<credential_provider>")
            .field("client", &"<azure_client>")
            .finish()
    }
}

/// Simple Azure client abstraction (in production, use azure-sdk-for-rust)
#[derive(Clone)]
struct AzureClient {
    config: AzureConfig,
    http_client: reqwest::Client,
}

impl AzureFileSystem {
    /// Create new Azure filesystem instance
    pub async fn new(config: AzureConfig) -> FsResult<Self> {
        // Initialize credential provider
        let credential_provider = Self::create_credential_provider(&config.credentials).await?;

        // Initialize Azure client
        let client = AzureClient::new(config.clone()).await?;

        Ok(Self {
            config,
            credential_provider,
            client,
        })
    }

    /// Create credential provider based on configuration
    async fn create_credential_provider(
        config: &AzureCredentialConfig,
    ) -> FsResult<Box<dyn AzureCredentialProvider>> {
        match &config.provider {
            AzureCredentialProviderType::AccountKey => {
                if let Some(account_key) = &config.account_key {
                    Ok(Box::new(AccountKeyProvider::new(account_key.clone())))
                } else {
                    Err(FilesystemError::Config(
                        "Account key provider requires account_key".to_string(),
                    ))
                }
            }
            AzureCredentialProviderType::SasToken => {
                if let Some(sas_token) = &config.sas_token {
                    Ok(Box::new(SasTokenProvider::new(sas_token.clone())))
                } else {
                    Err(FilesystemError::Config(
                        "SAS token provider requires sas_token".to_string(),
                    ))
                }
            }
            AzureCredentialProviderType::ManagedIdentity => Ok(Box::new(
                super::auth::AzureManagedIdentityProvider::new(config.client_id.clone()),
            )),
            AzureCredentialProviderType::ServicePrincipal => {
                if let (Some(client_id), Some(client_secret), Some(tenant_id)) = (
                    &config.sp_client_id,
                    &config.sp_client_secret,
                    &config.tenant_id,
                ) {
                    Ok(Box::new(ServicePrincipalProvider::new(
                        tenant_id.clone(),
                        client_id.clone(),
                        client_secret.clone(),
                    )))
                } else {
                    Err(FilesystemError::Config(
                        "Service principal provider requires tenant_id, sp_client_id, and sp_client_secret".to_string()
                    ))
                }
            }
            _ => {
                // Fallback to managed identity for other types
                Ok(Box::new(super::auth::AzureManagedIdentityProvider::new(
                    config.client_id.clone(),
                )))
            }
        }
    }

    /// Parse Azure URL to extract container and blob path
    fn parse_azure_url(&self, path: &str) -> FsResult<(String, String)> {
        if path.starts_with('/') {
            // Remove leading slash for blob path
            let blob_path = &path[1..];
            if let Some(ref container) = self.config.default_container {
                Ok((container.clone(), blob_path.to_string()))
            } else {
                Err(FilesystemError::Config(
                    "No default container configured for relative Azure paths".to_string(),
                ))
            }
        } else if path.contains('/') {
            // Extract container from path
            let parts: Vec<&str> = path.splitn(2, '/').collect();
            Ok((parts[0].to_string(), parts[1].to_string()))
        } else {
            Err(FilesystemError::Config(format!(
                "Invalid Azure path format: {}",
                path
            )))
        }
    }
}

impl AzureClient {
    async fn new(config: AzureConfig) -> FsResult<Self> {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| FilesystemError::Network(e.to_string()))?;

        Ok(Self {
            config,
            http_client,
        })
    }

    /// Get Azure blob
    async fn get_blob(
        &self,
        container: &str,
        blob_path: &str,
        credentials: &AzureCredentials,
    ) -> FsResult<Vec<u8>> {
        let url = if self.config.hierarchical_namespace {
            // Data Lake Gen2 API
            format!(
                "https://{}.dfs.core.windows.net/{}/{}",
                self.config.account_name, container, blob_path
            )
        } else {
            // Blob Storage API
            format!(
                "https://{}.blob.core.windows.net/{}/{}",
                self.config.account_name, container, blob_path
            )
        };

        let response = self
            .http_client
            .get(&url)
            .header(
                "Authorization",
                format!("Bearer {}", credentials.access_token),
            )
            .header("x-ms-version", "2020-04-08")
            .send()
            .await
            .map_err(|e| FilesystemError::Network(e.to_string()))?;

        if response.status().is_success() {
            response
                .bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| FilesystemError::Network(e.to_string()))
        } else if response.status().as_u16() == 404 {
            Err(FilesystemError::NotFound(format!(
                "adls://{}/{}",
                container, blob_path
            )))
        } else {
            Err(FilesystemError::Network(format!(
                "Azure error: {}",
                response.status()
            )))
        }
    }

    /// Put Azure blob
    async fn put_blob(
        &self,
        container: &str,
        blob_path: &str,
        data: &[u8],
        credentials: &AzureCredentials,
        options: Option<FileOptions>,
    ) -> FsResult<()> {
        let url = if self.config.hierarchical_namespace {
            format!(
                "https://{}.dfs.core.windows.net/{}/{}",
                self.config.account_name, container, blob_path
            )
        } else {
            format!(
                "https://{}.blob.core.windows.net/{}/{}",
                self.config.account_name, container, blob_path
            )
        };

        let options = options.unwrap_or_default();

        let mut request = self
            .http_client
            .put(&url)
            .header(
                "Authorization",
                format!("Bearer {}", credentials.access_token),
            )
            .header("x-ms-version", "2020-04-08")
            .header("x-ms-blob-type", "BlockBlob")
            .body(data.to_vec());

        // Add blob tier if specified
        if let Some(storage_class) = options.storage_class {
            request = request.header("x-ms-access-tier", storage_class);
        } else {
            request = request.header("x-ms-access-tier", self.config.default_blob_tier.as_str());
        }

        // Add metadata if specified
        if let Some(metadata) = options.metadata {
            for (key, value) in metadata {
                request = request.header(format!("x-ms-meta-{}", key), value);
            }
        }

        let response = request
            .send()
            .await
            .map_err(|e| FilesystemError::Network(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(FilesystemError::Network(format!(
                "Azure PUT error: {}",
                response.status()
            )))
        }
    }
}

#[async_trait]
impl FileSystem for AzureFileSystem {
    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        let (container, blob_path) = self.parse_azure_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;
        self.client
            .get_blob(&container, &blob_path, &credentials)
            .await
    }

    async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()> {
        let (container, blob_path) = self.parse_azure_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;
        self.client
            .put_blob(&container, &blob_path, data, &credentials, options)
            .await
    }

    async fn append(&self, path: &str, data: &[u8]) -> FsResult<()> {
        // Azure Blob Storage doesn't support append natively for block blobs
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
        let (container, blob_path) = self.parse_azure_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;

        let url = format!(
            "https://{}.blob.core.windows.net/{}/{}",
            self.config.account_name, container, blob_path
        );

        let response = self
            .client
            .http_client
            .delete(&url)
            .header(
                "Authorization",
                format!("Bearer {}", credentials.access_token),
            )
            .header("x-ms-version", "2020-04-08")
            .send()
            .await
            .map_err(|e| FilesystemError::Network(e.to_string()))?;

        if response.status().is_success() || response.status().as_u16() == 404 {
            Ok(())
        } else {
            Err(FilesystemError::Network(format!(
                "Azure DELETE error: {}",
                response.status()
            )))
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
        let (container, blob_path) = self.parse_azure_url(path)?;
        let credentials = self.credential_provider.get_credentials().await?;

        let url = format!(
            "https://{}.blob.core.windows.net/{}/{}",
            self.config.account_name, container, blob_path
        );

        let response = self
            .client
            .http_client
            .head(&url)
            .header(
                "Authorization",
                format!("Bearer {}", credentials.access_token),
            )
            .header("x-ms-version", "2020-04-08")
            .send()
            .await
            .map_err(|e| FilesystemError::Network(e.to_string()))?;

        if response.status().is_success() {
            let size = response
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);

            let etag = response
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            let blob_tier = response
                .headers()
                .get("x-ms-access-tier")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            Ok(FileMetadata {
                path: format!("adls://{}/{}", container, blob_path),
                size,
                created: None,       // Would need to parse x-ms-creation-time header
                modified: None,      // Would need to parse Last-Modified header
                is_directory: false, // Azure blobs are always files
                permissions: None,
                etag,
                storage_class: blob_tier,
            })
        } else if response.status().as_u16() == 404 {
            Err(FilesystemError::NotFound(format!(
                "adls://{}/{}",
                container, blob_path
            )))
        } else {
            Err(FilesystemError::Network(format!(
                "Azure HEAD error: {}",
                response.status()
            )))
        }
    }

    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        // Azure blob listing implementation
        // This would use List Blobs API in production
        let (container, prefix) = self.parse_azure_url(path)?;
        let _credentials = self.credential_provider.get_credentials().await?;

        // Simplified implementation - in production use proper Azure List Blobs
        Ok(Vec::new()) // Placeholder
    }

    async fn create_dir(&self, _path: &str) -> FsResult<()> {
        // Azure doesn't have directories, this is a no-op
        Ok(())
    }

    async fn create_dir_all(&self, _path: &str) -> FsResult<()> {
        // Azure doesn't have directories, this is a no-op
        Ok(())
    }

    async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        // Azure Copy Blob implementation
        let data = self.read(from).await?;
        self.write(to, &data, None).await
    }

    async fn move_file(&self, from: &str, to: &str) -> FsResult<()> {
        self.copy(from, to).await?;
        self.delete(from).await
    }

    fn filesystem_type(&self) -> &'static str {
        "adls"
    }

    async fn sync(&self) -> FsResult<()> {
        // Azure operations are immediately durable
        Ok(())
    }
}

/// Simple credential providers for Azure

/// Account key credential provider
pub struct AccountKeyProvider {
    account_key: String,
}

impl AccountKeyProvider {
    pub fn new(account_key: String) -> Self {
        Self { account_key }
    }
}

#[async_trait]
impl AzureCredentialProvider for AccountKeyProvider {
    async fn get_credentials(&self) -> FsResult<AzureCredentials> {
        // In production, generate proper SAS token from account key
        Ok(AzureCredentials {
            account_name: "default".to_string(),
            access_token: format!("AccountKey:{}", self.account_key),
            expiration: None,
        })
    }

    async fn refresh_credentials(&self) -> FsResult<AzureCredentials> {
        self.get_credentials().await
    }
}

/// SAS token credential provider
pub struct SasTokenProvider {
    sas_token: String,
}

impl SasTokenProvider {
    pub fn new(sas_token: String) -> Self {
        Self { sas_token }
    }
}

#[async_trait]
impl AzureCredentialProvider for SasTokenProvider {
    async fn get_credentials(&self) -> FsResult<AzureCredentials> {
        Ok(AzureCredentials {
            account_name: "default".to_string(),
            access_token: self.sas_token.clone(),
            expiration: None, // SAS tokens have their own expiration
        })
    }

    async fn refresh_credentials(&self) -> FsResult<AzureCredentials> {
        self.get_credentials().await
    }
}

/// Service principal credential provider
pub struct ServicePrincipalProvider {
    tenant_id: String,
    client_id: String,
    client_secret: String,
    http_client: reqwest::Client,
}

impl ServicePrincipalProvider {
    pub fn new(tenant_id: String, client_id: String, client_secret: String) -> Self {
        Self {
            tenant_id,
            client_id,
            client_secret,
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl AzureCredentialProvider for ServicePrincipalProvider {
    async fn get_credentials(&self) -> FsResult<AzureCredentials> {
        let url = format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant_id
        );

        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
            ("scope", "https://storage.azure.com/.default"),
        ];

        let response = self
            .http_client
            .post(&url)
            .form(&params)
            .send()
            .await
            .map_err(|e| FilesystemError::Auth(format!("Failed to get Azure token: {}", e)))?;

        let token_json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| FilesystemError::Auth(format!("Failed to parse Azure token: {}", e)))?;

        let access_token = token_json["access_token"]
            .as_str()
            .ok_or_else(|| {
                FilesystemError::Auth("access_token not found in Azure response".to_string())
            })?
            .to_string();

        let expires_in = token_json["expires_in"].as_u64().unwrap_or(3600);
        let expiration = Some(Instant::now() + Duration::from_secs(expires_in));

        Ok(AzureCredentials {
            account_name: "default".to_string(),
            access_token,
            expiration,
        })
    }

    async fn refresh_credentials(&self) -> FsResult<AzureCredentials> {
        self.get_credentials().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_azure_url_parsing() {
        let config = AzureConfig {
            account_name: "testaccount".to_string(),
            default_container: Some("test-container".to_string()),
            ..Default::default()
        };

        let credential_provider = Box::new(AccountKeyProvider::new("test_key".to_string()));
        let client = AzureClient::new(config.clone()).await.unwrap();

        let fs = AzureFileSystem {
            config,
            credential_provider,
            client,
        };

        // Test with default container
        let (container, blob_path) = fs.parse_azure_url("/path/to/file.txt").unwrap();
        assert_eq!(container, "test-container");
        assert_eq!(blob_path, "path/to/file.txt");

        // Test with explicit container
        let (container, blob_path) = fs.parse_azure_url("my-container/path/to/file.txt").unwrap();
        assert_eq!(container, "my-container");
        assert_eq!(blob_path, "path/to/file.txt");
    }
}
