//! In-memory `FileSystem` test double shared by catalog backend tests
//! (TD-CAT-1 native, TD-OBJSTORE-1 delta): an object store keyed by full
//! URL. Only the methods the catalog exercises via the injected path are
//! implemented (read/write/exists/list/delete/create_dir_all); `list`
//! returns DIRECT children only, matching the native catalog's usage.

use async_trait::async_trait;
use proximadb_storage_filesystem_types::{FileOptions, FileSystem, FilesystemError};
use std::collections::HashMap;

// ── Injected object-store backend (TD-CAT-1) ────────────────────
//
// An in-memory `FileSystem` standing in for an object store (keyed by
// full URL). Only the methods the catalog actually exercises via the
// injected path are implemented (read/write/exists/list/delete/
// create_dir_all); the rest are unused here.
#[derive(Debug, Default)]
pub(crate) struct MemFs {
    files: std::sync::Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait]
impl FileSystem for MemFs {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn filesystem_type(&self) -> &'static str {
        "memfs"
    }
    async fn read(&self, path: &str) -> proximadb_storage_filesystem_types::FsResult<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| FilesystemError::NotFound(path.to_string()))
    }
    async fn write(
        &self,
        path: &str,
        data: &[u8],
        _options: Option<FileOptions>,
    ) -> proximadb_storage_filesystem_types::FsResult<()> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string(), data.to_vec());
        Ok(())
    }
    async fn delete(&self, path: &str) -> proximadb_storage_filesystem_types::FsResult<()> {
        self.files.lock().unwrap().remove(path);
        Ok(())
    }
    async fn exists(&self, path: &str) -> proximadb_storage_filesystem_types::FsResult<bool> {
        Ok(self.files.lock().unwrap().contains_key(path))
    }
    async fn create_dir_all(
        &self,
        _path: &str,
    ) -> proximadb_storage_filesystem_types::FsResult<()> {
        Ok(()) // object stores have no directories
    }
    async fn list(
        &self,
        path: &str,
    ) -> proximadb_storage_filesystem_types::FsResult<
        Vec<proximadb_storage_filesystem_types::DirEntry>,
    > {
        let prefix = path.trim_end_matches('/');
        let mut entries = Vec::new();
        for key in self.files.lock().unwrap().keys() {
            if let Some(rest) = key.strip_prefix(prefix) {
                let rest = rest.trim_start_matches('/');
                if !rest.is_empty() && !rest.contains('/') {
                    entries.push(proximadb_storage_filesystem_types::DirEntry {
                        name: rest.to_string(),
                        url: key.clone(),
                        metadata: proximadb_storage_filesystem_types::FsFileMetadata::default(),
                    });
                }
            }
        }
        Ok(entries)
    }
    // Unused by the catalog's injected path.
    async fn append(
        &self,
        _p: &str,
        _d: &[u8],
    ) -> proximadb_storage_filesystem_types::FsResult<()> {
        unimplemented!()
    }
    async fn metadata(
        &self,
        _p: &str,
    ) -> proximadb_storage_filesystem_types::FsResult<
        proximadb_storage_filesystem_types::FsFileMetadata,
    > {
        unimplemented!()
    }
    async fn create_dir(&self, _p: &str) -> proximadb_storage_filesystem_types::FsResult<()> {
        unimplemented!()
    }
    async fn copy(&self, _f: &str, _t: &str) -> proximadb_storage_filesystem_types::FsResult<()> {
        unimplemented!()
    }
    async fn move_file(
        &self,
        _f: &str,
        _t: &str,
    ) -> proximadb_storage_filesystem_types::FsResult<()> {
        unimplemented!()
    }
    async fn open_file(
        &self,
        _p: &str,
        _c: bool,
    ) -> proximadb_storage_filesystem_types::FsResult<
        Box<dyn proximadb_storage_filesystem_types::FilesystemFile>,
    > {
        unimplemented!()
    }
    async fn sync(&self) -> proximadb_storage_filesystem_types::FsResult<()> {
        Ok(())
    }
}
