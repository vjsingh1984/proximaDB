use crate::storage::{Result, StorageError};
use std::path::PathBuf;

#[derive(Debug)]
pub struct DiskManager {
    data_dirs: Vec<PathBuf>,
    current_write_dir: usize,
}

impl DiskManager {
    pub fn new(data_dirs: Vec<PathBuf>) -> Result<Self> {
        // Create directories if they don't exist
        for dir in &data_dirs {
            if !dir.exists() {
                std::fs::create_dir_all(dir).map_err(|e| StorageError::DiskIO(e))?;
            }
        }

        Ok(Self {
            data_dirs,
            current_write_dir: 0,
        })
    }

    pub fn get_write_path(&mut self) -> &PathBuf {
        let path = &self.data_dirs[self.current_write_dir];
        // TODO: Implement round-robin or space-based selection
        self.current_write_dir = (self.current_write_dir + 1) % self.data_dirs.len();
        path
    }

    pub fn get_read_paths(&self) -> &[PathBuf] {
        &self.data_dirs
    }
}
