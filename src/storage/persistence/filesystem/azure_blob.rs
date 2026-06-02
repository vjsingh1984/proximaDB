/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Azure Blob Storage (ADLS Gen2) [`FileSystem`] backend on the canonical
//! `object_store` crate. Feature-gated behind `azure`.
//!
//! `object_store` is the battle-tested Rust object-store abstraction (Arrow /
//! DataFusion / Delta ecosystem). Unlike the pre-GA `azure_storage_blobs` 0.19
//! (whose ranged streaming GET hangs against Azurite), `object_store::get_range`
//! is a **true byte-range read** — the bandwidth-optimal ADR-023 cold-load
//! primitive — and `with_use_emulator(true)` targets Azurite as well as real
//! Azure / shared-key accounts.

use async_trait::async_trait;
use dashmap::DashMap;
use futures::StreamExt;
use object_store::ObjectStore;
use object_store::PutPayload;
use object_store::azure::MicrosoftAzureBuilder;
use object_store::path::Path as ObjPath;
use std::sync::Arc;

use super::{
    DirEntry, FileOptions, FileSystem, FilesystemError, FilesystemFile, FsFileMetadata, FsResult,
};

/// Configuration for [`AzureBlobFileSystem`]. `use_emulator` targets Azurite
/// (well-known `devstoreaccount1` creds); otherwise supply `account` +
/// `access_key` (and optionally a custom `endpoint`).
#[derive(Debug, Clone)]
pub struct AzureBlobConfig {
    /// Storage account name (ignored in emulator mode).
    pub account: String,
    /// Shared-key access key (not needed when `use_emulator`).
    pub access_key: Option<String>,
    /// Use the Azurite emulator (object_store fills in the well-known creds + host).
    pub use_emulator: bool,
    /// Optional custom blob endpoint (non-emulator).
    pub endpoint: Option<String>,
}

impl Default for AzureBlobConfig {
    fn default() -> Self {
        Self {
            account: "devstoreaccount1".to_string(),
            access_key: None,
            use_emulator: true,
            endpoint: None,
        }
    }
}

/// Azure Blob `FileSystem` backend over `object_store`. One `ObjectStore` is
/// built (and cached) per container.
pub struct AzureBlobFileSystem {
    config: AzureBlobConfig,
    stores: DashMap<String, Arc<dyn ObjectStore>>,
}

impl std::fmt::Debug for AzureBlobFileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureBlobFileSystem").finish()
    }
}

impl AzureBlobFileSystem {
    pub async fn new(cfg: AzureBlobConfig) -> FsResult<Self> {
        Ok(Self {
            config: cfg,
            stores: DashMap::new(),
        })
    }

    /// Parse `abfs://container/blob` (or `adls://`/`az://`/`azure://`).
    fn parse(path: &str) -> FsResult<(String, String)> {
        let rest = path
            .strip_prefix("abfs://")
            .or_else(|| path.strip_prefix("adls://"))
            .or_else(|| path.strip_prefix("az://"))
            .or_else(|| path.strip_prefix("azure://"))
            .ok_or_else(|| FilesystemError::InvalidPath(format!("not an azure path: {path}")))?;
        let (container, blob) = rest.split_once('/').ok_or_else(|| {
            FilesystemError::InvalidPath(format!("missing blob in azure path: {path}"))
        })?;
        if container.is_empty() || blob.is_empty() {
            return Err(FilesystemError::InvalidPath(format!("bad azure path: {path}")));
        }
        Ok((container.to_string(), blob.to_string()))
    }

    /// Build (and cache) an `ObjectStore` for a container.
    fn store_for(&self, container: &str) -> FsResult<Arc<dyn ObjectStore>> {
        if let Some(s) = self.stores.get(container) {
            return Ok(s.clone());
        }
        let mut builder = MicrosoftAzureBuilder::new()
            .with_container_name(container)
            .with_allow_http(true);
        if self.config.use_emulator {
            builder = builder.with_use_emulator(true);
        } else {
            builder = builder.with_account(self.config.account.clone());
            if let Some(key) = &self.config.access_key {
                builder = builder.with_access_key(key.clone());
            }
            if let Some(ep) = &self.config.endpoint {
                builder = builder.with_endpoint(ep.clone());
            }
        }
        let store: Arc<dyn ObjectStore> = Arc::new(
            builder
                .build()
                .map_err(|e| FilesystemError::Config(format!("Azure store build: {e}")))?,
        );
        self.stores.insert(container.to_string(), store.clone());
        Ok(store)
    }

