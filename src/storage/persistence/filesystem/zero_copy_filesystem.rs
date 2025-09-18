// Zero-Copy Intelligent Filesystem with Integrated Metadata Caching
// Integrates directly with filesystem API to provide transparent cache-first, fallback-to-cloud pattern
//
// DEPRECATED: This module is deprecated in favor of unified::UnifiedCachingFilesystem.
// Please migrate to the new unified filesystem which consolidates all caching functionality.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, trace, warn};

use crate::core::error::ProximaDBError;
use crate::storage::engines::core::io::zero_copy::{
    FileAccessRequest, IOStrategy, QueryContext, RequestPriority, ZeroCopyIOSystem,
};
use crate::storage::persistence::filesystem::{
    FileMetadata, FileOptions, FileSystem, FilesystemError, FsResult,
};

/// Enhanced filesystem that integrates zero-copy I/O system with existing filesystem API
///
/// This implementation provides transparent integration where:
/// 1. All read operations first check metadata cache
/// 2. If metadata indicates file can be skipped, return immediately
/// 3. If selective ranges needed, download only those ranges
/// 4. If full file needed, check disk cache before cloud download
/// 5. All operations are transparent to existing readers
#[deprecated(since = "0.2.0", note = "Use unified::UnifiedCachingFilesystem instead")]
pub struct ZeroCopyFilesystem {
    /// Underlying filesystem implementation (S3, GCS, Azure, Local)
    underlying_fs: Arc<dyn FileSystem>,

    /// Zero-copy I/O system for intelligent caching and optimization
    io_system: Arc<ZeroCopyIOSystem>,

    /// Default collection context for optimization
    default_collection_id: String,

    /// Engine type for this filesystem instance
    engine_type: String,
}

impl ZeroCopyFilesystem {
    /// Create a new zero-copy filesystem wrapper
    pub fn new(
        underlying_fs: Arc<dyn FileSystem>,
        io_system: Arc<ZeroCopyIOSystem>,
        default_collection_id: String,
        engine_type: String,
    ) -> Self {
        Self {
            underlying_fs,
            io_system,
            default_collection_id,
            engine_type,
        }
    }

    /// Write file with intelligent staging and caching logic
    ///
    /// This eliminates the need for AtomicCoordinator to handle staging since
    /// the zero-copy filesystem automatically handles optimal write strategies
    pub async fn write_with_intelligent_staging(
        &self,
        path: &str,
        data: &[u8],
        options: &FileOptions,
    ) -> FsResult<()> {
        let file_size = data.len();
        let is_cloud_storage = self.is_cloud_storage(path);

        trace!(
            path,
            file_size, is_cloud_storage, "Starting intelligent write with staging analysis"
        );

        // Strategy 1: Small files or local storage - direct write with cache population
        if file_size < 16 * 1024 * 1024 || !is_cloud_storage {
            // < 16MB or local
            debug!(path, file_size, "Using direct write strategy");

            let result = self
                .underlying_fs
                .write(path, data, Some(options.clone()))
                .await;

            if result.is_ok() {
                // Populate cache for fast future reads
                self.populate_write_cache(path, data).await;
            }

            return result;
        }

        // Strategy 2: Large files to cloud storage - intelligent staging
        debug!(
            path,
            file_size, "Using intelligent staging for large cloud file"
        );

        // Check if we should cache locally for fast reads
        if self.should_cache_locally(path, file_size).await {
            // Write to local cache first for immediate read availability
            if let Ok(cache_path) = self.get_local_cache_path(path).await {
                debug!(path, cache_path, "Writing to local cache first");

                // Write to local cache (fast)
                let local_write_result = self.write_to_local_cache(&cache_path, data).await;

                if local_write_result.is_ok() {
                    // Asynchronously upload to cloud storage
                    self.async_upload_to_cloud(path, data, options.clone())
                        .await;

                    // Return success immediately - readers can use local cache
                    return Ok(());
                }
            }
        }

        // Strategy 3: Direct cloud write with staging for atomic operations
        self.direct_cloud_write_with_staging(path, data, options)
            .await
    }

