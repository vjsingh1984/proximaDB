// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! WAL Disk Manager - Multi-disk collection-organized WAL storage

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
// TODO: Re-enable when async file operations are implemented
// use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;

use super::config::{DiskDistributionStrategy, SyncMode, WalConfig};
use super::{FlushResult, WalEntry};
use super::avro::{AvroWalEntry, AvroWalOperation, AvroOpType};
use crate::core::CollectionId;
use crate::storage::persistence::filesystem::{FileOptions, FilesystemFactory};

/// Disk segment information
#[derive(Debug, Clone)]
pub struct DiskSegment {
    /// Segment file path
    pub path: PathBuf,

    /// Segment sequence range
    pub sequence_range: (u64, u64),

    /// File size in bytes
    pub size_bytes: u64,

    /// Number of entries
    pub entry_count: u64,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp
    pub modified_at: DateTime<Utc>,

    /// Compression ratio (if compressed)
    pub compression_ratio: f64,
}

/// Collection disk layout
#[derive(Debug)]
struct CollectionDiskLayout {
    /// Collection ID
    collection_id: CollectionId,

    /// Assigned disk index
    disk_index: usize,

    /// Base directory for this collection
    base_directory: PathBuf,

    /// Current segment sequence
    current_segment: u64,

    /// Segment files
    segments: Vec<DiskSegment>,

    /// Total disk usage
    total_size_bytes: u64,
}

impl CollectionDiskLayout {
    fn new(collection_id: CollectionId, disk_index: usize, base_directory: PathBuf) -> Self {
        Self {
            collection_id,
            disk_index,
            base_directory,
            current_segment: 0,
            segments: Vec::new(),
            total_size_bytes: 0,
        }
    }

    /// Get next segment path
    fn next_segment_path(&mut self) -> PathBuf {
        self.current_segment += 1;
        self.base_directory
            .join(format!("segment_{:08}.wal", self.current_segment))
    }

    /// Add segment to layout
    fn add_segment(&mut self, segment: DiskSegment) {
        self.total_size_bytes += segment.size_bytes;
        self.segments.push(segment);
    }

    /// Get all segment paths for reading
    fn get_all_segment_paths(&self) -> Vec<PathBuf> {
        self.segments.iter().map(|s| s.path.clone()).collect()
    }
}

/// WAL disk manager with multi-disk support
#[derive(Debug)]
pub struct WalDiskManager {
    /// Filesystem factory for multi-backend support
    filesystem: Arc<FilesystemFactory>,

    /// Configuration
    config: WalConfig,

    /// Collection layouts mapped to disks
    collection_layouts: Arc<RwLock<HashMap<CollectionId, CollectionDiskLayout>>>,

    /// Disk usage tracking
    disk_usage: Arc<RwLock<Vec<u64>>>, // bytes per disk

    /// Disk directories as URLs
    disk_directories: Vec<String>,
    
    /// Sequence numbers per collection for WAL file ordering
    sequence_counters: Arc<RwLock<HashMap<CollectionId, u64>>>,
}

impl WalDiskManager {
    /// Create new disk manager
    pub async fn new(config: WalConfig, filesystem: Arc<FilesystemFactory>) -> Result<Self> {
        let disk_directories = config.multi_disk.data_directories.clone();

        // Initialize directories using filesystem API
        for url in &disk_directories {
            let fs = filesystem.get_filesystem(url)
                .with_context(|| format!("Failed to get filesystem for WAL URL: {}", url))?;
            
            // Extract path for creation
            let path = if url.starts_with("file://") {
                url.strip_prefix("file://").unwrap_or(url)
            } else {
                url
            };
            
            fs.create_dir_all(path)
                .await
                .with_context(|| format!("Failed to create WAL directory: {}", path))?;
        }

        let disk_usage = vec![0u64; disk_directories.len()];

        let manager = Self {
            filesystem,
            config,
            collection_layouts: Arc::new(RwLock::new(HashMap::new())),
            disk_usage: Arc::new(RwLock::new(disk_usage)),
            disk_directories,
            sequence_counters: Arc::new(RwLock::new(HashMap::new())),
        };

        // Recover existing layouts
        manager.recover_layouts().await?;

        Ok(manager)
    }

