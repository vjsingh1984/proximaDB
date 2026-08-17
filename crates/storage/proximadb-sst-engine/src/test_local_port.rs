//! tokio::fs-backed `FilesystemPort` for sst-engine tests (they write real
//! local files; the port keeps the production seam intact). Same shape as
//! engine-core's `parquet_write_engine::test_local_port` (TD-DECOMP-78).

#[cfg(test)]
pub(crate) mod local_port {
    use async_trait::async_trait;
    use proximadb_storage_filesystem_types::{
        DirEntry, FileOptions, FileSystem, FilesystemError, FsResult,
    };
    use std::sync::Arc;

    /// A `FilesystemPort` over the local filesystem via tokio::fs.
    pub(crate) fn local_port() -> Arc<dyn proximadb_storage_ports::FilesystemPort> {
        Arc::new(TokioLocalPort)
    }

    struct TokioLocalPort;

    fn io_err(e: std::io::Error) -> FilesystemError {
        FilesystemError::Io(e)
    }

    fn strip(p: &str) -> &str {
        p.trim_start_matches("file://")
    }

    #[async_trait]
    impl proximadb_storage_ports::FilesystemPort for TokioLocalPort {
        fn get_filesystem(&self, _url: &str) -> FsResult<Arc<dyn FileSystem>> {
            Ok(Arc::new(LocalFs))
        }
        async fn create_dir_all(&self, url: &str) -> FsResult<()> {
            tokio::fs::create_dir_all(strip(url)).await.map_err(io_err)
        }
        async fn write(
            &self,
            url: &str,
            data: &[u8],
            _options: Option<FileOptions>,
        ) -> FsResult<()> {
            if let Some(parent) = std::path::Path::new(strip(url)).parent() {
                tokio::fs::create_dir_all(parent).await.map_err(io_err)?;
            }
            tokio::fs::write(strip(url), data).await.map_err(io_err)
        }
        async fn move_atomic(&self, from_url: &str, to_url: &str) -> FsResult<()> {
            tokio::fs::rename(strip(from_url), strip(to_url))
                .await
                .map_err(io_err)
        }
        async fn delete(&self, url: &str) -> FsResult<()> {
            tokio::fs::remove_file(strip(url)).await.map_err(io_err)
        }
        async fn read(&self, url: &str) -> FsResult<Vec<u8>> {
            tokio::fs::read(strip(url)).await.map_err(io_err)
        }
        async fn list(&self, _url: &str) -> FsResult<Vec<DirEntry>> {
            Ok(Vec::new())
        }
    }

    /// Minimal local `FileSystem` backed by tokio::fs — enough for the
    /// deletion-vector store's write/read/move paths in tests.
    #[derive(Debug)]
    struct LocalFs;

    #[async_trait]
    impl FileSystem for LocalFs {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
            tokio::fs::read(strip(path)).await.map_err(io_err)
        }
        async fn write(
            &self,
            path: &str,
            data: &[u8],
            _options: Option<FileOptions>,
        ) -> FsResult<()> {
            if let Some(parent) = std::path::Path::new(strip(path)).parent() {
                tokio::fs::create_dir_all(parent).await.map_err(io_err)?;
            }
            tokio::fs::write(strip(path), data).await.map_err(io_err)
        }
        async fn delete(&self, path: &str) -> FsResult<()> {
            tokio::fs::remove_file(strip(path)).await.map_err(io_err)
        }
        async fn exists(&self, path: &str) -> FsResult<bool> {
            tokio::fs::try_exists(strip(path)).await.map_err(io_err)
        }
        async fn metadata(
            &self,
            path: &str,
        ) -> FsResult<proximadb_storage_filesystem_types::FsFileMetadata> {
            let m = tokio::fs::metadata(strip(path)).await.map_err(io_err)?;
            Ok(proximadb_storage_filesystem_types::FsFileMetadata {
                path: path.to_string(),
                size: m.len(),
                created: None,
                modified: m.modified().ok().map(chrono::DateTime::from),
                is_directory: m.is_dir(),
                permissions: None,
                etag: None,
                storage_class: None,
            })
        }
        async fn list(&self, _path: &str) -> FsResult<Vec<DirEntry>> {
            Ok(Vec::new())
        }
        async fn create_dir(&self, path: &str) -> FsResult<()> {
            tokio::fs::create_dir(strip(path)).await.map_err(io_err)
        }
        async fn create_dir_all(&self, path: &str) -> FsResult<()> {
            tokio::fs::create_dir_all(strip(path)).await.map_err(io_err)
        }
        async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
            tokio::fs::copy(strip(from), strip(to))
                .await
                .map_err(io_err)
                .map(|_| ())
        }
        async fn move_file(&self, from: &str, to: &str) -> FsResult<()> {
            tokio::fs::rename(strip(from), strip(to))
                .await
                .map_err(io_err)
        }
        async fn append(&self, path: &str, data: &[u8]) -> FsResult<()> {
            use tokio::io::AsyncWriteExt;
            let mut f = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(strip(path))
                .await
                .map_err(io_err)?;
            f.write_all(data).await.map_err(io_err)
        }
        async fn sync_data(&self, path: &str) -> FsResult<()> {
            let f = tokio::fs::File::open(strip(path)).await.map_err(io_err)?;
            f.sync_data().await.map_err(io_err)
        }
        async fn write_if_absent(
            &self,
            path: &str,
            data: &[u8],
            options: Option<FileOptions>,
        ) -> FsResult<()> {
            if self.exists(path).await? {
                return Err(FilesystemError::AlreadyExists(path.to_string()));
            }
            self.write(path, data, options).await
        }
        fn filesystem_type(&self) -> &'static str {
            "local-test"
        }
        async fn sync(&self) -> FsResult<()> {
            Ok(())
        }
        async fn open_file(
            &self,
            _path: &str,
            _create: bool,
        ) -> FsResult<Box<dyn proximadb_storage_filesystem_types::FilesystemFile>> {
            Err(FilesystemError::UnsupportedScheme(
                "test stub: open_file".to_string(),
            ))
        }
    }
}
