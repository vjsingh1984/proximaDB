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

//! Cloud authentication providers for AWS, Azure, and GCS with credential renewal

use std::env;
use async_trait::async_trait;
use tokio::time::{Duration, Instant};

use super::{FsResult, FilesystemError};

/// AWS credentials
#[derive(Debug, Clone)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub expiration: Option<Instant>,
}

/// Azure credentials
#[derive(Debug, Clone)]
pub struct AzureCredentials {
    pub account_name: String,
    pub access_token: String,
    pub expiration: Option<Instant>,
}

/// GCS credentials
#[derive(Debug, Clone)]
pub struct GcsCredentials {
    pub access_token: String,
    pub project_id: String,
    pub expiration: Option<Instant>,
}

/// Generic credential provider trait
#[async_trait]
pub trait CredentialProvider: Send + Sync {
    async fn get_credentials(&self) -> FsResult<AwsCredentials>;
    async fn refresh_credentials(&self) -> FsResult<AwsCredentials>;
}

/// Azure credential provider trait
#[async_trait]
pub trait AzureCredentialProvider: Send + Sync {
    async fn get_credentials(&self) -> FsResult<AzureCredentials>;
    async fn refresh_credentials(&self) -> FsResult<AzureCredentials>;
}

/// GCS credential provider trait  
#[async_trait]
pub trait GcsCredentialProvider: Send + Sync {
    async fn get_credentials(&self) -> FsResult<GcsCredentials>;
    async fn refresh_credentials(&self) -> FsResult<GcsCredentials>;
}

/// Static AWS credential provider
pub struct StaticCredentialProvider {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

impl StaticCredentialProvider {
    pub fn new(access_key_id: String, secret_access_key: String, session_token: Option<String>) -> Self {
        Self {
            access_key_id,
            secret_access_key,
            session_token,
        }
    }
}

#[async_trait]
impl CredentialProvider for StaticCredentialProvider {
    async fn get_credentials(&self) -> FsResult<AwsCredentials> {
        Ok(AwsCredentials {
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            session_token: self.session_token.clone(),
            expiration: None, // Static credentials don't expire
        })
    }
    
    async fn refresh_credentials(&self) -> FsResult<AwsCredentials> {
        self.get_credentials().await
    }
}

/// Environment variable credential provider
pub struct EnvironmentCredentialProvider;

impl EnvironmentCredentialProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CredentialProvider for EnvironmentCredentialProvider {
    async fn get_credentials(&self) -> FsResult<AwsCredentials> {
        let access_key_id = env::var("AWS_ACCESS_KEY_ID")
            .map_err(|_| FilesystemError::Auth("AWS_ACCESS_KEY_ID not found in environment".to_string()))?;
        
        let secret_access_key = env::var("AWS_SECRET_ACCESS_KEY")
            .map_err(|_| FilesystemError::Auth("AWS_SECRET_ACCESS_KEY not found in environment".to_string()))?;
        
        let session_token = env::var("AWS_SESSION_TOKEN").ok();
        
        Ok(AwsCredentials {
            access_key_id,
            secret_access_key,
            session_token,
            expiration: None,
        })
    }
    
    async fn refresh_credentials(&self) -> FsResult<AwsCredentials> {
        self.get_credentials().await
    }
}

/// EC2 Instance Metadata Service credential provider
pub struct InstanceMetadataProvider {
    http_client: reqwest::Client,
}

impl InstanceMetadataProvider {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::new(),
        }
    }
    
    async fn get_iam_security_credentials(&self) -> FsResult<AwsCredentials> {
        // IMDSv2 implementation
        let token_url = "http://169.254.169.254/latest/api/token";
        let token_response = self.http_client
            .put(token_url)
            .header("X-aws-ec2-metadata-token-ttl-seconds", "21600")
            .send()
            .await
            .map_err(|e| FilesystemError::Auth(format!("Failed to get IMDSv2 token: {}", e)))?;
        
        let token = token_response.text().await
            .map_err(|e| FilesystemError::Auth(format!("Failed to read IMDSv2 token: {}", e)))?;
        
        // Get role name
        let role_url = "http://169.254.169.254/latest/meta-data/iam/security-credentials/";
        let role_response = self.http_client
            .get(role_url)
            .header("X-aws-ec2-metadata-token", &token)
            .send()
            .await
            .map_err(|e| FilesystemError::Auth(format!("Failed to get IAM role: {}", e)))?;
        
        let role_name = role_response.text().await
            .map_err(|e| FilesystemError::Auth(format!("Failed to read IAM role: {}", e)))?;
        
        // Get credentials
        let creds_url = format!("{}{}", role_url, role_name.trim());
        let creds_response = self.http_client
            .get(&creds_url)
            .header("X-aws-ec2-metadata-token", &token)
            .send()
            .await
            .map_err(|e| FilesystemError::Auth(format!("Failed to get credentials: {}", e)))?;
        
        let creds_json: serde_json::Value = creds_response.json().await
            .map_err(|e| FilesystemError::Auth(format!("Failed to parse credentials: {}", e)))?;
        
        let access_key_id = creds_json["AccessKeyId"].as_str()
            .ok_or_else(|| FilesystemError::Auth("AccessKeyId not found in credentials".to_string()))?
            .to_string();
        
        let secret_access_key = creds_json["SecretAccessKey"].as_str()
            .ok_or_else(|| FilesystemError::Auth("SecretAccessKey not found in credentials".to_string()))?
            .to_string();
        
        let session_token = creds_json["Token"].as_str().map(|s| s.to_string());
        
        // Parse expiration
        let expiration = creds_json["Expiration"].as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| Instant::now() + Duration::from_secs((dt.timestamp() - chrono::Utc::now().timestamp()) as u64));
        
        Ok(AwsCredentials {
            access_key_id,
            secret_access_key,
            session_token,
            expiration,
        })
    }
}