    /// Get next sequence number for a collection (for sequential WAL files)
    async fn get_next_sequence(&self, collection_id: &CollectionId) -> Result<u64> {
        let mut counters = self.sequence_counters.write().await;
        let current = counters.entry(collection_id.clone()).or_insert(0);
        *current += 1;
        Ok(*current)
    }



    /// Write raw serialized data for a collection (used by strategies)
    pub async fn write_raw(
        &self,
        collection_id: &CollectionId,
        data: Vec<u8>,
    ) -> Result<FlushResult> {
        if data.is_empty() {
            return Ok(FlushResult {
                entries_flushed: 0,
                bytes_written: 0,
                segments_created: 0,
                collections_affected: vec![],
                flush_duration_ms: 0,
            });
        }

        let start_time = std::time::Instant::now();

        // Get or create collection layout
        self.get_or_create_layout(collection_id).await?;

        // Create new segment
        let segment_path = {
            let mut layouts = self.collection_layouts.write().await;
            let layout = layouts.get_mut(collection_id).unwrap();
            layout.next_segment_path()
        };

        // Write to filesystem
        let bytes_written = self.write_segment(&segment_path, &data).await?;

        // Update layout
        let segment = DiskSegment {
            path: segment_path,
            sequence_range: (0, 0), // Unknown for raw data
            size_bytes: bytes_written,
            entry_count: 0, // Unknown for raw data
            created_at: Utc::now(),
            modified_at: Utc::now(),
            compression_ratio: 1.0, // Assume already compressed by strategy
        };

        {
            let mut layouts = self.collection_layouts.write().await;
            if let Some(layout) = layouts.get_mut(collection_id) {
                layout.add_segment(segment);

                // Update disk usage
                let mut disk_usage = self.disk_usage.write().await;
                disk_usage[layout.disk_index] += bytes_written;
            }
        }

        let flush_duration = start_time.elapsed().as_millis() as u64;

        Ok(FlushResult {
            entries_flushed: 0, // Unknown for raw data
            bytes_written,
            segments_created: 1,
            collections_affected: vec![collection_id.clone()],
            flush_duration_ms: flush_duration,
        })
    }

    /// Recover layouts from existing disk structure
    async fn recover_layouts(&self) -> Result<()> {
        let mut layouts = self.collection_layouts.write().await;
        let mut disk_usage = self.disk_usage.write().await;

        for (disk_index, disk_dir) in self.disk_directories.iter().enumerate() {
            let mut disk_total = 0u64;

            // Convert URL to filesystem path
            let path = if disk_dir.starts_with("file://") {
                disk_dir.strip_prefix("file://").unwrap_or(disk_dir)
            } else {
                disk_dir
            };

            // Scan for collection directories
            let mut entries = fs::read_dir(path).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(collection_name) = path.file_name().and_then(|n| n.to_str()) {
                        let collection_id = CollectionId::from(collection_name.to_string());

                        let mut layout = CollectionDiskLayout::new(
                            collection_id.clone(),
                            disk_index,
                            path.clone(),
                        );

                        // Scan for segment files
                        let mut segment_entries = fs::read_dir(&path).await?;
                        while let Some(segment_entry) = segment_entries.next_entry().await? {
                            let segment_path = segment_entry.path();
                            if segment_path.extension().and_then(|e| e.to_str()) == Some("wal") {
                                if let Ok(metadata) = segment_entry.metadata().await {
                                    let size = metadata.len();
                                    disk_total += size;

                                    // Create segment info (simplified recovery)
                                    let segment = DiskSegment {
                                        path: segment_path,
                                        sequence_range: (0, 0), // Will be read if needed
                                        size_bytes: size,
                                        entry_count: 0, // Will be calculated if needed
                                        created_at: metadata
                                            .created()
                                            .map(|t| DateTime::from(t))
                                            .unwrap_or_else(|_| Utc::now()),
                                        modified_at: metadata
                                            .modified()
                                            .map(|t| DateTime::from(t))
                                            .unwrap_or_else(|_| Utc::now()),
                                        compression_ratio: 1.0,
                                    };

                                    layout.add_segment(segment);
                                }
                            }
                        }

                        if !layout.segments.is_empty() {
                            layouts.insert(collection_id, layout);
                        }
                    }
                }
            }

