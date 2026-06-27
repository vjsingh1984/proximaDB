//! Pure filesystem abstraction types for ProximaDB storage.
//!
//! The `FileSystem` / `FilesystemFile` traits plus their supporting value types,
//! extracted from `src/storage/persistence/filesystem/mod.rs` into a leaf crate
//! so storage modules (encryption, schema, common, metadata) can depend *down*
//! on the abstraction rather than up into the root crate. Behavior-neutral: the
//! root path re-exports these via `pub use proximadb_storage_filesystem_types::*;`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Error as IoError;

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

/// Backwards-compat alias for [`FsFileMetadata`].
pub type FileMetadata = FsFileMetadata;

/// File metadata information
#[derive(Debug, Clone, Default)]
pub struct FsFileMetadata {
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
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub url: String, // Full URL instead of relative path
    pub metadata: FsFileMetadata,
}

/// Temporary directory strategy for atomic operations
#[derive(Debug, Clone, Default)]
pub enum TempStrategy {
    /// Direct write (no temp files) - for local filesystem with atomic guarantees
    DirectWrite,

    /// Write to ___temp subdirectory in same location (same mount point)
    /// Ensures move operations are filesystem renames, not copies
    #[default]
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

/// File operation options
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileOptions {
    pub create_dirs: bool,
    pub overwrite: bool,
    pub buffer_size: Option<usize>,
    pub encryption: Option<String>,
    /// Canonical object-storage access tier for this write (the cost lever — see
    /// [`ObjectAccessTier`]). Holds the lowercase canonical name (`hot`|`cool`|
    /// `cold`|`archive`) or a native provider spelling; a backend maps it to its
    /// provider class at the I/O boundary. `None` ⇒ the account/backend default
    /// (today's behavior, unchanged).
    pub storage_class: Option<String>,
    pub metadata: Option<HashMap<String, String>>,

    /// Pre-computed temp path (cached for performance)
    /// None means direct write, Some means atomic write-temp-rename
    pub temp_path: Option<String>,
}

impl FileOptions {
    /// Resolve [`FileOptions::storage_class`] to a canonical [`ObjectAccessTier`].
    /// Returns `None` when unset or unrecognized — the caller then uses the
    /// backend/account default tier rather than failing the write.
    pub fn access_tier(&self) -> Option<ObjectAccessTier> {
        self.storage_class
            .as_deref()
            .and_then(ObjectAccessTier::parse)
    }
}

/// Canonical, cloud-neutral object-storage **access tier** — the dominant
/// object-storage **cost lever** (ADR-036). A colder tier trades retrieval
/// latency/cost for a far lower at-rest GB-month price; on object storage the
/// at-rest term, not CPU, is what we co-design against. Callers choose a
/// *canonical* tier and each cloud backend maps it to that provider's native
/// class at the I/O boundary, so call sites never hard-code `"Cool"` (Azure) vs
/// `"STANDARD_IA"` (S3) vs `"NEARLINE"` (GCS).
///
/// Distinct from [`FileStorageTier`], which models *physical media latency*
/// (Memory/NVMe/SSD/…); this is the *cloud billing tier* applied per object PUT.
///
/// IMPORTANT (Azure orientation, ADR-036): the tier is a property of the **object
/// PUT** (`x-ms-access-tier`), available on a *flat* Blob account — it does **not**
/// require ADLS Gen2 Hierarchical Namespace. HNS adds per-operation cost with no
/// benefit for our flat-key, immutable, ranged-read workload; the access tier is
/// the lever, the namespace mode is not. The `az://`/`azure://`/`adls://`/`abfs://`
/// schemes all resolve to the *same* Blob endpoint backend (see the filesystem
/// factory) — the scheme is **not** a cost lever, only the tier is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectAccessTier {
    /// Frequent access, lowest latency, highest at-rest price. The backend default.
    Hot,
    /// Infrequent access (≈30-day min). Azure `Cool` / S3 `STANDARD_IA` / GCS `NEARLINE`.
    Cool,
    /// Rare access, still online/instant-retrieval. Azure `Cold` / S3 Glacier
    /// Instant Retrieval / GCS `COLDLINE`.
    Cold,
    /// Offline, minutes-to-hours retrieval, cheapest. Azure `Archive` / S3 Glacier
    /// Flexible Retrieval / GCS `ARCHIVE`.
    Archive,
}

