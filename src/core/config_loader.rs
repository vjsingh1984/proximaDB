use crate::core::Config;
use std::fs;
use std::path::Path;
use tracing::{info, warn, debug};

pub struct ConfigLoader;

impl ConfigLoader {
    /// Load configuration with default merging and cloud support
    /// 
    /// Supports URLs like:
    /// - file://path/to/config.toml (local filesystem)
    /// - s3://bucket/path/config.toml (AWS S3)
    /// - adls://account.dfs.core.windows.net/container/path/config.toml (Azure Data Lake)
    /// - gcs://bucket/path/config.toml (Google Cloud Storage)
    pub fn load_with_defaults<P: AsRef<str>>(config_path: P) -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
        let config_url = config_path.as_ref();
        
        info!("🔧 Loading configuration from: {}", config_url);
        
        // Determine if this is a cloud URL or local path
        if Self::is_cloud_url(config_url) {
            info!("☁️ Detected cloud config URL, loading from cloud storage");
            Self::load_cloud_config(config_url)
        } else {
            info!("📁 Loading local config file");
            Self::load_local_config(config_url)
        }
    }
    
    /// Check if the URL is a cloud storage URL
    fn is_cloud_url(url: &str) -> bool {
        url.starts_with("s3://") || 
        url.starts_with("adls://") || 
        url.starts_with("gcs://") ||
        url.starts_with("azure://")
    }
    
    /// Load configuration from cloud storage
    fn load_cloud_config(config_url: &str) -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
        use tokio::runtime::Runtime;
        
        let rt = Runtime::new()?;
        rt.block_on(async {
            Self::load_cloud_config_async(config_url).await
        })
    }
    
    /// Load configuration from cloud storage (async)
    async fn load_cloud_config_async(config_url: &str) -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
        // Parse the cloud URL
        let (provider, bucket, path) = Self::parse_cloud_url(config_url)?;
        
        info!("☁️ Loading config from {} bucket: {}, path: {}", provider, bucket, path);
        
        match provider.as_str() {
            "s3" => Self::load_s3_config(&bucket, &path).await,
            "gcs" => Self::load_gcs_config(&bucket, &path).await,
            "adls" | "azure" => Self::load_adls_config(&bucket, &path).await,
            _ => Err(format!("Unsupported cloud provider: {}", provider).into())
        }
    }
    
    /// Parse cloud URL into components
    fn parse_cloud_url(url: &str) -> Result<(String, String, String), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(s3_path) = url.strip_prefix("s3://") {
            let parts: Vec<&str> = s3_path.splitn(2, '/').collect();
            if parts.len() != 2 {
                return Err("Invalid S3 URL format. Expected: s3://bucket/path".into());
            }
            Ok(("s3".to_string(), parts[0].to_string(), parts[1].to_string()))
        } else if let Some(gcs_path) = url.strip_prefix("gcs://") {
            let parts: Vec<&str> = gcs_path.splitn(2, '/').collect();
            if parts.len() != 2 {
                return Err("Invalid GCS URL format. Expected: gcs://bucket/path".into());
            }
            Ok(("gcs".to_string(), parts[0].to_string(), parts[1].to_string()))
        } else if let Some(adls_path) = url.strip_prefix("adls://") {
            // Parse ADLS URL: adls://account.dfs.core.windows.net/container/path
            let parts: Vec<&str> = adls_path.splitn(2, '/').collect();
            if parts.len() != 2 {
                return Err("Invalid ADLS URL format. Expected: adls://account.dfs.core.windows.net/container/path".into());
            }
            let account_and_container: Vec<&str> = parts[1].splitn(2, '/').collect();
            if account_and_container.len() != 2 {
                return Err("Invalid ADLS URL format. Expected: adls://account.dfs.core.windows.net/container/path".into());
            }
            Ok(("adls".to_string(), parts[0].to_string() + "/" + account_and_container[0], account_and_container[1].to_string()))
        } else if let Some(azure_path) = url.strip_prefix("azure://") {
            let parts: Vec<&str> = azure_path.splitn(2, '/').collect();
            if parts.len() != 2 {
                return Err("Invalid Azure URL format. Expected: azure://container/path".into());
            }
            Ok(("azure".to_string(), parts[0].to_string(), parts[1].to_string()))
        } else {
            Err("Unsupported cloud URL format".into())
        }
    }
    
    /// Load configuration from AWS S3
    async fn load_s3_config(bucket: &str, path: &str) -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
        info!("🪣 Loading S3 config from bucket: {}, path: {}", bucket, path);
        
        // For now, return a basic implementation
        // In production, this would use the AWS SDK to fetch the config
        warn!("⚠️ S3 config loading not fully implemented yet, using defaults");
        
        // TODO: Implement S3 config loading using aws-sdk-s3
        // Example implementation:
        // let config = aws_config::load_from_env().await;
        // let client = aws_sdk_s3::Client::new(&config);
        // let resp = client.get_object().bucket(bucket).key(path).send().await?;
        // let data = resp.body.collect().await?.into_bytes();
        // let config_str = String::from_utf8(data.to_vec())?;
        // let config: Config = toml::from_str(&config_str)?;
        
        Ok(Config::default())
    }
    
    /// Load configuration from Google Cloud Storage
    async fn load_gcs_config(bucket: &str, path: &str) -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
        info!("🟦 Loading GCS config from bucket: {}, path: {}", bucket, path);
        
        // For now, return a basic implementation
        // In production, this would use the Google Cloud SDK
        warn!("⚠️ GCS config loading not fully implemented yet, using defaults");
        
        // TODO: Implement GCS config loading using google-cloud-storage
        // Example implementation:
        // let config = ClientConfig::default().with_auth().await?;
        // let client = Client::new(config);
        // let data = client.download_object(&DownloadObjectRequest {
        //     bucket: bucket.to_string(),
        //     object: path.to_string(),
        //     ..Default::default()
        // }).await?;
        // let config_str = String::from_utf8(data)?;
        // let config: Config = toml::from_str(&config_str)?;
        
        Ok(Config::default())
    }
    
    /// Load configuration from Azure Data Lake Storage
    async fn load_adls_config(container: &str, path: &str) -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
        info!("🟢 Loading ADLS config from container: {}, path: {}", container, path);
        
        // For now, return a basic implementation
        // In production, this would use the Azure SDK
        warn!("⚠️ ADLS config loading not fully implemented yet, using defaults");
        
        // TODO: Implement ADLS config loading using azure-storage
        // Example implementation:
        // let account = std::env::var("AZURE_STORAGE_ACCOUNT")?;
        // let access_key = std::env::var("AZURE_STORAGE_ACCESS_KEY")?;
        // let storage_credentials = StorageCredentials::access_key(account.clone(), access_key);
        // let service_client = BlobServiceClient::new(&account, storage_credentials);
        // let container_client = service_client.container_client(container);
        // let blob_client = container_client.blob_client(path);
        // let response = blob_client.get().into_stream().next().await.unwrap()?;
        // let data = response.data.collect().await?;
        // let config_str = String::from_utf8(data)?;
        // let config: Config = toml::from_str(&config_str)?;
        
        Ok(Config::default())
    }
    
    /// Load configuration from local filesystem
    fn load_local_config(config_path: &str) -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
        let path = Path::new(config_path);
        
        // If user config exists, use it
        if path.exists() {
            debug!("📄 Reading local config file: {}", config_path);
            let config_str = fs::read_to_string(path)?;
            let config: Config = toml::from_str(&config_str)?;
            info!("✅ Successfully loaded config from: {}", config_path);
            Ok(config)
        } else {
            // Return default config if file doesn't exist
            warn!("⚠️ Config file not found: {}, using defaults", config_path);
            Ok(Config::default())
        }
    }
    
    /// Load configuration with proper error handling (legacy method)
    pub fn load_config<P: AsRef<Path>>(config_path: P) -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
        if config_path.as_ref().exists() {
            let config_str = fs::read_to_string(config_path)?;
            let config: Config = toml::from_str(&config_str)?;
            Ok(config)
        } else {
            // Return default config if file doesn't exist
            Ok(Config::default())
        }
    }
    
    /// Get cloud authentication configuration
    pub fn get_cloud_auth_info() -> String {
        let mut auth_info = Vec::new();
        
        // AWS authentication
        if std::env::var("AWS_ACCESS_KEY_ID").is_ok() && std::env::var("AWS_SECRET_ACCESS_KEY").is_ok() {
            auth_info.push("AWS: Access Key + Secret Key");
        } else if std::env::var("AWS_PROFILE").is_ok() {
            auth_info.push("AWS: Profile-based");
        } else {
            auth_info.push("AWS: Instance Role/Default");
        }
        
        // Azure authentication
        if std::env::var("AZURE_STORAGE_ACCOUNT").is_ok() && std::env::var("AZURE_STORAGE_ACCESS_KEY").is_ok() {
            auth_info.push("Azure: Storage Account + Access Key");
        } else if std::env::var("AZURE_CLIENT_ID").is_ok() {
            auth_info.push("Azure: Service Principal");
        } else {
            auth_info.push("Azure: Managed Identity");
        }
        
        // GCP authentication
        if std::env::var("GOOGLE_APPLICATION_CREDENTIALS").is_ok() {
            auth_info.push("GCP: Service Account JSON");
        } else {
            auth_info.push("GCP: Default Application Credentials");
        }
        
        format!("🔐 Cloud Authentication: {}", auth_info.join(", "))
    }
}