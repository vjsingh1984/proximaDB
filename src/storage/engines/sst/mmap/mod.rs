use crate::core::{String, StorageError, VectorId, VectorRecord};
use crate::storage::{engines::sst::SstRecord, Result};
use memmap2::MmapOptions;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// SST file index entry
#[derive(Debug, Clone)]
struct IndexEntry {
    offset: u64,
    length: u32,
}

/// Memory-mapped SST file
#[derive(Debug)]
struct MmapSstFile {
    mmap: memmap2::Mmap,
    index: BTreeMap<VectorId, IndexEntry>,
}

#[derive(Debug)]
pub struct MmapReader {
    data_dir: PathBuf,
    sst_files: Arc<RwLock<Vec<MmapSstFile>>>,
}

impl MmapReader {
    pub fn new(_collection_id: String, data_dir: PathBuf) -> Result<Self> {
        Ok(Self {
            data_dir,
            sst_files: Arc::new(RwLock::new(Vec::new())),
        })
    }

    pub async fn initialize(&self) -> Result<()> {
        self.load_sst_files().await
    }

    pub async fn get(&self, id: &VectorId) -> Result<Option<VectorRecord>> {
        let sst_files = self.sst_files.read().await;

        // Search through SST files in reverse order (newest first)
        for sst_file in sst_files.iter().rev() {
            if let Some(entry) = sst_file.index.get(id) {
                // Read the record from the memory-mapped file
                let start = entry.offset as usize;
                let end = start + entry.length as usize;

                if end <= sst_file.mmap.len() {
                    let data = &sst_file.mmap[start..end];
                    let sst_record: SstRecord = SstRecord::deserialize(data)
                        .map_err(|e| StorageError::Serialization(e.to_string()))?;

                    // Handle different record types
                    if sst_record.is_tombstone {
                        return Ok(None); // Vector was deleted
                    } else {
                        return Ok(Some(sst_record.into())); // Convert SstRecord to VectorRecord
                    }
                }
            }
        }

        Ok(None)
    }

    async fn load_sst_files(&self) -> Result<()> {
        // data_dir already contains the collection-specific path
        let collection_dir = &self.data_dir;

        if !collection_dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&collection_dir)
            .await
            .map_err(StorageError::DiskIO)?;

        let mut sst_paths = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(StorageError::DiskIO)? {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("sst_") && name.ends_with(".db") {
                    sst_paths.push(path);
                }
            }
        }

        // Sort SST files by name (which includes timestamp)
        sst_paths.sort();

        let mut sst_files = Vec::new();
        for path in sst_paths {
            if let Ok(mmap_file) = self.load_sst_file(&path).await {
                sst_files.push(mmap_file);
            }
        }

        *self.sst_files.write().await = sst_files;
        Ok(())
    }

    async fn load_sst_file(&self, path: &PathBuf) -> Result<MmapSstFile> {
        let file = std::fs::File::open(path).map_err(StorageError::DiskIO)?;

        let mmap = unsafe {
            MmapOptions::new()
                .map(&file)
                .map_err(StorageError::DiskIO)?
        };

        // Build index by scanning the file
        let mut index = BTreeMap::new();
        let mut offset = 0;

        while offset + 4 <= mmap.len() {
            // Read entry length
            let len_bytes = &mmap[offset..offset + 4];
            let entry_len =
                u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]])
                    as usize;

            if offset + 4 + entry_len > mmap.len() {
                break;
            }

            // Deserialize just to get the VectorId for building the index
            let entry_data = &mmap[offset + 4..offset + 4 + entry_len];
            match SstRecord::deserialize(entry_data) {
                Ok(record) => {
                    let id = VectorId::from(record.id);
                    index.insert(
                        id,
                        IndexEntry {
                            offset: (offset + 4) as u64,
                            length: entry_len as u32,
                        },
                    );
                }
                Err(_) => {
                    // Skip corrupted entries
                }
            }

            offset += 4 + entry_len;
        }

        Ok(MmapSstFile { mmap, index })
    }

    pub async fn refresh(&self) -> Result<()> {
        self.load_sst_files().await
    }

    /// Iterate over all vector records in the SST files
    /// Returns only active records (filters out tombstones)
    pub async fn iter_all(&self) -> Result<Vec<VectorRecord>> {
        let sst_files = self.sst_files.read().await;
        let mut records = Vec::new();

        // Iterate through all SST files
        for sst_file in sst_files.iter() {
            // Iterate through all index entries in this SST file
            for (vector_id, index_entry) in &sst_file.index {
                let start = index_entry.offset as usize;
                let end = start + index_entry.length as usize;

                if end <= sst_file.mmap.len() {
                    let data = &sst_file.mmap[start..end];

                    // Deserialize the record
                    match SstRecord::deserialize(data) {
                        Ok(sst_record) => {
                            if !sst_record.is_tombstone {
                                // Convert SstRecord to VectorRecord and add to results
                                records.push(sst_record.into());
                            }
                            // Skip deleted records (tombstones)
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to deserialize entry for vector {}: {}",
                                vector_id,
                                e
                            );
                        }
                    }
                }
            }
        }

        tracing::debug!(
            "MmapReader::iter_all found {} active records across {} SST files",
            records.len(),
            sst_files.len()
        );
        Ok(records)
    }
}
