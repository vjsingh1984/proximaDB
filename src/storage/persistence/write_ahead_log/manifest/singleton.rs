//! Global Manifest Singleton with Convenience Functions
//!
//! Supports both server mode (single instance) and embedded mode (multiple instances).
//! In embedded mode, call `reset()` before creating a new database instance.

use anyhow::{Context, Result, anyhow};
use std::sync::{Arc, RwLock};
use tracing::{debug, info, trace, warn};

use super::{
    GlobalManifestEntry, GlobalManifestService, GlobalManifestServiceConfig, WalEntryStatus,
};
use crate::storage::persistence::write_ahead_log::config::WALConfig;

/// Global singleton instance - uses RwLock to support reset in embedded mode
static GLOBAL_MANIFEST: RwLock<Option<Arc<GlobalManifestService>>> = RwLock::new(None);

/// Initialize the global manifest (call once during server startup)
pub async fn init(config: &WALConfig) -> Result<Arc<GlobalManifestService>> {
    // Check if already initialized
    {
        let guard = GLOBAL_MANIFEST
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;
        if let Some(ref service) = *guard {
            debug!("Global manifest already initialized");
            return Ok(service.clone());
        }
    }

    info!("Initializing global WAL manifest");
    debug!(
        "Config has {} data directories",
        config.multi_disk.data_directories.len()
    );
    debug!(
        "Config global_manifest_url: {:?}",
        config.global_manifest_url
    );

    // Get manifest location from explicit config or fallback to primary disk
    let wal_base_url = if let Some(ref explicit_url) = config.global_manifest_url {
        debug!(
            "Using explicit global manifest location: {}",
            explicit_url
        );
        explicit_url.clone()
    } else {
        // Fallback: first data directory + /wal
        let primary_disk = config
            .multi_disk
            .data_directories
            .first()
            .cloned()
            .unwrap_or_else(|| "file://./data".to_string());
        let url = format!("{}/wal", primary_disk.trim_end_matches('/'));
        debug!("Using default manifest location (primary disk): {}", url);
        url
    };

    trace!("Final wal_base_url: {}", wal_base_url);

    if config.multi_disk.data_directories.len() > 1 {
        info!(
            "Multi-disk mode: {} data disks, 1 global manifest location",
            config.multi_disk.data_directories.len()
        );
    }

    let filesystem_factory = Arc::new(
        crate::storage::persistence::filesystem::FilesystemFactory::create_default()
            .await
            .context("Failed to create filesystem factory")?,
    );

    let service = GlobalManifestService::new(
        GlobalManifestServiceConfig::default(),
        filesystem_factory,
        wal_base_url,
    )
    .await?;

    // Store in singleton
    {
        let mut guard = GLOBAL_MANIFEST
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;

        // Double-check in case another thread initialized while we were creating
        if guard.is_some() {
            warn!("Manifest initialized by another thread");
            return Ok(guard.as_ref().unwrap().clone());
        }

        *guard = Some(service.clone());
    }

    info!("Global manifest initialized");
    Ok(service)
}

/// Get the global manifest service (returns None if not initialized)
pub fn get_service() -> Option<Arc<GlobalManifestService>> {
    GLOBAL_MANIFEST
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}

/// Reset the global manifest singleton (for embedded mode)
///
/// This shuts down the existing manifest and clears the singleton so a new one
/// can be initialized with different configuration. Call this before creating
/// a new EmbeddedProximaDB instance with a different storage location.
pub async fn reset() -> Result<()> {
    // First shutdown the existing service if any
    let service = {
        let guard = GLOBAL_MANIFEST
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;
        guard.clone()
    };

    if let Some(svc) = service {
        debug!("Shutting down existing global manifest before reset");
        if let Err(e) = svc.shutdown().await {
            warn!("Error during manifest shutdown (continuing with reset): {}", e);
        }
    }

    // Clear the singleton
    {
        let mut guard = GLOBAL_MANIFEST
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;
        *guard = None;
    }

    debug!("Global manifest singleton reset");
    Ok(())
}

/// Get or initialize with default config (not recommended, use init() instead)
pub async fn get_or_init() -> Result<Arc<GlobalManifestService>> {
    if let Some(service) = get_service() {
        return Ok(service);
    }

    warn!("Manifest not initialized, using default config");
    init(&WALConfig::default()).await
}

/// Shutdown the global manifest
pub async fn shutdown() -> Result<()> {
    if let Some(service) = get_service() {
        info!("Shutting down global manifest");
        service.shutdown().await?;
        info!("Manifest shut down");
    }
    Ok(())
}

// Convenience functions

/// Append entry asynchronously (high performance)
pub async fn append_async(entry: GlobalManifestEntry) -> Result<()> {
    let service = get_service().ok_or_else(|| anyhow!("Global manifest not initialized"))?;
    service.append_async(entry).await
}

/// Append entry synchronously (waits for disk write)
pub async fn append_sync(entry: GlobalManifestEntry) -> Result<()> {
    let service = get_service().ok_or_else(|| anyhow!("Global manifest not initialized"))?;
    service.append_sync(entry).await
}

/// Get all active entries (not yet flushed)
pub async fn get_active_entries() -> Vec<GlobalManifestEntry> {
    match get_service() {
        Some(service) => service.get_active_entries().await,
        None => Vec::new(),
    }
}

/// Get entries for a specific collection
pub async fn get_collection_entries(collection_id: &str) -> Vec<GlobalManifestEntry> {
    match get_service() {
        Some(service) => service.get_collection_entries(collection_id).await,
        None => Vec::new(),
    }
}

/// Get all entries sorted by LSN
pub async fn get_all_entries() -> Vec<GlobalManifestEntry> {
    match get_service() {
        Some(service) => service.get_all_entries().await,
        None => Vec::new(),
    }
}

/// Create a checkpoint
pub async fn create_checkpoint() -> Result<super::GlobalCheckpoint> {
    let service = get_service().ok_or_else(|| anyhow!("Global manifest not initialized"))?;
    service.create_checkpoint().await
}

/// Cleanup checkpointed entries
pub async fn cleanup_checkpointed() -> Result<usize> {
    let service = get_service().ok_or_else(|| anyhow!("Global manifest not initialized"))?;
    service.cleanup_checkpointed_entries().await
}

/// Mark entries as flushed
pub async fn mark_flushed(batch_ids: &[String]) -> Result<()> {
    let service = get_service().ok_or_else(|| anyhow!("Global manifest not initialized"))?;
    service.mark_flushed(batch_ids).await
}

/// Mark entries as flushed AND delete the actual WAL files from disk
///
/// This is the recommended function to use after a successful flush to storage.
/// It marks entries as flushed first (for crash safety), then deletes the WAL files.
pub async fn mark_flushed_and_delete_files(batch_ids: &[String]) -> Result<usize> {
    let service = get_service().ok_or_else(|| anyhow!("Global manifest not initialized"))?;
    service.mark_flushed_and_delete_files(batch_ids).await
}