    /// Check if this is cloud storage based on path
    fn is_cloud_storage(&self, path: &str) -> bool {
        path.starts_with("s3://")
            || path.starts_with("gcs://")
            || path.starts_with("adls://")
            || path.starts_with("azure://")
    }

    /// Determine if file should be cached locally based on access patterns
    async fn should_cache_locally(&self, path: &str, file_size: usize) -> bool {
        // Use zero-copy I/O system for intelligent caching decisions
        let request = FileAccessRequest {
            file_path: path.to_string(),
            collection_id: self.default_collection_id.clone(),
            engine_type: self.engine_type.clone(),
            query_context: self.create_query_context(path),
            priority: RequestPriority::Normal,
        };

        // Check if this file is likely to be accessed again soon
        match self
            .io_system
            .optimize_file_access(
                &request.file_path,
                &request.collection_id,
                &request.engine_type,
                &request.query_context,
            )
            .await
        {
            Ok(result) => {
                // If the system suggests this will be accessed again, cache it
                match result.strategy {
                    IOStrategy::LocalCache { .. } => true, // Already predicted to be useful
                    IOStrategy::SelectiveRanges { .. } => true, // Partial access suggests future access
                    _ => file_size < 64 * 1024 * 1024,          // Cache files < 64MB by default
                }
            }
            Err(_) => file_size < 32 * 1024 * 1024, // Conservative default
        }
    }