#[async_trait]
impl CredentialProvider for InstanceMetadataProvider {
    async fn get_credentials(&self) -> FsResult<AwsCredentials> {
        self.get_iam_security_credentials().await
    }
    
    async fn refresh_credentials(&self) -> FsResult<AwsCredentials> {
        self.get_iam_security_credentials().await
    }
}

/// ECS Task Metadata Service credential provider
pub struct EcsTaskMetadataProvider {
    http_client: reqwest::Client,
}

impl EcsTaskMetadataProvider {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl CredentialProvider for EcsTaskMetadataProvider {
    async fn get_credentials(&self) -> FsResult<AwsCredentials> {
        let relative_uri = env::var("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
            .map_err(|_| FilesystemError::Auth("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI not found".to_string()))?;
        
        let url = format!("http://169.254.170.2{}", relative_uri);
        
        let response = self.http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| FilesystemError::Auth(format!("Failed to get ECS credentials: {}", e)))?;
        
        let creds_json: serde_json::Value = response.json().await
            .map_err(|e| FilesystemError::Auth(format!("Failed to parse ECS credentials: {}", e)))?;
        
        let access_key_id = creds_json["AccessKeyId"].as_str()
            .ok_or_else(|| FilesystemError::Auth("AccessKeyId not found in ECS credentials".to_string()))?
            .to_string();
        
        let secret_access_key = creds_json["SecretAccessKey"].as_str()
            .ok_or_else(|| FilesystemError::Auth("SecretAccessKey not found in ECS credentials".to_string()))?
            .to_string();
        
        let session_token = creds_json["Token"].as_str().map(|s| s.to_string());
        
        let expiration = creds_json["Expiration"].as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| Instant::now() + Duration::from_secs((dt.timestamp() - chrono::Utc::now().timestamp()) as u64));
        
        Ok(AwsCredentials {
            access_key_id,
            secret_access_key,
            session_token,
            expiration,
        })
    }
    
    async fn refresh_credentials(&self) -> FsResult<AwsCredentials> {
        self.get_credentials().await
    }
}

/// STS Assume Role credential provider
pub struct AssumeRoleProvider {
    role_arn: String,
    external_id: Option<String>,
    http_client: reqwest::Client,
}

impl AssumeRoleProvider {
    pub fn new(role_arn: String, external_id: Option<String>) -> Self {
        Self {
            role_arn,
            external_id,
            http_client: reqwest::Client::new(),
        }
    }
    
    async fn assume_role(&self) -> FsResult<AwsCredentials> {
        // In production, implement proper STS AssumeRole API call
        // This is a placeholder implementation
        Err(FilesystemError::Auth("AssumeRole not fully implemented".to_string()))
    }
}

#[async_trait]
impl CredentialProvider for AssumeRoleProvider {
    async fn get_credentials(&self) -> FsResult<AwsCredentials> {
        self.assume_role().await
    }
    
    async fn refresh_credentials(&self) -> FsResult<AwsCredentials> {
        self.assume_role().await
    }
}

/// AWS Profile credential provider
pub struct ProfileCredentialProvider {
    profile_name: String,
}

impl ProfileCredentialProvider {
    pub fn new(profile_name: String) -> Self {
        Self { profile_name }
    }
}

#[async_trait]
impl CredentialProvider for ProfileCredentialProvider {
    async fn get_credentials(&self) -> FsResult<AwsCredentials> {
        // In production, parse ~/.aws/credentials file
        Err(FilesystemError::Auth("Profile credentials not implemented".to_string()))
    }
    
    async fn refresh_credentials(&self) -> FsResult<AwsCredentials> {
        self.get_credentials().await
    }
}

/// Chain credential provider (tries multiple providers in order)
pub struct ChainCredentialProvider {
    providers: Vec<Box<dyn CredentialProvider>>,
}

impl ChainCredentialProvider {
    pub fn new() -> Self {
        let providers: Vec<Box<dyn CredentialProvider>> = vec![
            Box::new(EnvironmentCredentialProvider::new()),
            Box::new(InstanceMetadataProvider::new()),
            Box::new(EcsTaskMetadataProvider::new()),
        ];
        
        Self { providers }
    }
}

