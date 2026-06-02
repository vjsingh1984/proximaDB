/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Azure Blob Storage (ADLS Gen2) [`FileSystem`] backend on the official
//! `azure_storage_blobs` crate. Feature-gated behind `azure`.
//!
//! The SDK handles SAS/shared-key signing and the emulator location, so this
//! works against Azurite as well as real Azure. `read_range` issues a ranged
//! blob GET — the ADR-023 cold-load primitive.

use async_trait::async_trait;
use azure_core::request_options::Range;
use azure_storage::StorageCredentials;
use azure_storage_blobs::prelude::{BlobServiceClient, ClientBuilder};
use futures::StreamExt;

use super::{
    DirEntry, FileOptions, FileSystem, FilesystemError, FilesystemFile, FsFileMetadata, FsResult,
};

/// Configuration for [`AzureBlobFileSystem`]. `use_emulator` targets Azurite
/// (default account `devstoreaccount1`); otherwise supply `account` + `access_key`.
#[derive(Debug, Clone)]
pub struct AzureBlobConfig {
    /// Storage account name (Azurite default is `devstoreaccount1`).
    pub account: String,
    /// Shared-key access key (not needed when `use_emulator`).
    pub access_key: Option<String>,
    /// Use the Azurite emulator location (127.0.0.1:10000, well-known creds).
    pub use_emulator: bool,
}

impl Default for AzureBlobConfig {
    fn default() -> Self {
        Self {
            account: "devstoreaccount1".to_string(),
            access_key: None,
            use_emulator: true,
        }
    }
}

/// Azure Blob `FileSystem` backend over `azure_storage_blobs`.
pub struct AzureBlobFileSystem {
    service: BlobServiceClient,
}

impl std::fmt::Debug for AzureBlobFileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureBlobFileSystem").finish()
    }
}

impl AzureBlobFileSystem {
    pub async fn new(cfg: AzureBlobConfig) -> FsResult<Self> {
        let service = if cfg.use_emulator {
            ClientBuilder::emulator().blob_service_client()
        } else {
            let key = cfg.access_key.clone().ok_or_else(|| {
                FilesystemError::Config("azure access_key required (or use_emulator)".to_string())
            })?;
            ClientBuilder::new(
                cfg.account.clone(),
                StorageCredentials::access_key(cfg.account.clone(), key),
            )
            .blob_service_client()
        };
        Ok(Self { service })
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
        let client = self.service.container_client(container).blob_client(blob);
        client
            .get_content()
            .await
            .map_err(|e| Self::net("Azure get_content", e))
    }

    /// ADR-023 cold path: a ranged blob GET — fetches only `[offset, +length)`.
    async fn read_range(&self, path: &str, offset: u64, length: u64) -> FsResult<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }
        let (container, blob) = Self::parse(path)?;
        let client = self.service.container_client(container).blob_client(blob);
        // azure_core::Range is [start, end) (end exclusive).
        let mut stream = client.get().range(Range::new(offset, offset + length)).into_stream();
        let mut out = Vec::with_capacity(length as usize);
        while let Some(value) = stream.next().await {
            let response = value.map_err(|e| Self::net("Azure get range", e))?;
            let body = response
                .data
                .collect()
                .await
                .map_err(|e| Self::net("Azure body", e))?;
            out.extend_from_slice(&body);
        }
        Ok(out)
    }

    async fn write(&self, path: &str, data: &[u8], _options: Option<FileOptions>) -> FsResult<()> {
        let (container, blob) = Self::parse(path)?;
        let client = self.service.container_client(container).blob_client(blob);
        client
            .put_block_blob(data.to_vec())
            .await
            .map_err(|e| Self::net("Azure put_block_blob", e))?;
        Ok(())
    }

    async fn append(&self, _path: &str, _data: &[u8]) -> FsResult<()> {
        Err(FilesystemError::InvalidOperation(
            "block blobs do not support append; use append blobs".to_string(),
        ))
    }

    async fn delete(&self, path: &str) -> FsResult<()> {
        let (container, blob) = Self::parse(path)?;
        let client = self.service.container_client(container).blob_client(blob);
        client
            .delete()
            .await
            .map_err(|e| Self::net("Azure delete", e))?;
        Ok(())
    }

    async fn exists(&self, path: &str) -> FsResult<bool> {
        let (container, blob) = Self::parse(path)?;
        let client = self.service.container_client(container).blob_client(blob);
        client
            .exists()
            .await
            .map_err(|e| Self::net("Azure exists", e))
    }

    async fn metadata(&self, path: &str) -> FsResult<FsFileMetadata> {
        let (container, blob) = Self::parse(path)?;
        let client = self.service.container_client(container).blob_client(blob);
        let props = client
            .get_properties()
            .await
            .map_err(|e| Self::net("Azure get_properties", e))?;
        Ok(FsFileMetadata {
            path: path.to_string(),
            size: props.blob.properties.content_length,
            is_directory: false,
            ..Default::default()
        })
    }

    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        let (container, prefix) = Self::parse(path)?;
        let cc = self.service.container_client(container.clone());
        let mut stream = cc.list_blobs().prefix(prefix).into_stream();
        let mut entries = Vec::new();
        while let Some(value) = stream.next().await {
            let response = value.map_err(|e| Self::net("Azure list_blobs", e))?;
            for blob in response.blobs.blobs() {
                entries.push(DirEntry {
                    name: blob.name.clone(),
                    url: format!("az://{container}/{}", blob.name),
                    metadata: FsFileMetadata {
                        path: format!("az://{container}/{}", blob.name),
                        size: blob.properties.content_length,
                        is_directory: false,
                        ..Default::default()
                    },
                });
            }
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