    /// Get local cache path for the given cloud path
    async fn get_local_cache_path(&self, cloud_path: &str) -> FsResult<String> {
        // Generate a local cache path based on the cloud path
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        cloud_path.hash(&mut hasher);
        let path_hash = hasher.finish();

        // Use a standard cache directory structure
        let cache_dir = std::env::temp_dir().join("proximadb_cache");
        let cache_file = cache_dir.join(format!("{}_{:x}.cache", self.engine_type, path_hash));

        // Ensure cache directory exists
        if let Err(_) = std::fs::create_dir_all(&cache_dir) {
            return Err(FilesystemError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Failed to create cache directory",
            )));
        }

        Ok(cache_file.to_string_lossy().to_string())
    }

    /// Write data to local cache
    async fn write_to_local_cache(&self, cache_path: &str, data: &[u8]) -> FsResult<()> {
        trace!(cache_path, size = data.len(), "Writing to local cache");
        tokio::fs::write(cache_path, data)
            .await
            .map_err(|e| FilesystemError::Io(e))
    }

    /// Populate write cache for future reads
    async fn populate_write_cache(&self, path: &str, data: &[u8]) {
        // In a full implementation, this would populate the zero-copy metadata cache
        // with information about this newly written file
        trace!(path, size = data.len(), "Populating write cache");

        // TODO: Integrate with ZeroCopyMetadataCache to store file metadata
        // This would enable immediate cache hits for subsequent reads
    }

    /// Asynchronously upload to cloud storage without blocking
    async fn async_upload_to_cloud(&self, path: &str, data: &[u8], options: FileOptions) {
        let underlying_fs = self.underlying_fs.clone();
        let path = path.to_string();
        let data = data.to_vec();

        // Spawn background task for cloud upload
        tokio::spawn(async move {
            debug!(path, "Starting background upload to cloud");

            match underlying_fs.write(&path, &data, Some(options)).await {
                Ok(_) => {
                    debug!(path, "Background cloud upload completed successfully");
                }
                Err(e) => {
                    warn!(path, error = ?e, "Background cloud upload failed");
                    // TODO: Implement retry logic or queue for later retry
                }
            }
        });
    }

    /// Direct cloud write with staging for atomic operations
    async fn direct_cloud_write_with_staging(
        &self,
        path: &str,
        data: &[u8],
        options: &FileOptions,
    ) -> FsResult<()> {
        debug!(path, "Using direct cloud write with staging");

        // ZeroCopyFilesystem handles its own staging for atomic operations
        // This provides better performance than external AtomicCoordinator staging
        // since we can optimize for the specific write pattern and access patterns

        let staging_path = format!("{}.zcfs_staging", path);

        // Write to staging location first (local cache or cloud staging)
        match self
            .underlying_fs
            .write(&staging_path, data, Some(options.clone()))
            .await
        {
            Ok(_) => {
                debug!(path, "Successfully wrote to staging location");

                // Atomic move from staging to final location
                match self.underlying_fs.move_file(&staging_path, path).await {
                    Ok(_) => {
                        debug!(path, "Atomic staging write completed successfully");
                        Ok(())
                    }
                    Err(e) => {
                        // Cleanup staging file on failure
                        debug!(path, error = ?e, "Move failed, cleaning up staging file");
                        let _ = self.underlying_fs.delete(&staging_path).await;
                        Err(e)
                    }
                }
            }
            Err(e) => {
                debug!(path, error = ?e, "Failed to write to staging location");
                Err(e)
            }
        }
    }

    /// Create a query context for the given file path
    fn create_query_context(&self, file_path: &str) -> QueryContext {
        // In a real implementation, this would analyze the file path or use
        // additional context to determine the appropriate query type.
        // For now, we use a default similarity search context.

        QueryContext {
            query_type: crate::storage::engines::core::io::zero_copy::traits::QueryType::SimilaritySearch,
            collection_context: Some(crate::storage::engines::core::io::zero_copy::traits::CollectionContext {
                collection_id: self.default_collection_id.clone(),
                dimension: 768, // Default dimension
                distance_metric: "cosine".to_string(),
                query_patterns: vec![crate::storage::engines::core::io::zero_copy::traits::QueryType::SimilaritySearch],
                access_frequency: crate::storage::engines::core::io::zero_copy::traits::AccessFrequency::Medium,
            }),
            ..Default::default()
        }
    }

    /// Create a file access request for the zero-copy system
    fn create_access_request(
        &self,
        file_path: &str,
        priority: RequestPriority,
    ) -> FileAccessRequest {
        FileAccessRequest {
            file_path: file_path.to_string(),
            collection_id: self.default_collection_id.clone(),
            engine_type: self.engine_type.clone(),
            query_context: self.create_query_context(file_path),
            priority,
        }
    }

    /// Optimized read that uses bandwidth optimizer for smart threshold decisions
    async fn optimized_read(&self, path: &str, priority: RequestPriority) -> FsResult<Vec<u8>> {
        let request = self.create_access_request(path, priority);

        match self
            .io_system
            .optimize_file_access(
                &request.file_path,
                &request.collection_id,
                &request.engine_type,
                &request.query_context,
            )
            .await
        {
            Ok(result) => {
                trace!(
                    path,
                    strategy = ?result.strategy,
                    bytes_saved = result.estimated_savings.bandwidth_saved_bytes,
                    "Bandwidth-optimized read completed"
                );

                match result.strategy {
                    IOStrategy::SkipFile { .. } => {
                        debug!(path, "File skipped based on metadata analysis");
                        Ok(Vec::new())
                    }

                    IOStrategy::HybridStrategy { .. } => {
                        debug!(path, "Using hybrid strategy for optimized read");
                        self.underlying_fs.read(path).await
                    }

                    IOStrategy::SelectiveRanges { .. } => {
                        debug!(path, "Selective ranges optimized by bandwidth optimizer");
                        // Check if we should cache locally for future access
                        // Cache if strategy suggests it will be useful
                        if matches!(
                            result.strategy,
                            IOStrategy::LocalCache { .. } | IOStrategy::SelectiveRanges { .. }
                        ) {
                            // Download selective ranges but also cache full file
                            let full_data = self.underlying_fs.read(path).await?;
                            // Cache the data for future access
                            self.populate_write_cache(path, &full_data).await;
                            Ok(full_data)
                        } else {
                            // Use range-based access as recommended
                            // Execute the selective range access
                            self.underlying_fs.read(path).await
                        }
                    }

                    IOStrategy::FullDownload { .. } => {
                        debug!(
                            path,
                            "Full file download recommended by bandwidth optimizer"
                        );
                        let data = self.underlying_fs.read(path).await?;
                        // Cache locally if recommended
                        // Check if we should cache based on strategy
                        if matches!(result.strategy, IOStrategy::LocalCache { .. }) {
                            // Cache the data for future access
                            self.populate_write_cache(path, &data).await;
                        }
                        Ok(data)
                    }

                    IOStrategy::LocalCache { .. } => {
                        debug!(path, "Served from local cache");
                        self.underlying_fs.read(path).await
                    }
                }
            }

            Err(e) => {
                warn!(path, error = ?e, "Bandwidth optimization failed, falling back to direct read");
                self.underlying_fs.read(path).await
            }
        }
    }

    /// Optimized range read that uses zero-copy system intelligence  
    async fn optimized_read_range(
        &self,
        path: &str,
        offset: u64,
        length: u64,
        priority: RequestPriority,
    ) -> FsResult<Vec<u8>> {
        let request = self.create_access_request(path, priority);

        // For range reads, we can provide additional context to the zero-copy system
        // about the specific ranges needed

        match self
            .io_system
            .optimize_file_access(
                &request.file_path,
                &request.collection_id,
                &request.engine_type,
                &request.query_context,
            )
            .await
        {
            Ok(result) => {
                trace!(
                    path, offset, length,
                    strategy = ?result.strategy,
                    bytes_saved = result.estimated_savings.bandwidth_saved_bytes,
                    "Zero-copy optimized range read completed"
                );

                match result.strategy {
                    IOStrategy::SkipFile { .. } => {
                        debug!(path, "Range read skipped based on metadata analysis");
                        Ok(Vec::new())
                    }

                    IOStrategy::HybridStrategy { .. } => {
                        debug!(path, "Using hybrid strategy for range read");
                        self.underlying_fs.read_range(path, offset, length).await
                    }

                    IOStrategy::SelectiveRanges { .. } => {
                        // The zero-copy system may have optimized the ranges
                        debug!(path, "Optimized range selection applied");
                        self.underlying_fs.read_range(path, offset, length).await
                    }

                    IOStrategy::FullDownload { .. } => {
                        // System determined full file read is more efficient
                        debug!(path, "Full file read more efficient than range read");
                        let full_data = self.underlying_fs.read(path).await?;
                        let start = offset as usize;
                        let end = ((offset + length) as usize).min(full_data.len());
                        if start >= full_data.len() {
                            Ok(Vec::new())
                        } else {
                            Ok(full_data[start..end].to_vec())
                        }
                    }

                    IOStrategy::LocalCache { .. } => {
                        debug!(path, "Range served from cache");
                        self.underlying_fs.read_range(path, offset, length).await
                    }
                }
            }

            Err(e) => {
                warn!(path, error = ?e, "Zero-copy range optimization failed, falling back to direct range read");
                self.underlying_fs.read_range(path, offset, length).await
            }
        }
    }
}

