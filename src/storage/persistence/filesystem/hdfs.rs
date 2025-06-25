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

//! Hadoop Distributed File System (HDFS) implementation with WebHDFS REST API

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::time::Duration;

use super::{DirEntry, FileMetadata, FileOptions, FileSystem, FilesystemError, FsResult};

/// HDFS authentication types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HdfsAuthType {
    /// No authentication
    Simple,
    /// Kerberos authentication
    Kerberos,
    /// Username authentication
    PseudoAuth,
}

/// HDFS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HdfsConfig {
    /// HDFS NameNode URL (e.g., "http://namenode:9870")
    pub namenode_url: String,

    /// HDFS user name
    pub user: String,

    /// Authentication type
    pub auth_type: HdfsAuthType,

    /// Kerberos principal (for Kerberos auth)
    pub kerberos_principal: Option<String>,

    /// Kerberos keytab file path (for Kerberos auth)
    pub kerberos_keytab: Option<String>,

    /// Request timeout in seconds
    pub timeout_seconds: u64,

    /// Maximum retry attempts
    pub max_retries: u32,

    /// HDFS block size for new files (bytes)
    pub block_size: u64,

    /// HDFS replication factor
    pub replication: u16,

    /// Buffer size for file operations (bytes)
    pub buffer_size: u32,
}

impl Default for HdfsConfig {
    fn default() -> Self {
        Self {
            namenode_url: "http://localhost:9870".to_string(),
            user: "proximadb".to_string(),
            auth_type: HdfsAuthType::Simple,
            kerberos_principal: None,
            kerberos_keytab: None,
            timeout_seconds: 30,
            max_retries: 3,
            block_size: 128 * 1024 * 1024, // 128MB (default HDFS block size)
            replication: 3,                // Default HDFS replication
            buffer_size: 4096,             // 4KB buffer
        }
    }
}

/// HDFS filesystem implementation using WebHDFS REST API
pub struct HdfsFileSystem {
    config: HdfsConfig,
    client: HdfsClient,
}

impl std::fmt::Debug for HdfsFileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HdfsFileSystem")
            .field("config", &self.config)
            .field("client", &"<hdfs_client>")
            .finish()
    }
}

/// HDFS WebHDFS REST API client
struct HdfsClient {
    config: HdfsConfig,
    http_client: reqwest::Client,
    base_url: String,
}

impl HdfsFileSystem {
    /// Create new HDFS filesystem instance
    pub async fn new(config: HdfsConfig) -> FsResult<Self> {
        tracing::info!(
            "🐘 Initializing HDFS filesystem for NameNode: {}",
            config.namenode_url
        );

        // Initialize HDFS client
        let client = HdfsClient::new(config.clone()).await?;

        // Test connectivity
        client.test_connection().await?;

        tracing::info!("✅ HDFS filesystem initialized successfully");

        Ok(Self { config, client })
    }

    /// Normalize HDFS path (ensure it starts with /)
    fn normalize_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        }
    }
}

impl HdfsClient {
    async fn new(config: HdfsConfig) -> FsResult<Self> {
        tracing::debug!(
            "🌐 Creating HDFS WebHDFS client with timeout: {}s",
            config.timeout_seconds
        );

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| FilesystemError::Network(e.to_string()))?;

        // Ensure NameNode URL has proper WebHDFS prefix
        let base_url = if config.namenode_url.contains("/webhdfs/v1") {
            config.namenode_url.clone()
        } else {
            format!("{}/webhdfs/v1", config.namenode_url.trim_end_matches('/'))
        };

