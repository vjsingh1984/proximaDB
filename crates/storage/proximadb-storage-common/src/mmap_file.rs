// Memory-mapped file support for ProximaDB
// Efficient handling of large SST and Parquet files

use anyhow::{Result, anyhow};
use memmap2::{Mmap, MmapMut, MmapOptions};
use std::fs::{File, OpenOptions};
use std::ops::{Deref, Range};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Read-only memory-mapped file
pub struct MmapFile {
    mmap: Arc<Mmap>,
    path: PathBuf,
    len: usize,
}

impl MmapFile {
    /// Open a file for memory-mapped reading
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(&path)?;
        let metadata = file.metadata()?;
        let len = metadata.len() as usize;

        let mmap = unsafe { MmapOptions::new().map(&file)? };

        Ok(Self {
            mmap: Arc::new(mmap),
            path: path.as_ref().to_path_buf(),
            len,
        })
    }

    /// Get the length of the file
    pub fn len(&self) -> usize {
        self.len
    }

    /// Check if file is empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Get a slice of the file
    pub fn slice(&self, range: Range<usize>) -> Result<&[u8]> {
        if range.end > self.len {
            return Err(anyhow!(
                "Range {}..{} exceeds file size {}",
                range.start,
                range.end,
                self.len
            ));
        }

        Ok(&self.mmap[range])
    }

    /// Read at a specific offset
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let available = self.len.saturating_sub(offset);
        let to_read = buf.len().min(available);

        if to_read == 0 {
            return Ok(0);
        }

        buf[..to_read].copy_from_slice(&self.mmap[offset..offset + to_read]);
        Ok(to_read)
    }

    /// Get a view of the file starting at an offset
    pub fn view_from(&self, offset: usize) -> Result<MmapView> {
        if offset > self.len {
            return Err(anyhow!("Offset {} exceeds file size {}", offset, self.len));
        }

        Ok(MmapView {
            mmap: Arc::clone(&self.mmap),
            offset,
            len: self.len - offset,
        })
    }

    /// Advise the kernel about access pattern
    pub fn advise(&self, advice: Advice) -> Result<()> {
        self.mmap.advise(advice.into())?;
        Ok(())
    }
}

/// View into a memory-mapped file
pub struct MmapView {
    mmap: Arc<Mmap>,
    offset: usize,
    len: usize,
}

impl MmapView {
    /// Get the length of this view
    pub fn len(&self) -> usize {
        self.len
    }

    /// Check if view is empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Get bytes from the view
    pub fn get(&self, range: Range<usize>) -> Result<&[u8]> {
        if range.end > self.len {
            return Err(anyhow!("Range exceeds view size"));
        }

        let start = self.offset + range.start;
        let end = self.offset + range.end;
        Ok(&self.mmap[start..end])
    }
}

impl Deref for MmapView {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.mmap[self.offset..self.offset + self.len]
    }
}

/// Writable memory-mapped file
pub struct MmapMutFile {
    mmap: MmapMut,
    file: File,
    path: PathBuf,
    len: usize,
}

impl MmapMutFile {
    /// Create or open a file for memory-mapped writing
    pub fn create<P: AsRef<Path>>(path: P, size: usize) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;

        file.set_len(size as u64)?;

        let mmap = unsafe { MmapOptions::new().len(size).map_mut(&file)? };

        Ok(Self {
            mmap,
            file,
            path: path.as_ref().to_path_buf(),
            len: size,
        })
    }

    /// Open an existing file for memory-mapped writing
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(&path)?;

        let metadata = file.metadata()?;
        let len = metadata.len() as usize;

        let mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        Ok(Self {
            mmap,
            file,
            path: path.as_ref().to_path_buf(),
            len,
        })
    }

    /// Write data at a specific offset
    pub fn write_at(&mut self, offset: usize, data: &[u8]) -> Result<usize> {
        let available = self.len.saturating_sub(offset);
        let to_write = data.len().min(available);

        if to_write == 0 {
            return Ok(0);
        }

        self.mmap[offset..offset + to_write].copy_from_slice(&data[..to_write]);
        Ok(to_write)
    }

    /// Flush changes to disk
    pub fn flush(&self) -> Result<()> {
        self.mmap.flush()?;
        Ok(())
    }

    /// Flush changes asynchronously
    pub fn flush_async(&self) -> Result<()> {
        self.mmap.flush_async()?;
        Ok(())
    }

    /// Resize the file
    pub fn resize(&mut self, new_size: usize) -> Result<()> {
        self.file.set_len(new_size as u64)?;

        // Need to remap
        let new_mmap = unsafe { MmapOptions::new().len(new_size).map_mut(&self.file)? };

        self.mmap = new_mmap;
        self.len = new_size;
        Ok(())
    }
}