impl ObjectAccessTier {
    /// Lenient parse: canonical names + common provider spellings. Returns `None`
    /// for an unrecognized value — callers fall back to the backend default and
    /// never fail a write on a tier typo.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "hot" | "standard" => Some(Self::Hot),
            "cool" | "standard_ia" | "standard-ia" | "onezone_ia" | "nearline" => Some(Self::Cool),
            "cold" | "coldline" | "glacier_ir" | "glacier-ir" => Some(Self::Cold),
            "archive"
            | "glacier"
            | "glacier_flexible"
            | "deep_archive"
            | "glacier_deep_archive" => Some(Self::Archive),
            _ => None,
        }
    }

    /// Azure Blob `x-ms-access-tier` value (also the ADLS Gen2 access tier).
    pub fn as_azure_access_tier(self) -> &'static str {
        match self {
            Self::Hot => "Hot",
            Self::Cool => "Cool",
            Self::Cold => "Cold",
            Self::Archive => "Archive",
        }
    }

    /// AWS S3 `x-amz-storage-class` value. `Cold` maps to Glacier Instant Retrieval
    /// (synchronous, no restore); `Archive` maps to Glacier Flexible Retrieval.
    pub fn as_s3_storage_class(self) -> &'static str {
        match self {
            Self::Hot => "STANDARD",
            Self::Cool => "STANDARD_IA",
            Self::Cold => "GLACIER_IR",
            Self::Archive => "GLACIER",
        }
    }

    /// GCS storage class.
    pub fn as_gcs_storage_class(self) -> &'static str {
        match self {
            Self::Hot => "STANDARD",
            Self::Cool => "NEARLINE",
            Self::Cold => "COLDLINE",
            Self::Archive => "ARCHIVE",
        }
    }

    /// Canonical lowercase name — the serialized [`FileOptions::storage_class`] form.
    pub fn as_canonical_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Cool => "cool",
            Self::Cold => "cold",
            Self::Archive => "archive",
        }
    }
}

/// Authentication configuration for cloud providers
#[derive(Debug, Clone)]
pub struct FilesystemAuthConfig {
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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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

/// Backwards-compat alias for [`FsRetryConfig`].
pub type RetryConfig = FsRetryConfig;

/// Retry configuration for operations
#[derive(Debug, Clone)]
pub struct FsRetryConfig {
    /// Maximum number of retries
    pub max_retries: u32,
    /// Initial delay between retries (ms)
    pub initial_delay_ms: u64,
    /// Maximum delay between retries (ms)
    pub max_delay_ms: u64,
    /// Backoff multiplier for exponential backoff
    pub backoff_multiplier: f64,
}

/// Storage tier type for intelligent data placement
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileStorageTier {
    /// In-memory storage (fastest)
    Memory,
    /// NVMe SSD storage (microsecond latency)
    NVMe,
    /// SSD storage (millisecond latency)
    SSD,
    /// HDD storage (10-20ms latency)
    HDD,
    /// S3 Express One Zone (single-digit ms)
    S3Express,
    /// S3 Standard (10-100ms)
    S3Standard,
    /// S3 Glacier Instant (100ms+)
    S3GlacierInstant,
    /// Azure Premium SSD
    AzurePremium,
    /// Azure Standard SSD
    AzureStandard,
    /// Google Cloud SSD
    GcsSSD,
    /// Google Cloud HDD
    GcsHDD,
}

impl FileStorageTier {
    /// Get expected latency in microseconds
    pub fn expected_latency_us(&self) -> u64 {
        match self {
            FileStorageTier::Memory => 1,                 // <1μs
            FileStorageTier::NVMe => 100,                 // 100μs
            FileStorageTier::SSD => 1_000,                // 1ms
            FileStorageTier::HDD => 10_000,               // 10ms
            FileStorageTier::S3Express => 5_000,          // 5ms
            FileStorageTier::S3Standard => 50_000,        // 50ms
            FileStorageTier::S3GlacierInstant => 100_000, // 100ms
            FileStorageTier::AzurePremium => 500,         // 500μs
            FileStorageTier::AzureStandard => 2_000,      // 2ms
            FileStorageTier::GcsSSD => 800,               // 800μs
            FileStorageTier::GcsHDD => 15_000,            // 15ms
        }
    }