        Ok(Self {
            config,
            http_client,
            base_url,
        })
    }

    /// Test HDFS connectivity
    async fn test_connection(&self) -> FsResult<()> {
        tracing::debug!("🔍 Testing HDFS connectivity");

        let url = format!(
            "{}/?op=LISTSTATUS&user.name={}",
            self.base_url, self.config.user
        );

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            tracing::error!("❌ HDFS connectivity test failed: {}", e);
            FilesystemError::Network(format!("HDFS connectivity test failed: {}", e))
        })?;

        if response.status().is_success() {
            tracing::debug!("✅ HDFS connectivity test successful");
            Ok(())
        } else {
            tracing::error!("❌ HDFS connectivity test failed: {}", response.status());
            Err(FilesystemError::Network(format!(
                "HDFS connectivity test failed: {}",
                response.status()
            )))
        }
    }

    /// Read file from HDFS
    async fn read_file(&self, path: &str) -> FsResult<Vec<u8>> {
        let url = format!(
            "{}/{}?op=OPEN&user.name={}",
            self.base_url,
            path.trim_start_matches('/'),
            self.config.user
        );

        tracing::debug!("📥 Reading HDFS file: {}", url);

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            tracing::error!("❌ HDFS read request failed: {}", e);
            FilesystemError::Network(e.to_string())
        })?;

        if response.status().is_success() {
            let data = response
                .bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| FilesystemError::Network(e.to_string()))?;

            tracing::debug!("✅ Successfully read {} bytes from HDFS", data.len());
            Ok(data)
        } else if response.status().as_u16() == 404 {
            tracing::warn!("🔍 HDFS file not found: {}", path);
            Err(FilesystemError::NotFound(format!("hdfs://{}", path)))
        } else {
            tracing::error!("❌ HDFS read error: {}", response.status());
            Err(FilesystemError::Network(format!(
                "HDFS read error: {}",
                response.status()
            )))
        }
    }

    /// Write file to HDFS
    async fn write_file(
        &self,
        path: &str,
        data: &[u8],
        options: Option<FileOptions>,
    ) -> FsResult<()> {
        let options = options.unwrap_or_default();

        // Step 1: Create file (WebHDFS two-step process)
        let create_url = format!(
            "{}/{}?op=CREATE&user.name={}&blocksize={}&replication={}&overwrite={}",
            self.base_url,
            path.trim_start_matches('/'),
            self.config.user,
            self.config.block_size,
            self.config.replication,
            options.overwrite
        );

        tracing::debug!("📤 Creating HDFS file: {} ({} bytes)", path, data.len());

        let create_response = self
            .http_client
            .put(&create_url)
            .send()
            .await
            .map_err(|e| {
                tracing::error!("❌ HDFS create request failed: {}", e);
                FilesystemError::Network(e.to_string())
            })?;

        if create_response.status().as_u16() == 307 {
            // Step 2: Follow redirect to DataNode
            if let Some(location) = create_response.headers().get("location") {
                let datanode_url = location.to_str().map_err(|e| {
                    FilesystemError::Network(format!("Invalid location header: {}", e))
                })?;

                tracing::debug!("📤 Writing data to DataNode: {}", datanode_url);

                let write_response = self
                    .http_client
                    .put(datanode_url)
                    .body(data.to_vec())
                    .send()
                    .await
                    .map_err(|e| {
                        tracing::error!("❌ HDFS write request failed: {}", e);
                        FilesystemError::Network(e.to_string())
                    })?;

                if write_response.status().is_success() {
                    tracing::debug!("✅ Successfully wrote to HDFS");
                    Ok(())
                } else {
                    tracing::error!("❌ HDFS write error: {}", write_response.status());
                    Err(FilesystemError::Network(format!(
                        "HDFS write error: {}",
                        write_response.status()
                    )))
                }
            } else {
                Err(FilesystemError::Network(
                    "No location header in HDFS create response".to_string(),
                ))
            }
        } else {
            tracing::error!("❌ HDFS create error: {}", create_response.status());
            Err(FilesystemError::Network(format!(
                "HDFS create error: {}",
                create_response.status()
            )))
        }
    }

    /// Append to HDFS file
    async fn append_file(&self, path: &str, data: &[u8]) -> FsResult<()> {
        // Step 1: Append request
        let append_url = format!(
            "{}/{}?op=APPEND&user.name={}",
            self.base_url,
            path.trim_start_matches('/'),
            self.config.user
        );

        tracing::debug!("➕ Appending to HDFS file: {} ({} bytes)", path, data.len());

        let append_response = self
            .http_client
            .post(&append_url)
            .send()
            .await
            .map_err(|e| {
                tracing::error!("❌ HDFS append request failed: {}", e);
                FilesystemError::Network(e.to_string())
            })?;

        if append_response.status().as_u16() == 307 {
            // Step 2: Follow redirect to DataNode
            if let Some(location) = append_response.headers().get("location") {
                let datanode_url = location.to_str().map_err(|e| {
                    FilesystemError::Network(format!("Invalid location header: {}", e))
                })?;

                let write_response = self
                    .http_client
                    .post(datanode_url)
                    .body(data.to_vec())
                    .send()
                    .await
                    .map_err(|e| {
                        tracing::error!("❌ HDFS append write failed: {}", e);
                        FilesystemError::Network(e.to_string())
                    })?;

                if write_response.status().is_success() {
                    tracing::debug!("✅ Successfully appended to HDFS");
                    Ok(())
                } else {
                    tracing::error!("❌ HDFS append write error: {}", write_response.status());
                    Err(FilesystemError::Network(format!(
                        "HDFS append write error: {}",
                        write_response.status()
                    )))
                }
            } else {
                Err(FilesystemError::Network(
                    "No location header in HDFS append response".to_string(),
                ))
            }
        } else {
            tracing::error!("❌ HDFS append error: {}", append_response.status());
            Err(FilesystemError::Network(format!(
                "HDFS append error: {}",
                append_response.status()
            )))
        }
    }

    /// Delete HDFS file or directory
    async fn delete_path(&self, path: &str) -> FsResult<()> {
        let url = format!(
            "{}/{}?op=DELETE&user.name={}&recursive=true",
            self.base_url,
            path.trim_start_matches('/'),
            self.config.user
        );

        tracing::debug!("🗑️ Deleting HDFS path: {}", path);

        let response = self.http_client.delete(&url).send().await.map_err(|e| {
            tracing::error!("❌ HDFS delete request failed: {}", e);
            FilesystemError::Network(e.to_string())
        })?;

        if response.status().is_success() {
            tracing::debug!("✅ HDFS delete successful");
            Ok(())
        } else if response.status().as_u16() == 404 {
            tracing::debug!("✅ HDFS path already deleted or doesn't exist");
            Ok(())
        } else {
            tracing::error!("❌ HDFS delete error: {}", response.status());
            Err(FilesystemError::Network(format!(
                "HDFS delete error: {}",
                response.status()
            )))
        }
    }

    /// Get HDFS file status/metadata
    async fn get_file_status(&self, path: &str) -> FsResult<serde_json::Value> {
        let url = format!(
            "{}/{}?op=GETFILESTATUS&user.name={}",
            self.base_url,
            path.trim_start_matches('/'),
            self.config.user
        );

        tracing::debug!("📊 Getting HDFS file status: {}", path);

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            tracing::error!("❌ HDFS status request failed: {}", e);
            FilesystemError::Network(e.to_string())
        })?;

        if response.status().is_success() {
            let status_json: serde_json::Value = response
                .json()
                .await
                .map_err(|e| FilesystemError::Network(e.to_string()))?;

            Ok(status_json)
        } else if response.status().as_u16() == 404 {
            tracing::warn!("🔍 HDFS file status not found: {}", path);
            Err(FilesystemError::NotFound(format!("hdfs://{}", path)))
        } else {
            tracing::error!("❌ HDFS status error: {}", response.status());
            Err(FilesystemError::Network(format!(
                "HDFS status error: {}",
                response.status()
            )))
        }
    }

    /// List HDFS directory
    async fn list_directory(&self, path: &str) -> FsResult<Vec<serde_json::Value>> {
        let url = format!(
            "{}/{}?op=LISTSTATUS&user.name={}",
            self.base_url,
            path.trim_start_matches('/'),
            self.config.user
        );

        tracing::debug!("📋 Listing HDFS directory: {}", path);

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            tracing::error!("❌ HDFS list request failed: {}", e);
            FilesystemError::Network(e.to_string())
        })?;

        if response.status().is_success() {
            let list_json: serde_json::Value = response
                .json()
                .await
                .map_err(|e| FilesystemError::Network(e.to_string()))?;

            let file_statuses = list_json["FileStatuses"]["FileStatus"]
                .as_array()
                .unwrap_or(&vec![])
                .clone();

            tracing::debug!(
                "✅ Listed {} entries in HDFS directory",
                file_statuses.len()
            );
            Ok(file_statuses)
        } else {
            tracing::error!("❌ HDFS list error: {}", response.status());
            Err(FilesystemError::Network(format!(
                "HDFS list error: {}",
                response.status()
            )))
        }
    }

    /// Create HDFS directory
    async fn create_directory(&self, path: &str) -> FsResult<()> {
        let url = format!(
            "{}/{}?op=MKDIRS&user.name={}",
            self.base_url,
            path.trim_start_matches('/'),
            self.config.user
        );

        tracing::debug!("📁 Creating HDFS directory: {}", path);

        let response = self.http_client.put(&url).send().await.map_err(|e| {
            tracing::error!("❌ HDFS mkdir request failed: {}", e);
            FilesystemError::Network(e.to_string())
        })?;

        if response.status().is_success() {
            tracing::debug!("✅ HDFS directory created successfully");
            Ok(())
        } else {
            tracing::error!("❌ HDFS mkdir error: {}", response.status());
            Err(FilesystemError::Network(format!(
                "HDFS mkdir error: {}",
                response.status()
            )))
        }
    }
}

