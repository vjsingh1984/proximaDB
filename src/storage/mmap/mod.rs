use crate::core::{VectorRecord, VectorId, CollectionId, StorageError};
use crate::storage::{Result, lsm::LsmEntry};
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
    collection_id: CollectionId,
    data_dir: PathBuf,
    sst_files: Arc<RwLock<Vec<MmapSstFile>>>,
}

impl MmapReader {
    pub fn new(collection_id: CollectionId, data_dir: PathBuf) -> Result<Self> {
        Ok(Self {
            collection_id: collection_id.clone(),
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
                    let (_, lsm_entry): (VectorId, LsmEntry) = bincode::deserialize(data)
                        .map_err(|e| StorageError::Serialization(e.to_string()))?;
                    
                    // Handle different entry types
                    match lsm_entry {
                        LsmEntry::Record(record) => return Ok(Some(record)),
                        LsmEntry::Tombstone { .. } => return Ok(None), // Vector was deleted
                    }
                }
            }
        }
        
        Ok(None)
    }
    
    async fn load_sst_files(&self) -> Result<()> {
        let collection_dir = self.data_dir.join(&self.collection_id);
        
        if !collection_dir.exists() {
            return Ok(());
        }
        
        let mut entries = tokio::fs::read_dir(&collection_dir).await
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
        let file = std::fs::File::open(path)
            .map_err(StorageError::DiskIO)?;
        
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
            let entry_len = u32::from_le_bytes([
                len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]
            ]) as usize;
            
            if offset + 4 + entry_len > mmap.len() {
                break;
            }
            
            // Deserialize just to get the VectorId for building the index
            let entry_data = &mmap[offset + 4..offset + 4 + entry_len];
            match bincode::deserialize::<(VectorId, LsmEntry)>(entry_data) {
                Ok((id, _)) => {
                    index.insert(id, IndexEntry {
                        offset: (offset + 4) as u64,
                        length: entry_len as u32,
                    });
                },
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
}