/// Memory access advice
#[derive(Debug, Clone, Copy)]
pub enum Advice {
    Normal,
    Random,
    Sequential,
    WillNeed,
    DontNeed,
}

impl From<Advice> for memmap2::Advice {
    fn from(advice: Advice) -> Self {
        match advice {
            Advice::Normal => memmap2::Advice::Normal,
            Advice::Random => memmap2::Advice::Random,
            Advice::Sequential => memmap2::Advice::Sequential,
            Advice::WillNeed => memmap2::Advice::WillNeed,
            Advice::DontNeed => memmap2::Advice::WillNeed, // DontNeed not available in this version
        }
    }
}

/// Memory-mapped SST file reader
pub struct MmapSstReader {
    mmap: MmapFile,
    header_size: usize,
    index_offset: usize,
    data_offset: usize,
}

impl MmapSstReader {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mmap = MmapFile::open(path)?;

        // Read header to get offsets
        let header_size = 4096; // Fixed header size
        let header_bytes = mmap.slice(0..header_size)?;

        // Parse header to get offsets (simplified)
        let index_offset =
            u64::from_le_bytes(header_bytes[8..16].try_into().unwrap_or([0; 8])) as usize;

        let data_offset =
            u64::from_le_bytes(header_bytes[16..24].try_into().unwrap_or([0; 8])) as usize;

        Ok(Self {
            mmap,
            header_size,
            index_offset,
            data_offset,
        })
    }

    /// Read a data block directly from memory
    pub fn read_block(&self, block_offset: usize, block_size: usize) -> Result<&[u8]> {
        let start = self.data_offset + block_offset;
        let end = start + block_size;
        self.mmap.slice(start..end)
    }

    /// Get a view of the index section
    pub fn index_view(&self) -> Result<MmapView> {
        self.mmap.view_from(self.index_offset)
    }

    /// Advise sequential access for scanning
    pub fn advise_sequential(&self) -> Result<()> {
        self.mmap.advise(Advice::Sequential)
    }

    /// Advise random access for point lookups
    pub fn advise_random(&self) -> Result<()> {
        self.mmap.advise(Advice::Random)
    }
}

/// Memory-mapped Parquet file reader
pub struct MmapParquetReader {
    mmap: MmapFile,
    footer_size: usize,
    row_group_offsets: Vec<usize>,
}

impl MmapParquetReader {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mmap = MmapFile::open(path)?;

        // Read footer (last 8 bytes contain footer size)
        let file_len = mmap.len();
        let footer_len_bytes = mmap.slice(file_len - 8..file_len)?;
        let footer_size =
            u32::from_le_bytes(footer_len_bytes[0..4].try_into().unwrap_or([0; 4])) as usize;

        // Read footer
        let footer_start = file_len - 8 - footer_size;
        let _footer_bytes = mmap.slice(footer_start..file_len - 8)?;

        // Parse footer to get row group offsets (simplified)
        // In real implementation, would parse Parquet metadata
        let row_group_offsets = vec![0]; // Placeholder

