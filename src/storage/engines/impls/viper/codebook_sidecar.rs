//! VIPER Codebook Sidecar Storage
//!
//! Implements sidecar file storage for quantization codebooks in VIPER's Parquet format.
//! Codebooks are stored as separate JSON files alongside Parquet data files.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::storage::engines::core::formats::codebook_metadata::{
    CodebookSerializer, QuantizationCodebookMetadata,
};
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;
use crate::storage::persistence::filesystem::FileSystem;
use crate::compute::quantization::unified::UnifiedQuantizationEngine;

/// VIPER-specific codebook sidecar manager
pub struct ViperCodebookSidecarManager {
    serializer: CodebookSerializer,
    collection_id: String,
    filesystem: Arc<dyn FileSystem>,
}

impl ViperCodebookSidecarManager {
    /// Create new VIPER codebook sidecar manager
    pub fn new(
        collection_id: String,
        filesystem: Arc<dyn FileSystem>,
    ) -> Self {
        Self {
            serializer: CodebookSerializer::new(),
            collection_id,
            filesystem,
        }
    }

    /// Generate sidecar filename from Parquet file path
    pub fn sidecar_path(parquet_path: &Path) -> PathBuf {
        let mut sidecar_path = parquet_path.to_path_buf();
        let filename = parquet_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        sidecar_path.set_file_name(format!("{}.codebook.json", filename));
        sidecar_path
    }

    /// Write codebook metadata as sidecar file
    pub async fn write_sidecar(
        &self,
        parquet_path: &Path,
        metadata: &QuantizationCodebookMetadata,
    ) -> Result<()> {
        let sidecar_path = Self::sidecar_path(parquet_path);
        let json = self.serializer.serialize_for_sidecar(metadata)?;

        // Write through unified filesystem
        self.filesystem
            .write(sidecar_path.to_str().unwrap(), json.as_bytes(), None)
            .await
            .context("Failed to write codebook sidecar file")?;

        info!(
            "VIPER: Wrote codebook sidecar for {} with {} PQ codebooks",
            parquet_path.display(),
            metadata.pq_codebooks.len()
        );

        Ok(())
    }

    /// Read codebook metadata from sidecar file
    pub async fn read_sidecar(&self, parquet_path: &Path) -> Result<Option<QuantizationCodebookMetadata>> {
        let sidecar_path = Self::sidecar_path(parquet_path);

        // Check if sidecar exists
        if !self.filesystem.exists(sidecar_path.to_str().unwrap()).await? {
            debug!(
                "VIPER: No codebook sidecar found for {}",
                parquet_path.display()
            );
            return Ok(None);
        }

        // Read through unified filesystem
        let json_bytes = self.filesystem
            .read(sidecar_path.to_str().unwrap())
            .await
            .context("Failed to read codebook sidecar file")?;

        let json = String::from_utf8(json_bytes.to_vec())
            .context("Invalid UTF-8 in codebook sidecar")?;

        let metadata = self.serializer.deserialize_from_sidecar(&json)?;

        debug!(
            "VIPER: Read codebook sidecar for {} with {} PQ codebooks",
            parquet_path.display(),
            metadata.pq_codebooks.len()
        );

        Ok(Some(metadata))
    }

    /// Delete sidecar file when Parquet file is deleted
    pub async fn delete_sidecar(&self, parquet_path: &Path) -> Result<()> {
        let sidecar_path = Self::sidecar_path(parquet_path);

        if self.filesystem.exists(sidecar_path.to_str().unwrap()).await? {
            self.filesystem
                .delete(sidecar_path.to_str().unwrap())
                .await
                .context("Failed to delete codebook sidecar file")?;

            info!(
                "VIPER: Deleted codebook sidecar for {}",
                parquet_path.display()
            );
        }

        Ok(())
    }

    /// List all sidecar files in a directory
    pub async fn list_sidecars(&self, directory: &Path) -> Result<Vec<PathBuf>> {
        let dir_entries = self.filesystem
            .list(directory.to_str().unwrap())
            .await
            .context("Failed to list codebook sidecar files")?;

        // Filter for codebook files
        Ok(dir_entries
            .into_iter()
            .filter(|entry| entry.name.ends_with(".codebook.json"))
            .map(|entry| PathBuf::from(entry.name))
            .collect())
    }

    /// Validate sidecar consistency with Parquet file
    pub async fn validate_consistency(
        &self,
        parquet_path: &Path,
        expected_dimension: usize,
    ) -> Result<bool> {
        if let Some(metadata) = self.read_sidecar(parquet_path).await? {
            // Check dimension consistency
            for (name, pq_codebook) in &metadata.pq_codebooks {
                if pq_codebook.dimension != expected_dimension {
                    warn!(
                        "VIPER: Dimension mismatch in codebook {} for {}: expected {}, got {}",
                        name,
                        parquet_path.display(),
                        expected_dimension,
                        pq_codebook.dimension
                    );
                    return Ok(false);
                }
            }

            // Check collection ID
            if metadata.collection_id != self.collection_id {
                warn!(
                    "VIPER: Collection ID mismatch in sidecar for {}: expected {}, got {}",
                    parquet_path.display(),
                    self.collection_id,
                    metadata.collection_id
                );
                return Ok(false);
            }

            Ok(true)
        } else {
            // No sidecar means no validation needed
            Ok(true)
        }
    }