    /// Get optimal I/O size in bytes for this tier
    pub fn optimal_io_size(&self) -> usize {
        match self {
            FileStorageTier::Memory => 64 * 1024,                 // 64KB
            FileStorageTier::NVMe => 128 * 1024,                  // 128KB
            FileStorageTier::SSD => 256 * 1024,                   // 256KB
            FileStorageTier::HDD => 1024 * 1024,                  // 1MB
            FileStorageTier::S3Express => 512 * 1024,             // 512KB
            FileStorageTier::S3Standard => 1024 * 1024,           // 1MB
            FileStorageTier::S3GlacierInstant => 4 * 1024 * 1024, // 4MB
            FileStorageTier::AzurePremium => 256 * 1024,          // 256KB
            FileStorageTier::AzureStandard => 512 * 1024,         // 512KB
            FileStorageTier::GcsSSD => 256 * 1024,                // 256KB
            FileStorageTier::GcsHDD => 2 * 1024 * 1024,           // 2MB
        }
    }

    /// Check if this tier is faster than another
    pub fn is_faster_than(&self, other: &FileStorageTier) -> bool {
        self.expected_latency_us() < other.expected_latency_us()
    }
}

/// Backwards-compat alias for [`FsTierConfig`].
pub type TierConfig = FsTierConfig;

/// Tier-specific storage configuration
#[derive(Debug, Clone)]
pub struct FsTierConfig {
    /// Storage tier type
    pub tier: FileStorageTier,

    /// Base URL for this tier (e.g., "file:///mnt/nvme", "s3://bucket")
    pub base_url: String,

    /// Maximum capacity in bytes (None = unlimited)
    pub max_capacity_bytes: Option<u64>,

    /// Current usage in bytes (tracked runtime)
    pub current_usage_bytes: u64,

    /// Enable compression for this tier
    pub compression: bool,

    /// Custom I/O size override (uses tier default if None)
    pub io_size_override: Option<usize>,
}

/// Filesystem performance configuration
#[derive(Debug, Clone)]
pub struct FilesystemPerformanceConfig {
    /// Connection pool size per backend
    pub connection_pool_size: usize,

    /// Enable connection keep-alive
    pub enable_keep_alive: bool,

    /// Request timeout (seconds)
    pub request_timeout_seconds: u64,

    /// Enable compression for network transfers
    pub compression: bool,

    /// Retry configuration
    pub retry_config: FsRetryConfig,

    /// Buffer size for operations (bytes)
    pub buffer_size: usize,

    /// Enable parallel operations
    pub enable_parallel_ops: bool,

    /// Maximum concurrent operations
    pub max_concurrent_ops: usize,

    /// Tier-specific configurations
    pub tier_configs: Vec<FsTierConfig>,
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
    /// Get self as Any for downcasting to concrete types
    fn as_any(&self) -> &dyn std::any::Any;

    /// Read file contents
    async fn read(&self, path: &str) -> FsResult<Vec<u8>>;

    /// Get memory-mapped access to a file (only supported for local filesystem)
    /// Returns None if memory mapping is not supported (e.g., cloud storage)
    /// The returned mmap is read-only and safe for concurrent access
    async fn get_mmap(&self, _path: &str) -> FsResult<Option<memmap2::Mmap>> {
        // Default implementation returns None (not supported)
        // LocalFileSystem will override this to provide actual memory mapping
        Ok(None)
    }