    fn net(ctx: &str, e: impl std::fmt::Display) -> FilesystemError {
        FilesystemError::Network(format!("{ctx}: {e}"))
    }
}

#[async_trait]
impl FileSystem for AzureBlobFileSystem {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        let (container, blob) = Self::parse(path)?;
        let store = self.store_for(&container)?;
        let result = store
            .get(&ObjPath::from(blob))
            .await
            .map_err(|e| Self::net("Azure get", e))?;
        let bytes = result
            .bytes()
            .await
            .map_err(|e| Self::net("Azure body", e))?;
        Ok(bytes.to_vec())
    }

    /// ADR-023 cold path: a TRUE ranged blob GET — fetches only `[offset, +length)`.
    async fn read_range(&self, path: &str, offset: u64, length: u64) -> FsResult<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }
        let (container, blob) = Self::parse(path)?;
        let store = self.store_for(&container)?;
        let bytes = store
            .get_range(&ObjPath::from(blob), offset..(offset + length))
            .await
            .map_err(|e| Self::net("Azure get_range", e))?;
        Ok(bytes.to_vec())
    }

    async fn write(&self, path: &str, data: &[u8], _options: Option<FileOptions>) -> FsResult<()> {
        let (container, blob) = Self::parse(path)?;
        let store = self.store_for(&container)?;
        store
            .put(&ObjPath::from(blob), PutPayload::from(data.to_vec()))
            .await
            .map_err(|e| Self::net("Azure put", e))?;
        Ok(())
    }

    async fn append(&self, _path: &str, _data: &[u8]) -> FsResult<()> {
        Err(FilesystemError::InvalidOperation(
            "block blobs do not support append".to_string(),
        ))
    }

    async fn delete(&self, path: &str) -> FsResult<()> {
        let (container, blob) = Self::parse(path)?;
        let store = self.store_for(&container)?;
        store
            .delete(&ObjPath::from(blob))
            .await
            .map_err(|e| Self::net("Azure delete", e))?;
        Ok(())
    }

    async fn exists(&self, path: &str) -> FsResult<bool> {
        let (container, blob) = Self::parse(path)?;
        let store = self.store_for(&container)?;
        Ok(store.head(&ObjPath::from(blob)).await.is_ok())
    }

    async fn metadata(&self, path: &str) -> FsResult<FsFileMetadata> {
        let (container, blob) = Self::parse(path)?;
        let store = self.store_for(&container)?;
        let meta = store
            .head(&ObjPath::from(blob))
            .await
            .map_err(|e| Self::net("Azure head", e))?;
        Ok(FsFileMetadata {
            path: path.to_string(),
            size: meta.size,
            is_directory: false,
            etag: meta.e_tag,
            ..Default::default()
        })
    }

    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        let (container, prefix) = Self::parse(path)?;
        let store = self.store_for(&container)?;
        let mut stream = store.list(Some(&ObjPath::from(prefix)));
        let mut entries = Vec::new();
        while let Some(meta) = stream.next().await {
            let meta = meta.map_err(|e| Self::net("Azure list", e))?;
            let key = meta.location.to_string();
            let name = key.rsplit('/').next().unwrap_or(&key).to_string();
            entries.push(DirEntry {
                name,
                url: format!("az://{container}/{key}"),
                metadata: FsFileMetadata {
                    path: format!("az://{container}/{key}"),
                    size: meta.size,
                    is_directory: false,
                    etag: meta.e_tag,
                    ..Default::default()
                },
            });
        }
        Ok(entries)
    }

    async fn create_dir(&self, _path: &str) -> FsResult<()> {
        Ok(())
    }

    async fn create_dir_all(&self, _path: &str) -> FsResult<()> {
        Ok(())
    }

    async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        let data = self.read(from).await?;
        self.write(to, &data, None).await
    }

    async fn move_file(&self, from: &str, to: &str) -> FsResult<()> {
        self.copy(from, to).await?;
        self.delete(from).await
    }

    fn filesystem_type(&self) -> &'static str {
        "adls"
    }

    async fn sync(&self) -> FsResult<()> {
        Ok(())
    }

    async fn open_file(&self, _path: &str, _create: bool) -> FsResult<Box<dyn FilesystemFile>> {
        Err(FilesystemError::InvalidOperation(
            "streaming open_file is not supported on the Azure backend".to_string(),
        ))
    }
}
