//! Global Manifest Singleton with Convenience Functions

use anyhow::{anyhow, Context, Result};
use once_cell::sync::OnceCell;
use std::sync::Arc;
use tracing::{info, warn};

use super::{GlobalManifestService, GlobalManifestServiceConfig, GlobalManifestEntry, WalEntryStatus};
use crate::storage::persistence::write_ahead_log::config::WALConfig;

/// Global singleton instance
static GLOBAL_MANIFEST: OnceCell<Arc<GlobalManifestService>> = OnceCell::new();

/// Initialize the global manifest (call once during server startup)
pub async fn init(config: &WALConfig) -> Result<Arc<GlobalManifestService>> {
    if let Some(service) = GLOBAL_MANIFEST.get() {
        info!("✅ Global manifest already initialized");
        return Ok(service.clone());
    }

    info!("🌐 Initializing global WAL manifest");
    info!("🔍 DEBUG: Config has {} data directories", config.multi_disk.data_directories.len());
    info!("🔍 DEBUG: Config global_manifest_url: {:?}", config.global_manifest_url);

    // Get manifest location from explicit config or fallback to primary disk
    let wal_base_url = if let Some(ref explicit_url) = config.global_manifest_url {
        info!("📌 Using explicit global manifest location: {}", explicit_url);
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
        info!("📁 Using default manifest location (primary disk): {}", url);
        url
    };

    info!("🔍 DEBUG: Final wal_base_url: {}", wal_base_url);

    if config.multi_disk.data_directories.len() > 1 {
        info!("📊 Multi-disk mode: {} data disks, 1 global manifest location",
              config.multi_disk.data_directories.len());
    }

    let filesystem_factory = Arc::new(
        crate::storage::persistence::filesystem::FilesystemFactory::create_default()
            .await
            .context("Failed to create filesystem factory")?
    );

    let service = GlobalManifestService::new(
        GlobalManifestServiceConfig::default(),
        filesystem_factory,
        wal_base_url,
    ).await?;

    match GLOBAL_MANIFEST.set(service.clone()) {
        Ok(()) => {
            info!("✅ Global manifest initialized");
            Ok(service)
        }
        Err(_) => {
            warn!("⚠️  Manifest initialized by another thread");
            Ok(GLOBAL_MANIFEST.get().unwrap().clone())
        }
    }
}

/// Get the global manifest service (returns None if not initialized)
pub fn get_service() -> Option<Arc<GlobalManifestService>> {
    GLOBAL_MANIFEST.get().cloned()
}

/// Get or initialize with default config (not recommended, use init() instead)
pub async fn get_or_init() -> Result<Arc<GlobalManifestService>> {
    if let Some(service) = GLOBAL_MANIFEST.get() {
        return Ok(service.clone());
    }

    warn!("⚠️  Manifest not initialized, using default config");
    init(&WALConfig::default()).await
}

/// Shutdown the global manifest
pub async fn shutdown() -> Result<()> {
    if let Some(service) = GLOBAL_MANIFEST.get() {
        info!("🛑 Shutting down global manifest");
        service.shutdown().await?;
        info!("✅ Manifest shut down");
    }
    Ok(())
}

// Convenience functions

/// Append entry asynchronously (high performance)
pub async fn append_async(entry: GlobalManifestEntry) -> Result<()> {
    let service = get_service()
        .ok_or_else(|| anyhow!("Global manifest not initialized"))?;
    service.append_async(entry).await
}

/// Append entry synchronously (waits for disk write)
pub async fn append_sync(entry: GlobalManifestEntry) -> Result<()> {
    let service = get_service()
        .ok_or_else(|| anyhow!("Global manifest not initialized"))?;
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
    let service = get_service()
        .ok_or_else(|| anyhow!("Global manifest not initialized"))?;
    service.create_checkpoint().await
}

/// Cleanup checkpointed entries
pub async fn cleanup_checkpointed() -> Result<usize> {
    let service = get_service()
        .ok_or_else(|| anyhow!("Global manifest not initialized"))?;
    service.cleanup_checkpointed_entries().await
}

/// Mark entries as flushed
pub async fn mark_flushed(batch_ids: &[String]) -> Result<()> {
    let service = get_service()
        .ok_or_else(|| anyhow!("Global manifest not initialized"))?;
    service.mark_flushed(batch_ids).await
}