        Ok(Self {
            mmap,
            footer_size,
            row_group_offsets,
        })
    }

    /// Read a row group directly from memory
    pub fn read_row_group(&self, index: usize) -> Result<&[u8]> {
        if index >= self.row_group_offsets.len() {
            return Err(anyhow!("Row group index {} out of bounds", index));
        }

        let start = self.row_group_offsets[index];
        let end = if index + 1 < self.row_group_offsets.len() {
            self.row_group_offsets[index + 1]
        } else {
            self.mmap.len() - self.footer_size - 8
        };

        self.mmap.slice(start..end)
    }

    /// Get a view of a specific column chunk
    pub fn column_chunk_view(&self, row_group: usize, column: usize) -> Result<MmapView> {
        // In real implementation, would calculate exact offset
        let offset = self.row_group_offsets[row_group] + column * 1024;
        self.mmap.view_from(offset)
    }
}

/// Pool of memory-mapped files
pub struct MmapPool {
    files:
        Arc<parking_lot::RwLock<proximadb_runtime_common::cache::LruCache<PathBuf, Arc<MmapFile>>>>,
    max_files: usize,
}

impl MmapPool {
    pub fn new(max_files: usize) -> Self {
        Self {
            files: Arc::new(parking_lot::RwLock::new(
                proximadb_runtime_common::cache::LruCache::new(max_files),
            )),
            max_files,
        }
    }

    /// Get or open a memory-mapped file
    pub fn get<P: AsRef<Path>>(&self, path: P) -> Result<Arc<MmapFile>> {
        let path = path.as_ref().to_path_buf();

        {
            let mut cache = self.files.write();
            if let Some(mmap) = cache.get(&path) {
                return Ok(Arc::clone(mmap));
            }
        }

        // Open the file
        let mmap = Arc::new(MmapFile::open(&path)?);

        {
            let mut cache = self.files.write();
            cache.put(path, Arc::clone(&mmap));
        }

        Ok(mmap)
    }

    /// Evict a file from the pool
    pub fn evict<P: AsRef<Path>>(&self, path: P) {
        let mut cache = self.files.write();
        cache.pop(&path.as_ref().to_path_buf());
    }

    /// Clear all cached files
    pub fn clear(&self) {
        let mut cache = self.files.write();
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_mmap_file_read() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("test.dat");

        // Create test file
        let data = b"Hello, memory-mapped world!";
        std::fs::write(&path, data)?;

        // Open with mmap
        let mmap = MmapFile::open(&path)?;
        assert_eq!(mmap.len(), data.len());

        // Read slice
        let slice = mmap.slice(0..5)?;
        assert_eq!(slice, b"Hello");

        // Read at offset
        let mut buf = vec![0u8; 5];
        let n = mmap.read_at(7, &mut buf)?;
        assert_eq!(n, 5);
        assert_eq!(&buf, b"memor");

        Ok(())
    }

    #[test]
    fn test_mmap_file_write() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("test_write.dat");

        // Create mmap file
        let mut mmap = MmapMutFile::create(&path, 1024)?;

        // Write data
        let data = b"Test data";
        let n = mmap.write_at(0, data)?;
        assert_eq!(n, data.len());

        // Flush and verify
        mmap.flush()?;

        let contents = std::fs::read(&path)?;
        assert_eq!(&contents[..data.len()], data);

        Ok(())
    }

    #[test]
    fn test_mmap_view() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("test_view.dat");

        let data = b"0123456789ABCDEF";
        std::fs::write(&path, data)?;

        let mmap = MmapFile::open(&path)?;
        let view = mmap.view_from(5)?;

        assert_eq!(view.len(), 11);
        assert_eq!(&view[0..3], b"567");

        Ok(())
    }

    #[test]
    fn test_mmap_pool() -> Result<()> {
        let dir = tempdir()?;
        let pool = MmapPool::new(2);

        // Create test files
        let path1 = dir.path().join("file1.dat");
        let path2 = dir.path().join("file2.dat");

        std::fs::write(&path1, b"File 1")?;
        std::fs::write(&path2, b"File 2")?;

        // Get files from pool
        let mmap1 = pool.get(key)?;
        let mmap2 = pool.get(key)?;

        assert_eq!(mmap1.len(), 6);
        assert_eq!(mmap2.len(), 6);

        // Get again - should return cached
        let mmap1_again = pool.get(key)?;
        assert!(Arc::ptr_eq(&mmap1, &mmap1_again));

        Ok(())
    }
}