#[async_trait]
impl CredentialProvider for ChainCredentialProvider {
    async fn get_credentials(&self) -> FsResult<AwsCredentials> {
        for provider in &self.providers {
            if let Ok(credentials) = provider.get_credentials().await {
                return Ok(credentials);
            }
        }
        
        Err(FilesystemError::Auth(
            "No credential provider in chain succeeded".to_string()
        ))
    }
    
    async fn refresh_credentials(&self) -> FsResult<AwsCredentials> {
        self.get_credentials().await
    }
}

/// Azure Managed Identity credential provider
pub struct AzureManagedIdentityProvider {
    client_id: Option<String>,
    http_client: reqwest::Client,
}

impl AzureManagedIdentityProvider {
    pub fn new(client_id: Option<String>) -> Self {
        Self {
            client_id,
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl AzureCredentialProvider for AzureManagedIdentityProvider {
    async fn get_credentials(&self) -> FsResult<AzureCredentials> {
        let url = "http://169.254.169.254/metadata/identity/oauth2/token";
        
        let mut request = self.http_client
            .get(url)
            .header("Metadata", "true")
            .query(&[("api-version", "2018-02-01"), ("resource", "https://storage.azure.com/")]);
        
        if let Some(ref client_id) = self.client_id {
            request = request.query(&[("client_id", client_id)]);
        }
        
        let response = request.send().await
            .map_err(|e| FilesystemError::Auth(format!("Failed to get Azure token: {}", e)))?;
        
        let token_json: serde_json::Value = response.json().await
            .map_err(|e| FilesystemError::Auth(format!("Failed to parse Azure token: {}", e)))?;
        
        let access_token = token_json["access_token"].as_str()
            .ok_or_else(|| FilesystemError::Auth("access_token not found in Azure response".to_string()))?
            .to_string();
        
        // Extract account name from environment or metadata
        let account_name = env::var("AZURE_STORAGE_ACCOUNT")
            .unwrap_or_else(|_| "default".to_string());
        
        let expires_in = token_json["expires_in"].as_u64().unwrap_or(3600);
        let expiration = Some(Instant::now() + Duration::from_secs(expires_in));
        
        Ok(AzureCredentials {
            account_name,
            access_token,
            expiration,
        })
    }
    
    async fn refresh_credentials(&self) -> FsResult<AzureCredentials> {
        self.get_credentials().await
    }
}

/// Google Cloud Application Default Credentials provider
pub struct GcsApplicationDefaultProvider {
    http_client: reqwest::Client,
}

impl GcsApplicationDefaultProvider {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl GcsCredentialProvider for GcsApplicationDefaultProvider {
    async fn get_credentials(&self) -> FsResult<GcsCredentials> {
        let url = "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
        
        let response = self.http_client
            .get(url)
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| FilesystemError::Auth(format!("Failed to get GCS token: {}", e)))?;
        
        let token_json: serde_json::Value = response.json().await
            .map_err(|e| FilesystemError::Auth(format!("Failed to parse GCS token: {}", e)))?;
        
        let access_token = token_json["access_token"].as_str()
            .ok_or_else(|| FilesystemError::Auth("access_token not found in GCS response".to_string()))?
            .to_string();
        
        let project_id = env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| env::var("GCLOUD_PROJECT"))
            .unwrap_or_else(|_| "default".to_string());
        
        let expires_in = token_json["expires_in"].as_u64().unwrap_or(3600);
        let expiration = Some(Instant::now() + Duration::from_secs(expires_in));
        
        Ok(GcsCredentials {
            access_token,
            project_id,
            expiration,
        })
    }
    
    async fn refresh_credentials(&self) -> FsResult<GcsCredentials> {
        self.get_credentials().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_static_credential_provider() {
        let provider = StaticCredentialProvider::new(
            "test_access_key".to_string(),
            "test_secret_key".to_string(),
            Some("test_session_token".to_string()),
        );
        
        let credentials = provider.get_credentials().await.unwrap();
        assert_eq!(credentials.access_key_id, "test_access_key");
        assert_eq!(credentials.secret_access_key, "test_secret_key");
        assert_eq!(credentials.session_token, Some("test_session_token".to_string()));
        assert!(credentials.expiration.is_none());
    }
    
    #[tokio::test] 
    async fn test_environment_credential_provider() {
        // Set environment variables for test
        env::set_var("AWS_ACCESS_KEY_ID", "env_access_key");
        env::set_var("AWS_SECRET_ACCESS_KEY", "env_secret_key");
        env::set_var("AWS_SESSION_TOKEN", "env_session_token");
        
        let provider = EnvironmentCredentialProvider::new();
        let credentials = provider.get_credentials().await.unwrap();
        
        assert_eq!(credentials.access_key_id, "env_access_key");
        assert_eq!(credentials.secret_access_key, "env_secret_key");
        assert_eq!(credentials.session_token, Some("env_session_token".to_string()));
        
        // Clean up
        env::remove_var("AWS_ACCESS_KEY_ID");
        env::remove_var("AWS_SECRET_ACCESS_KEY");
        env::remove_var("AWS_SESSION_TOKEN");
    }
}