    /// Migrate codebooks during compaction
    pub async fn migrate_during_compaction(
        &self,
        source_files: &[PathBuf],
        target_file: &Path,
        engine: &UnifiedQuantizationEngine,
    ) -> Result<()> {
        // Collect all codebooks from source files
        let mut merged_metadata: Option<QuantizationCodebookMetadata> = None;

        for source_file in source_files {
            if let Some(metadata) = self.read_sidecar(source_file).await? {
                if merged_metadata.is_none() {
                    merged_metadata = Some(metadata);
                } else {
                    // Merge codebooks (take latest)
                    // In production, might want more sophisticated merging
                    if metadata.created_at > merged_metadata.as_ref().unwrap().created_at {
                        merged_metadata = Some(metadata);
                    }
                }
            }
        }

        // If no existing codebooks, extract from engine
        let metadata = if let Some(m) = merged_metadata {
            m
        } else {
            self.serializer.extract_from_engine(engine, &self.collection_id).await?
        };

        // Write to target
        self.write_sidecar(target_file, &metadata).await?;

        // Clean up source sidecars
        for source_file in source_files {
            self.delete_sidecar(source_file).await?;
        }

        info!(
            "VIPER: Migrated codebooks from {} files to {}",
            source_files.len(),
            target_file.display()
        );

        Ok(())
    }
}

/// NOVA-specific extensions for progressive columnar storage
pub struct NovaCodebookSidecarManager {
    base: ViperCodebookSidecarManager,
    enable_progressive: bool,
}

impl NovaCodebookSidecarManager {
    /// Create NOVA-specific manager with progressive support
    pub fn new(
        collection_id: String,
        filesystem: Arc<dyn FileSystem>,
        enable_progressive: bool,
    ) -> Self {
        Self {
            base: ViperCodebookSidecarManager::new(collection_id, filesystem),
            enable_progressive,
        }
    }

    /// Write progressive codebooks with level indicators
    pub async fn write_progressive_sidecar(
        &self,
        parquet_path: &Path,
        metadata: &QuantizationCodebookMetadata,
        level: &str,
    ) -> Result<()> {
        // Modify path to include level
        let mut sidecar_path = ViperCodebookSidecarManager::sidecar_path(parquet_path);
        let filename = sidecar_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        sidecar_path.set_file_name(format!("{}.{}.codebook.json", filename, level));

        let json = self.base.serializer.serialize_for_sidecar(metadata)?;

        self.base.filesystem
            .write(sidecar_path.to_str().unwrap(), json.as_bytes(), None)
            .await
            .context("Failed to write progressive codebook sidecar")?;

        info!(
            "NOVA: Wrote progressive codebook sidecar for {} at level {}",
            parquet_path.display(),
            level
        );

        Ok(())
    }

    /// Read all progressive levels
    pub async fn read_all_levels(
        &self,
        parquet_path: &Path,
    ) -> Result<Vec<(String, QuantizationCodebookMetadata)>> {
        let directory = parquet_path.parent().unwrap_or(Path::new("."));
        let base_name = parquet_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();

        let dir_entries = self.base.filesystem.list(directory.to_str().unwrap()).await?;
        let pattern = format!("{}.*.codebook.json", base_name);

        let mut results = Vec::new();
        for entry in dir_entries {
            if !entry.name.contains(&pattern) {
                continue;
            }
            let path = PathBuf::from(&entry.name);
            if let Ok(Some(metadata)) = self.base.read_sidecar(&path).await {
                // Extract level from filename
                let level = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.split('.').nth(1))
                    .unwrap_or("unknown")
                    .to_string();

                results.push((level, metadata));
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_viper_sidecar_write_read() {
        let temp_dir = TempDir::new().unwrap();
        let config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let fs_factory = FilesystemFactory::create(config).await.unwrap();
        let filesystem = fs_factory.get_unified_caching_filesystem("file:///tmp", "test_collection".to_string(), "viper".to_string()).unwrap();

        let manager = ViperCodebookSidecarManager::new(
            "test_collection".to_string(),
            filesystem,
        );

        let parquet_path = temp_dir.path().join("test.parquet");
        let metadata = QuantizationCodebookMetadata {
            collection_id: "test_collection".to_string(),
            binary_codebook: None,
            int8_codebook: None,
            pq_codebooks: Default::default(),
            created_at: 1234567890,
            training_samples: 1000,
            schema_version: 1,
        };

        // Write sidecar
        manager.write_sidecar(&parquet_path, &metadata).await.unwrap();

        // Read back
        let read_metadata = manager.read_sidecar(&parquet_path).await.unwrap().unwrap();
        assert_eq!(read_metadata.collection_id, metadata.collection_id);
        assert_eq!(read_metadata.training_samples, metadata.training_samples);
    }

    #[test]
    fn test_sidecar_path_generation() {
        let parquet_path = Path::new("/data/collection/segment_001.parquet");
        let sidecar_path = ViperCodebookSidecarManager::sidecar_path(&parquet_path);
        assert_eq!(
            sidecar_path.file_name().unwrap().to_str().unwrap(),
            "segment_001.codebook.json"
        );
    }
}