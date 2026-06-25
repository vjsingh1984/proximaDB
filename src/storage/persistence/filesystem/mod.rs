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

use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, trace};
use url::Url;

pub mod atomic_strategy;
// intelligent_filesystem removed - using UnifiedCachingFilesystem instead
pub mod local;
// Cloud object-store backends, built on the official SDKs (which handle signing,
// custom endpoints, and path-style — incl. MinIO/Azurite/fake-gcs emulators) and
// gated behind their features. These replace the deleted legacy s3.rs/azure.rs/
// gcs.rs/auth.rs/manager.rs, which were dead, incomplete, hand-rolled clients.
#[cfg(feature = "aws")]
pub mod aws_s3;
#[cfg(feature = "azure")]
pub mod azure_blob;
#[cfg(feature = "gcp")]
pub mod gcs_store;
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

// The filesystem abstraction trait + value types live in the leaf crate
// `proximadb-storage-filesystem-types` (extracted to break the
// encryption/schema/common/metadata → root coupling). Re-exported here so every
// existing `crate::storage::persistence::filesystem::{FileSystem, ...}` path
// keeps resolving unchanged.
pub use proximadb_storage_filesystem_types::*;

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
    pub auth_config: Option<FilesystemAuthConfig>,

    /// Performance optimization settings
    pub performance_config: FilesystemPerformanceConfig,

    /// Scheme mapping for URL scheme overrides (e.g., "gs" -> "gcs")
    pub scheme_mapping: HashMap<String, String>,
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

        // Cloud object-store backends (real official-SDK FileSystem impls). Each
        // is registered by scheme when its feature is compiled in; config comes
        // from env (standard cloud env vars + PROXIMADB_* overrides for custom
        // endpoints like MinIO/Azurite/fake-gcs). Registration is best-effort —
        // a misconfigured cloud backend logs a warning and is skipped rather than
        // failing factory init. The default build (no cloud feature) registers
        // only "file". `get_filesystem(url)` then dispatches s3://, gs://, adls://
        // to these without further changes.
        #[cfg(feature = "aws")]
        match aws_s3::AwsS3FileSystem::new(Self::aws_s3_config_from_env()).await {
            Ok(backend) => {
                let fs =
                    self.maybe_wrap_with_encryption(Arc::new(backend) as Arc<dyn FileSystem>)?;
                self.filesystems.insert("s3".to_string(), fs);
            }
            Err(e) => tracing::warn!("S3 FileSystem not registered: {e}"),
        }
        #[cfg(feature = "azure")]
        match azure_blob::AzureBlobFileSystem::new(Self::azure_config_from_env()).await {
            Ok(backend) => {
                let fs =
                    self.maybe_wrap_with_encryption(Arc::new(backend) as Arc<dyn FileSystem>)?;
                for scheme in ["adls", "abfs", "az", "azure"] {
                    self.filesystems.insert(scheme.to_string(), fs.clone());
                }
            }
            Err(e) => tracing::warn!("Azure FileSystem not registered: {e}"),
        }
        #[cfg(feature = "gcp")]
        match gcs_store::GcsFileSystem::new(Self::gcs_config_from_env()).await {
            Ok(backend) => {
                let fs =
                    self.maybe_wrap_with_encryption(Arc::new(backend) as Arc<dyn FileSystem>)?;
                for scheme in ["gcs", "gs"] {
                    self.filesystems.insert(scheme.to_string(), fs.clone());
                }
            }
            Err(e) => tracing::warn!("GCS FileSystem not registered: {e}"),
        }

        Ok(())
    }

    /// Build the S3 backend config from env (`AWS_*` + `PROXIMADB_S3_*` overrides
    /// for custom endpoints / path-style, e.g. MinIO).
    #[cfg(feature = "aws")]
    fn aws_s3_config_from_env() -> aws_s3::AwsS3Config {
        aws_s3::AwsS3Config {
            region: std::env::var("PROXIMADB_S3_REGION")
                .or_else(|_| std::env::var("AWS_REGION"))
                .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
                .unwrap_or_else(|_| "us-east-1".to_string()),
            endpoint_url: std::env::var("PROXIMADB_S3_ENDPOINT").ok(),
            force_path_style: std::env::var("PROXIMADB_S3_FORCE_PATH_STYLE")
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                .unwrap_or(false),
            access_key_id: std::env::var("AWS_ACCESS_KEY_ID").ok(),
            secret_access_key: std::env::var("AWS_SECRET_ACCESS_KEY").ok(),
            session_token: std::env::var("AWS_SESSION_TOKEN").ok(),
        }
    }

    /// Build the Azure backend config from env (`AZURE_STORAGE_*`;
    /// `PROXIMADB_AZURE_EMULATOR=1` for Azurite).
    #[cfg(feature = "azure")]
    fn azure_config_from_env() -> azure_blob::AzureBlobConfig {
        azure_blob::AzureBlobConfig {
            account: std::env::var("AZURE_STORAGE_ACCOUNT")
                .unwrap_or_else(|_| "devstoreaccount1".to_string()),
            access_key: std::env::var("AZURE_STORAGE_KEY").ok(),
            use_emulator: std::env::var("PROXIMADB_AZURE_EMULATOR")
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                .unwrap_or(false),
            endpoint: std::env::var("AZURE_STORAGE_ENDPOINT").ok(),
            // Secret-less auth: the AKS workload-identity webhook projects these
            // three env vars into the pod, so an MVP deployment with a federated
            // identity authenticates to ADLS with no storage key in config.
            // `AZURE_CLIENT_ID` alone (no tenant/token) selects a user-assigned
            // Managed Identity; absent entirely, the system-assigned MI is used.
            client_id: std::env::var("AZURE_CLIENT_ID").ok(),
            tenant_id: std::env::var("AZURE_TENANT_ID").ok(),
            federated_token_file: std::env::var("AZURE_FEDERATED_TOKEN_FILE").ok(),
        }
    }

    /// Build the GCS backend config from env (`PROXIMADB_GCS_*`;
    /// `PROXIMADB_GCS_ANONYMOUS=1` + endpoint for fake-gcs-server).
    #[cfg(feature = "gcp")]
    fn gcs_config_from_env() -> gcs_store::GcsConfig {
        gcs_store::GcsConfig {
            endpoint_url: std::env::var("PROXIMADB_GCS_ENDPOINT").ok(),
            anonymous: std::env::var("PROXIMADB_GCS_ANONYMOUS")
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                .unwrap_or(false),
            project_id: std::env::var("GCP_PROJECT")
                .or_else(|_| std::env::var("GOOGLE_CLOUD_PROJECT"))
                .ok(),
        }
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

    pub async fn metadata(&self, url: &str) -> FsResult<FsFileMetadata> {
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