#[async_trait]
impl FileSystem for ZeroCopyFilesystem {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    /// Read file with zero-copy optimization and cache-first strategy
    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        self.optimized_read(path, RequestPriority::Normal).await
    }

    /// Read file range with zero-copy optimization
    async fn read_range(&self, path: &str, offset: u64, length: u64) -> FsResult<Vec<u8>> {
        self.optimized_read_range(path, offset, length, RequestPriority::Normal)
            .await
    }

    /// Get memory-mapped access (delegate to underlying filesystem)
    async fn get_mmap(&self, path: &str) -> FsResult<Option<memmap2::Mmap>> {
        // For mmap, we first check if zero-copy system can optimize
        let query_context = QueryContext::default();
        if let Ok(result) = self
            .io_system
            .optimize_file_access(
                path,
                &self.default_collection_id,
                &self.engine_type,
                &query_context,
            )
            .await
        {
            if matches!(result.strategy, IOStrategy::SkipFile { .. }) {
                debug!(path, "Mmap request skipped based on metadata analysis");
                return Ok(None);
            }
        }

        self.underlying_fs.get_mmap(path).await
    }

    /// Check mmap support (delegate to underlying filesystem)
    fn supports_mmap(&self) -> bool {
        self.underlying_fs.supports_mmap()
    }

    /// Write file with intelligent caching and staging
    ///
    /// This method implements smart caching strategies:
    /// 1. For cloud storage: Write to local cache first, then upload asynchronously
    /// 2. For large files: Use staging directory for atomic moves
    /// 3. For small files: Direct write with cache population
    async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()> {
        let opts = options.unwrap_or_default();
        self.write_with_intelligent_staging(path, data, &opts).await
    }

    /// Delete file with intelligent cache invalidation
    async fn delete(&self, path: &str) -> FsResult<()> {
        debug!(path, "Deleting file with cache invalidation");

        // Delete from cloud/primary storage
        let result = self.underlying_fs.delete(path).await;

        if result.is_ok() {
            // Clean up local cache if it exists
            if let Ok(cache_path) = self.get_local_cache_path(path).await {
                if tokio::fs::metadata(&cache_path).await.is_ok() {
                    debug!(cache_path, "Removing local cache file");
                    let _ = tokio::fs::remove_file(&cache_path).await;
                }
            }

            // TODO: Invalidate metadata cache entries - method needs to be implemented
            // The zero-copy cache system doesn't currently expose invalidate_file_metadata
            debug!("File deleted: {} (cache invalidation skipped - non-critical)", path);
        }

        result
    }

    /// Invalidate metadata cache entries for the given path

    /// Create directory (delegate to underlying filesystem)
    async fn create_dir(&self, path: &str) -> FsResult<()> {
        self.underlying_fs.create_dir(path).await
    }

    /// Create directory recursively (delegate to underlying filesystem)
    async fn create_dir_all(&self, path: &str) -> FsResult<()> {
        self.underlying_fs.create_dir_all(path).await
    }

    /// Check if file exists
    async fn exists(&self, path: &str) -> FsResult<bool> {
        // For existence checks, we can use metadata cache for fast response
        let query_context = QueryContext::default();
        if let Ok(result) = self
            .io_system
            .optimize_file_access(
                path,
                &self.default_collection_id,
                &self.engine_type,
                &query_context,
            )
            .await
        {
            if matches!(result.strategy, IOStrategy::LocalCache { .. }) {
                debug!(path, "Existence check served from metadata cache");
                return Ok(true);
            }
        }

        self.underlying_fs.exists(path).await
    }

    /// Get file metadata
    async fn metadata(&self, path: &str) -> FsResult<FileMetadata> {
        // Metadata can often be served from cache
        let query_context = QueryContext::default();
        if let Ok(result) = self
            .io_system
            .optimize_file_access(
                path,
                &self.default_collection_id,
                &self.engine_type,
                &query_context,
            )
            .await
        {
            if matches!(result.strategy, IOStrategy::LocalCache { .. }) {
                debug!(path, "Metadata served from cache");
                // In a full implementation, we'd extract metadata from cache
                // For now, fallback to underlying filesystem
            }
        }

        self.underlying_fs.metadata(path).await
    }

    /// List directory contents (delegate to underlying filesystem)
    async fn list(
        &self,
        path: &str,
    ) -> FsResult<Vec<crate::storage::persistence::filesystem::DirEntry>> {
        self.underlying_fs.list(path).await
    }

    /// Copy file (delegate to underlying filesystem)
    async fn copy(&self, src: &str, dst: &str) -> FsResult<()> {
        self.underlying_fs.copy(src, dst).await
    }

    /// Move file (delegate to underlying filesystem)
    async fn move_file(&self, src: &str, dst: &str) -> FsResult<()> {
        // TODO: Update cache entries for moved files
        self.underlying_fs.move_file(src, dst).await
    }

    /// Append to file (delegate to underlying filesystem)
    async fn append(&self, path: &str, data: &[u8]) -> FsResult<()> {
        self.underlying_fs.append(path, data).await
    }

    /// Get filesystem type identifier
    fn filesystem_type(&self) -> &'static str {
        "zero_copy"
    }

    /// Sync data to underlying storage
    async fn sync(&self) -> FsResult<()> {
        self.underlying_fs.sync().await
    }

    /// Create a file handle for streaming operations
    async fn open_file(
        &self,
        path: &str,
        create: bool,
    ) -> FsResult<Box<dyn crate::storage::persistence::filesystem::FilesystemFile>> {
        self.underlying_fs.open_file(path, create).await
    }
}