            disk_usage[disk_index] = disk_total;
        }

        Ok(())
    }

    /// Get or create layout for collection
    async fn get_or_create_layout(&self, collection_id: &CollectionId) -> Result<()> {
        let mut layouts = self.collection_layouts.write().await;

        if !layouts.contains_key(collection_id) {
            // Assign disk based on strategy
            let disk_index = self.assign_disk_for_collection(collection_id).await?;
            let base_url = &self.disk_directories[disk_index];
            
            // Build collection directory URL
            let collection_dir_url = if base_url.starts_with("file://") {
                let base_path = base_url.strip_prefix("file://").unwrap_or(base_url);
                format!("{}/{}", base_path, collection_id.as_str())
            } else {
                format!("{}/{}", base_url.trim_end_matches('/'), collection_id.as_str())
            };
            
            // Create collection directory using filesystem API
            let fs = self.filesystem.get_filesystem(base_url)
                .with_context(|| format!("Failed to get filesystem for URL: {}", base_url))?;
            
            fs.create_dir_all(&collection_dir_url).await.with_context(|| {
                format!("Failed to create collection directory: {}", collection_dir_url)
            })?;
            
            // Convert back to PathBuf for compatibility with existing code
            let base_dir = if base_url.starts_with("file://") {
                PathBuf::from(collection_dir_url)
            } else {
                PathBuf::from(&collection_dir_url)
            };

            let layout = CollectionDiskLayout::new(collection_id.clone(), disk_index, base_dir);

            layouts.insert(collection_id.clone(), layout);
        }

        Ok(())
    }

    /// Assign disk for new collection
    async fn assign_disk_for_collection(&self, collection_id: &CollectionId) -> Result<usize> {
        let disk_usage = self.disk_usage.read().await;

        match self.config.multi_disk.distribution_strategy {
            DiskDistributionStrategy::RoundRobin => {
                // Simple round-robin based on collection count
                let layouts = self.collection_layouts.read().await;
                Ok(layouts.len() % self.disk_directories.len())
            }
            DiskDistributionStrategy::Hash => {
                // Consistent hash-based distribution
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};

                let mut hasher = DefaultHasher::new();
                collection_id.hash(&mut hasher);
                let hash = hasher.finish();

                Ok((hash as usize) % self.disk_directories.len())
            }
            DiskDistributionStrategy::LoadBalanced => {
                // Choose disk with least usage
                let min_usage_disk = disk_usage
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, &usage)| usage)
                    .map(|(index, _)| index)
                    .unwrap_or(0);

                Ok(min_usage_disk)
            }
        }
    }

    /// Write segment to filesystem
    async fn write_segment(&self, path: &Path, data: &[u8]) -> Result<u64> {
        // Use filesystem factory for multi-backend support
        let url = format!("file://{}", path.to_string_lossy());

        self.filesystem
            .write(&url, data, None)
            .await
            .with_context(|| format!("Failed to write WAL segment: {:?}", path))?;

        // Sync based on configuration
        match self.config.performance.sync_mode {
            SyncMode::Always | SyncMode::PerBatch => {
                self.filesystem
                    .sync(&url)
                    .await
                    .with_context(|| format!("Failed to sync WAL segment: {:?}", path))?;
            }
            _ => {
                // No immediate sync
            }
        }

        Ok(data.len() as u64)
    }


    /// Estimate compression ratio
    fn estimate_compression_ratio(&self, entries: &[WalEntry], compressed_size: u64) -> f64 {
        let uncompressed_size: usize = entries.iter().map(|e| self.estimate_entry_size(e)).sum();

        if uncompressed_size > 0 {
            uncompressed_size as f64 / compressed_size as f64
        } else {
            1.0
        }
    }

    /// Estimate entry size
    fn estimate_entry_size(&self, entry: &WalEntry) -> usize {
        // Simplified estimation
        std::mem::size_of::<WalEntry>()
            + match &entry.operation {
                super::WalOperation::Insert { record, .. }
                | super::WalOperation::Update { record, .. } => {
                    record.vector.len() * std::mem::size_of::<f32>() + record.metadata.len() * 32
                    // Estimate 32 bytes per metadata entry
                }
                _ => 64,
            }
    }

    /// Calculate average compression ratio
    async fn calculate_average_compression_ratio(
        &self,
        layouts: &HashMap<CollectionId, CollectionDiskLayout>,
    ) -> f64 {
        let ratios: Vec<f64> = layouts
            .values()
            .flat_map(|l| l.segments.iter().map(|s| s.compression_ratio))
            .collect();

        if ratios.is_empty() {
            1.0
        } else {
            ratios.iter().sum::<f64>() / ratios.len() as f64
        }
    }

    /// List all collections that have WAL data on disk
    pub async fn list_collections(&self) -> Result<Vec<CollectionId>> {
        let layouts = self.collection_layouts.read().await;
        Ok(layouts.keys().cloned().collect())
    }

    /// Read WAL entries for a collection from disk (stub implementation)
    pub async fn read_entries(
        &self,
        _collection_id: &CollectionId,
        _from_sequence: u64,
        _limit: Option<usize>,
    ) -> Result<Vec<WalEntry>> {
        // TODO: Implement actual disk reading with deserializer
        // For now, return empty vector since disk reading is not fully implemented
        tracing::debug!("🚧 WAL DISK: read_entries not yet implemented, returning empty result");
        Ok(Vec::new())
    }

    /// Get WAL statistics from disk
    pub async fn get_stats(&self) -> Result<DiskStats> {
        let layouts = self.collection_layouts.read().await;
        let disk_usage = self.disk_usage.read().await;

        let total_segments = layouts.values().map(|l| l.segments.len() as u64).sum();
        let total_size_bytes = layouts
            .values()
            .flat_map(|l| l.segments.iter())
            .map(|s| s.size_bytes)
            .sum();

        let disk_distribution = self
            .disk_directories
            .iter()
            .enumerate()
            .map(|(i, dir)| DiskUsage {
                directory: if dir.starts_with("file://") {
                    PathBuf::from(dir.strip_prefix("file://").unwrap_or(dir))
                } else {
                    PathBuf::from(dir.clone())
                },
                usage_bytes: disk_usage.get(i).copied().unwrap_or(0),
                collections: layouts
                    .values()
                    .filter(|l| l.disk_index == i)
                    .map(|l| l.collection_id.clone())
                    .collect(),
            })
            .collect();

        Ok(DiskStats {
            total_segments,
            total_size_bytes,
            collections_count: layouts.len(),
            disk_distribution,
            compression_ratio: self.calculate_average_compression_ratio(&layouts).await,
        })
    }

    /// Drop all WAL data for a collection
    pub async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()> {
        let mut layouts = self.collection_layouts.write().await;

        if let Some(layout) = layouts.remove(collection_id) {
            // Remove all segment files
            for segment in &layout.segments {
                if segment.path.exists() {
                    if let Err(e) = fs::remove_file(&segment.path).await {
                        tracing::warn!("Failed to remove WAL segment {:?}: {}", segment.path, e);
                    }
                }
            }

            // Remove collection directory if empty
            if layout.base_directory.exists() {
                if let Err(e) = fs::remove_dir(&layout.base_directory).await {
                    tracing::debug!(
                        "Collection directory not empty or failed to remove: {:?}: {}",
                        layout.base_directory,
                        e
                    );
                }
            }

            tracing::info!("✅ Dropped WAL disk data for collection: {}", collection_id);
        }

        Ok(())
    }

    /// Append WAL entries directly to disk for immediate durability
    /// This method provides immediate persistence guarantee for crash recovery
    pub async fn append_wal_entries(&self, entries: &[WalEntry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        use std::collections::HashMap;
        
        // Group entries by collection for efficient I/O
        let mut by_collection: HashMap<CollectionId, Vec<&WalEntry>> = HashMap::new();
        for entry in entries {
            by_collection.entry(entry.collection_id.clone())
                .or_default()
                .push(entry);
        }

        // Write to collection-specific WAL files
        for (collection_id, collection_entries) in by_collection {
            let entry_count = collection_entries.len(); // Store count before move
            
            // Get WAL file path with error handling
            let wal_path = match self.get_wal_file_path(&collection_id).await {
                Ok(path) => path,
                Err(e) => {
                    tracing::warn!(
                        "⚠️ Failed to get WAL file path for {}: {}. Skipping disk write.",
                        collection_id, e
                    );
                    continue; // Skip this collection but continue with others
                }
            };
            
            // Serialize entries to Avro format with error handling
            tracing::info!("💾 [DISK] About to serialize {} WAL entries for collection {}", 
                          collection_entries.len(), collection_id);
            
            let avro_data = match self.serialize_entries_to_avro(collection_entries).await {
                Ok(data) => {
                    tracing::info!("💾 [DISK] ✅ Successfully serialized {} bytes for collection {}", 
                                  data.len(), collection_id);
                    data
                },
                Err(e) => {
                    tracing::error!(
                        "💾 [DISK] ❌ CRITICAL: Failed to serialize WAL entries for {}: {}. Skipping disk write.",
                        collection_id, e
                    );
                    tracing::error!("💾 [DISK] This means ZERO durability for collection {}", collection_id);
                    continue; // Skip this collection but continue with others
                }
            };
            
            // Write to sequential WAL file (each batch gets a new sequence file) with robust error handling
            tracing::info!("💾 [DISK] About to write {} bytes to WAL file: {}", avro_data.len(), wal_path);
            
            match self.filesystem.get_filesystem("file://") {
                Ok(filesystem) => {
                    tracing::debug!("💾 [DISK] Got filesystem for writing to {}", wal_path);
                    
                    let file_options = FileOptions {
                        overwrite: false, // Each file is unique by sequence/timestamp
                        create_dirs: true,
                        ..Default::default()
                    };
                    
                    tracing::debug!("💾 [DISK] File options: overwrite=false, create_dirs=true");
                    
                    filesystem.write_atomic(&wal_path, &avro_data, Some(file_options)).await
                        .map_err(|e| {
                            tracing::error!(
                                "💾 [DISK] ❌ CRITICAL: WAL sequential file write failed for {}: {}",
                                collection_id, e
                            );
                            anyhow::anyhow!("WAL disk write failed: {}", e)
                        })?;
                    
                    tracing::info!("💾 [DISK] ✅ SUCCESS: WAL file written: {} ({} bytes) - DURABILITY ACHIEVED", 
                                  wal_path, avro_data.len());
                },
                Err(e) => {
                    tracing::error!(
                        "💾 [DISK] ❌ CRITICAL: Failed to get filesystem for WAL write: {}",
                        e
                    );
                    return Err(anyhow::anyhow!("Failed to get filesystem for WAL write: {}", e));
                }
            }

            tracing::debug!(
                "📝 WAL: Appended {} entries to {} ({} bytes)",
                entry_count,
                wal_path,
                avro_data.len()
            );
        }

        Ok(())
    }

    /// Get WAL file path for collection
    async fn get_wal_file_path(&self, collection_id: &CollectionId) -> Result<String> {
        // Create directory if it doesn't exist - absolute path format
        let base_url = &self.disk_directories[0];
        let dir_path = if base_url.starts_with("file://") {
            let base_path = base_url.strip_prefix("file://").unwrap_or(base_url);
            format!("{}/{}", base_path, collection_id)
        } else {
            format!("{}/{}", base_url.trim_end_matches('/'), collection_id)
        };
        
        if let Err(e) = fs::create_dir_all(&dir_path).await {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(anyhow::anyhow!("Failed to create WAL directory {}: {}", dir_path, e));
            }
        }
        
        // WAL file format: {collection_id}/wal_{sequence:010}_{timestamp}.avro
        // This enables point-in-time recovery and safe deletion of flushed segments
        let timestamp = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let sequence = self.get_next_sequence(collection_id).await?;
        let wal_file = format!("{}/wal_{:010}_{}.avro", dir_path, sequence, timestamp);
        Ok(wal_file)
    }

    /// Serialize WAL entries to PROPER Avro binary format for immediate persistence
    async fn serialize_entries_to_avro(&self, entries: Vec<&WalEntry>) -> Result<Vec<u8>> {
        use apache_avro::{Schema, Writer, Codec};
        
        tracing::info!("💾 [DISK] Starting Avro serialization for {} WAL entries", entries.len());
        
        // Use the canonical Avro schema from the schema module
        let schema = Schema::parse_str(super::schema::AVRO_SCHEMA_V1)
            .context("Failed to parse Avro schema")?;
        
        tracing::debug!("💾 [DISK] Avro schema parsed successfully");
        tracing::debug!("💾 [DISK] Schema: {}", super::schema::AVRO_SCHEMA_V1);
        
        // Create Avro writer with Deflate compression
        let mut writer = Writer::with_codec(&schema, Vec::new(), Codec::Deflate);
        
        tracing::debug!("💾 [DISK] Avro writer created with Deflate compression");
        
        // Serialize each WAL entry to Avro format using the canonical conversion function
        for (i, entry) in entries.iter().enumerate() {
            tracing::debug!("💾 [DISK] Processing WAL entry {}/{}: {}", i + 1, entries.len(), entry.entry_id);
            tracing::debug!("💾 [DISK] Entry details: collection={}, operation={:?}", entry.collection_id, entry.operation);
            
            let avro_entry = match super::schema::convert_to_avro_entry(entry) {
                Ok(avro_entry) => {
                    tracing::debug!("💾 [DISK] ✅ Converted WAL entry {} to AvroWalEntry", entry.entry_id);
                    tracing::debug!("💾 [DISK] AvroWalEntry: {:?}", avro_entry);
                    avro_entry
                },
                Err(e) => {
                    tracing::error!("💾 [DISK] ❌ Failed to convert WAL entry {} to AvroWalEntry: {}", entry.entry_id, e);
                    tracing::error!("💾 [DISK] Original entry: {:?}", entry);
                    return Err(anyhow::anyhow!("Failed to convert WAL entry to Avro: {}", e));
                }
            };
            
            // Convert to Avro Value first, then append
            let avro_value = match apache_avro::to_value(&avro_entry) {
                Ok(value) => {
                    tracing::debug!("💾 [DISK] ✅ Converted AvroWalEntry {} to Avro Value", entry.entry_id);
                    tracing::debug!("💾 [DISK] Avro Value type: {:?}", value);
                    value
                },
                Err(e) => {
                    tracing::error!("💾 [DISK] ❌ Failed to convert AvroWalEntry {} to Avro Value: {}", entry.entry_id, e);
                    tracing::error!("💾 [DISK] AvroWalEntry that failed: {:?}", avro_entry);
                    return Err(anyhow::anyhow!("Failed to convert WAL entry to Avro value: {}", e));
                }
            };
            
            if let Err(e) = writer.append(avro_value) {
                tracing::error!("💾 [DISK] ❌ Failed to append Avro Value for entry {} to writer: {}", entry.entry_id, e);
                return Err(anyhow::anyhow!("Failed to append WAL entry to Avro writer: {}", e));
            }
            
            tracing::debug!("💾 [DISK] ✅ Successfully appended WAL entry {} to Avro writer", entry.entry_id);
        }
        
        // Get the binary Avro data
        tracing::debug!("💾 [DISK] Finalizing Avro writer...");
        let avro_data = writer.into_inner()
            .context("Failed to finalize Avro writer")?;
            
        tracing::info!("💾 [DISK] ✅ Successfully serialized {} WAL entries to {} bytes of Avro data", 
                      entries.len(), avro_data.len());
        
        Ok(avro_data)
    }
    
}

/// Disk usage information
#[derive(Debug, Clone)]
pub struct DiskUsage {
    pub directory: PathBuf,
    pub usage_bytes: u64,
    pub collections: Vec<CollectionId>,
}

/// Disk statistics
#[derive(Debug, Clone)]
pub struct DiskStats {
    pub total_segments: u64,
    pub total_size_bytes: u64,
    pub collections_count: usize,
    pub disk_distribution: Vec<DiskUsage>,
    pub compression_ratio: f64,
}