    /// Check if this filesystem supports memory mapping
    fn supports_mmap(&self) -> bool {
        false // Default: most filesystems don't support mmap
    }

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
    async fn read_ranges(
        &self,
        path: &str,
        ranges: Vec<std::ops::Range<u64>>,
    ) -> FsResult<Vec<Vec<u8>>> {
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
    async fn sync_file(&self, _path: &str) -> FsResult<()> {
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
    async fn metadata(&self, path: &str) -> FsResult<FsFileMetadata>;

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
                let parent = final_path.parent();
                let temp_dir = parent.map_or_else(
                    || std::path::PathBuf::from("___temp"),
                    |p| p.join("___temp"),
                );
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
        let temp_path_opt = options.as_ref().and_then(|o| o.temp_path.clone());

        match temp_path_opt {
            None => {
                // Direct write (optimal for local filesystem)
                self.write(final_path, data, options).await
            }
            Some(temp_path_str) => {
                // Atomic write-temp-rename (optimal for object stores)
                let temp_path = std::path::Path::new(&temp_path_str);

                // Ensure temp directory exists
                if let Some(temp_parent) = temp_path.parent() {
                    self.create_dir_all(&temp_parent.to_string_lossy()).await?;
                }

                // Write to temp location
                let temp_opts = options.map(|o| FileOptions {
                    temp_path: None, // Prevent recursion
                    ..o
                });
                self.write(&temp_path_str, data, temp_opts).await?;

                // Atomic move (rename on same mount point)
                self.move_file(&temp_path_str, final_path).await
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
    async fn write_string(
        &self,
        path: &str,
        content: &str,
        options: Option<FileOptions>,
    ) -> FsResult<()> {
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

impl Default for FilesystemPerformanceConfig {
    fn default() -> Self {
        Self {
            connection_pool_size: 10,
            enable_keep_alive: true,
            request_timeout_seconds: 30,
            compression: true,
            retry_config: FsRetryConfig {
                max_retries: 3,
                initial_delay_ms: 100,
                max_delay_ms: 5000,
                backoff_multiplier: 2.0,
            },
            buffer_size: 8 * 1024 * 1024, // 8MB
            enable_parallel_ops: true,
            max_concurrent_ops: 100,
            tier_configs: vec![],
        }
    }
}

/// Convert a filesystem error into the foundational `VectorDBError` contract.
/// Defined here (where `FilesystemError` is local) to satisfy the orphan rule
/// after the trait + types were extracted from the root crate.
impl From<FilesystemError> for proximadb_kernel::error::VectorDBError {
    fn from(err: FilesystemError) -> Self {
        proximadb_kernel::error::VectorDBError::Filesystem(err.to_string())
    }
}

#[cfg(test)]
mod object_access_tier_tests {
    use super::{FileOptions, ObjectAccessTier};

    #[test]
    fn parse_accepts_canonical_names_case_insensitively() {
        assert_eq!(ObjectAccessTier::parse("hot"), Some(ObjectAccessTier::Hot));
        assert_eq!(
            ObjectAccessTier::parse("Cool"),
            Some(ObjectAccessTier::Cool)
        );
        assert_eq!(
            ObjectAccessTier::parse(" COLD "),
            Some(ObjectAccessTier::Cold)
        );
        assert_eq!(
            ObjectAccessTier::parse("archive"),
            Some(ObjectAccessTier::Archive)
        );
    }

    #[test]
    fn parse_accepts_native_provider_spellings() {
        // S3
        assert_eq!(
            ObjectAccessTier::parse("STANDARD"),
            Some(ObjectAccessTier::Hot)
        );
        assert_eq!(
            ObjectAccessTier::parse("STANDARD_IA"),
            Some(ObjectAccessTier::Cool)
        );
        assert_eq!(
            ObjectAccessTier::parse("GLACIER_IR"),
            Some(ObjectAccessTier::Cold)
        );
        assert_eq!(
            ObjectAccessTier::parse("GLACIER"),
            Some(ObjectAccessTier::Archive)
        );
        // GCS
        assert_eq!(
            ObjectAccessTier::parse("NEARLINE"),
            Some(ObjectAccessTier::Cool)
        );
        assert_eq!(
            ObjectAccessTier::parse("COLDLINE"),
            Some(ObjectAccessTier::Cold)
        );
    }

    #[test]
    fn parse_returns_none_for_unknown_so_write_falls_back_to_default() {
        assert_eq!(ObjectAccessTier::parse("warm"), None);
        assert_eq!(ObjectAccessTier::parse(""), None);
    }

    #[test]
    fn per_cloud_mappings_are_native() {
        assert_eq!(ObjectAccessTier::Cool.as_azure_access_tier(), "Cool");
        assert_eq!(ObjectAccessTier::Cold.as_azure_access_tier(), "Cold");
        assert_eq!(ObjectAccessTier::Cool.as_s3_storage_class(), "STANDARD_IA");
        assert_eq!(ObjectAccessTier::Cold.as_s3_storage_class(), "GLACIER_IR");
        assert_eq!(ObjectAccessTier::Archive.as_gcs_storage_class(), "ARCHIVE");
    }

    #[test]
    fn canonical_str_round_trips_through_parse() {
        for t in [
            ObjectAccessTier::Hot,
            ObjectAccessTier::Cool,
            ObjectAccessTier::Cold,
            ObjectAccessTier::Archive,
        ] {
            assert_eq!(ObjectAccessTier::parse(t.as_canonical_str()), Some(t));
        }
    }

    #[test]
    fn file_options_access_tier_resolves_and_defaults_safely() {
        let mut o = FileOptions::default();
        assert_eq!(o.access_tier(), None); // unset ⇒ backend default
        o.storage_class = Some("cool".to_string());
        assert_eq!(o.access_tier(), Some(ObjectAccessTier::Cool));
        o.storage_class = Some("nonsense".to_string());
        assert_eq!(o.access_tier(), None); // typo ⇒ default, never an error
    }
}