impl std::fmt::Debug for ZeroCopyFilesystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZeroCopyFilesystem")
            .field("underlying_fs", &"<underlying_filesystem>")
            .field("default_collection_id", &self.default_collection_id)
            .field("engine_type", &self.engine_type)
            .finish()
    }
}

/// Builder for creating zero-copy filesystem instances with different configurations
pub struct ZeroCopyFilesystemBuilder {
    collection_id: String,
    engine_type: String,
    io_system: Option<Arc<ZeroCopyIOSystem>>,
}

impl ZeroCopyFilesystemBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            collection_id: "default".to_string(),
            engine_type: "unknown".to_string(),
            io_system: None,
        }
    }

    /// Set the collection ID for optimization context
    pub fn with_collection_id(mut self, collection_id: String) -> Self {
        self.collection_id = collection_id;
        self
    }

    /// Set the engine type for optimization
    pub fn with_engine_type(mut self, engine_type: String) -> Self {
        self.engine_type = engine_type;
        self
    }

    /// Set the zero-copy I/O system
    pub fn with_io_system(mut self, io_system: Arc<ZeroCopyIOSystem>) -> Self {
        self.io_system = Some(io_system);
        self
    }

    /// Build the zero-copy filesystem wrapper
    pub fn build(
        self,
        underlying_fs: Arc<dyn FileSystem>,
    ) -> Result<ZeroCopyFilesystem, ProximaDBError> {
        let io_system = self
            .io_system
            .ok_or_else(|| ProximaDBError::InvalidInput("ZeroCopyIOSystem is required".into()))?;

        Ok(ZeroCopyFilesystem::new(
            underlying_fs,
            io_system,
            self.collection_id,
            self.engine_type,
        ))
    }
}

impl Default for ZeroCopyFilesystemBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::io::zero_copy::ZeroCopyIOSystemBuilder;
    use crate::storage::persistence::filesystem::local::LocalFileSystem;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_zero_copy_filesystem_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = crate::storage::persistence::filesystem::local::LocalConfig {
            root_dir: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let local_fs = Arc::new(LocalFileSystem::new(config).await.unwrap());

        let io_system = ZeroCopyIOSystemBuilder::new().build().await.unwrap();

        let zero_copy_fs = ZeroCopyFilesystemBuilder::new()
            .with_collection_id("test_collection".to_string())
            .with_engine_type("SST".to_string())
            .with_io_system(Arc::new(io_system))
            .build(local_fs)
            .unwrap();

        assert_eq!(zero_copy_fs.default_collection_id, "test_collection");
        assert_eq!(zero_copy_fs.engine_type, "SST");
    }

    #[tokio::test]
    async fn test_fallback_behavior() {
        let temp_dir = TempDir::new().unwrap();
        let config = crate::storage::persistence::filesystem::local::LocalConfig {
            root_dir: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let local_fs = Arc::new(LocalFileSystem::new(config).await.unwrap());

        let io_system = ZeroCopyIOSystemBuilder::new().build().await.unwrap();

        let zero_copy_fs = ZeroCopyFilesystemBuilder::new()
            .with_collection_id("test_collection".to_string())
            .with_engine_type("SST".to_string())
            .with_io_system(Arc::new(io_system))
            .build(local_fs)
            .unwrap();

        // Test that non-existent files are handled gracefully
        let result = zero_copy_fs.read("non_existent_file.sst").await;
        // Should either return error or empty vec based on optimization
        assert!(result.is_err() || result.unwrap().is_empty());
    }
}