#[async_trait]
impl FileSystem for HdfsFileSystem {
    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        tracing::debug!("📖 HDFS read: {}", path);
        let normalized_path = self.normalize_path(path);
        self.client.read_file(&normalized_path).await
    }

    async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()> {
        tracing::debug!("📝 HDFS write: {} ({} bytes)", path, data.len());
        let normalized_path = self.normalize_path(path);
        self.client
            .write_file(&normalized_path, data, options)
            .await
    }

    async fn append(&self, path: &str, data: &[u8]) -> FsResult<()> {
        tracing::debug!("➕ HDFS append: {} ({} bytes)", path, data.len());
        let normalized_path = self.normalize_path(path);
        self.client.append_file(&normalized_path, data).await
    }

    async fn delete(&self, path: &str) -> FsResult<()> {
        tracing::debug!("🗑️ HDFS delete: {}", path);
        let normalized_path = self.normalize_path(path);
        self.client.delete_path(&normalized_path).await
    }

    async fn exists(&self, path: &str) -> FsResult<bool> {
        tracing::trace!("🔍 HDFS exists check: {}", path);
        match self.metadata(path).await {
            Ok(_) => {
                tracing::trace!("✅ HDFS path exists");
                Ok(true)
            }
            Err(FilesystemError::NotFound(_)) => {
                tracing::trace!("❌ HDFS path does not exist");
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    async fn metadata(&self, path: &str) -> FsResult<FileMetadata> {
        tracing::debug!("📊 HDFS metadata: {}", path);
        let normalized_path = self.normalize_path(path);
        let status_json = self.client.get_file_status(&normalized_path).await?;

        let file_status = &status_json["FileStatus"];

        let size = file_status["length"].as_u64().unwrap_or(0);
        let is_directory = file_status["type"].as_str() == Some("DIRECTORY");
        let path_suffix = file_status["pathSuffix"].as_str().unwrap_or("");

        // Convert HDFS timestamps (milliseconds since epoch)
        let modification_time = file_status["modificationTime"]
            .as_u64()
            .and_then(|ts| chrono::DateTime::from_timestamp_millis(ts as i64));
        let access_time = file_status["accessTime"]
            .as_u64()
            .and_then(|ts| chrono::DateTime::from_timestamp_millis(ts as i64));

        let permissions = file_status["permission"].as_str().map(|s| s.to_string());

        tracing::debug!(
            "✅ HDFS metadata retrieved: {} bytes, dir: {}",
            size,
            is_directory
        );

        Ok(FileMetadata {
            path: format!("hdfs://{}", normalized_path),
            size,
            created: None, // HDFS doesn't track creation time
            modified: modification_time,
            is_directory,
            permissions,
            etag: None,          // HDFS doesn't have ETags
            storage_class: None, // HDFS doesn't have storage classes
        })
    }

    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        tracing::debug!("📋 HDFS list: {}", path);
        let normalized_path = self.normalize_path(path);
        let file_statuses = self.client.list_directory(&normalized_path).await?;

        let mut entries = Vec::new();

        for status in file_statuses {
            let name = status["pathSuffix"].as_str().unwrap_or("").to_string();
            let size = status["length"].as_u64().unwrap_or(0);
            let is_directory = status["type"].as_str() == Some("DIRECTORY");

            let modification_time = status["modificationTime"]
                .as_u64()
                .and_then(|ts| chrono::DateTime::from_timestamp_millis(ts as i64));

            let permissions = status["permission"].as_str().map(|s| s.to_string());

            let entry_path = if normalized_path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", normalized_path, name)
            };

            let metadata = FileMetadata {
                path: format!("hdfs://{}", entry_path),
                size,
                created: None,
                modified: modification_time,
                is_directory,
                permissions,
                etag: None,
                storage_class: None,
            };

            entries.push(DirEntry {
                name,
                path: entry_path,
                metadata,
            });
        }

        // Sort entries by name for consistent ordering
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        tracing::debug!("✅ Listed {} entries in HDFS directory", entries.len());
        Ok(entries)
    }

    async fn create_dir(&self, path: &str) -> FsResult<()> {
        tracing::debug!("📁 HDFS create_dir: {}", path);
        let normalized_path = self.normalize_path(path);
        self.client.create_directory(&normalized_path).await
    }

    async fn create_dir_all(&self, path: &str) -> FsResult<()> {
        tracing::debug!("📁 HDFS create_dir_all: {}", path);
        // HDFS MKDIRS operation creates parent directories automatically
        self.create_dir(path).await
    }

    async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        tracing::debug!("📋 HDFS copy: {} → {}", from, to);
        // HDFS doesn't have native copy, so read and write
        let data = self.read(from).await?;
        self.write(to, &data, None).await
    }

    async fn move_file(&self, from: &str, to: &str) -> FsResult<()> {
        tracing::debug!("🔄 HDFS move: {} → {}", from, to);
        // HDFS supports rename operation, but for simplicity use copy + delete
        self.copy(from, to).await?;
        self.delete(from).await
    }

    fn filesystem_type(&self) -> &'static str {
        "hdfs"
    }

    async fn sync(&self) -> FsResult<()> {
        tracing::trace!("🔄 HDFS sync (no-op)");
        // HDFS operations are immediately durable due to replication
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hdfs_path_normalization() {
        let config = HdfsConfig::default();
        let client = HdfsClient::new(config.clone()).await.unwrap();
        let fs = HdfsFileSystem { config, client };

        assert_eq!(fs.normalize_path("/absolute/path"), "/absolute/path");
        assert_eq!(fs.normalize_path("relative/path"), "/relative/path");
        assert_eq!(fs.normalize_path("file.txt"), "/file.txt");
    }
}
