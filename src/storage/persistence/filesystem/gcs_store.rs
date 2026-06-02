/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Google Cloud Storage [`FileSystem`] backend on the canonical `object_store`
//! crate. Feature-gated behind `gcp`.
//!
//! `object_store::get_range` is a TRUE byte-range GCS GET — the bandwidth-optimal
//! ADR-023 cold-load primitive. Real GCS uses Application Default Credentials;
//! `fake-gcs-server` is targeted via object_store's documented emulator hook — a
//! service-account key carrying `gcs_base_url` + `disable_oauth` (empty bearer),
//! which redirects the base URL without real signing. One canonical dependency
//! across S3 / Azure / GCS, dropping the `google-cloud-storage` SDK.

use async_trait::async_trait;
use dashmap::DashMap;
use futures::StreamExt;
use object_store::ClientOptions;
use object_store::ObjectStore;
use object_store::PutPayload;
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::path::Path as ObjPath;
use std::sync::Arc;

use super::{
    DirEntry, FileOptions, FileSystem, FilesystemError, FilesystemFile, FsFileMetadata, FsResult,
};

/// Configuration for [`GcsFileSystem`]. `endpoint_url` + `anonymous` target a
/// local `fake-gcs-server`; leave them unset for real GCS (default credentials).
#[derive(Debug, Clone, Default)]
pub struct GcsConfig {
    /// Custom storage endpoint (e.g. `http://localhost:4443` for fake-gcs-server).
    pub endpoint_url: Option<String>,
    /// Use anonymous (credential-less) access — required for fake-gcs-server.
    pub anonymous: bool,
    /// GCP project id (optional; not needed for fake-gcs).
    pub project_id: Option<String>,
}

/// GCS `FileSystem` backend over `object_store`. One `ObjectStore` is built (and
/// cached) per bucket.
pub struct GcsFileSystem {
    config: GcsConfig,
    stores: DashMap<String, Arc<dyn ObjectStore>>,
}

impl std::fmt::Debug for GcsFileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcsFileSystem").finish()
    }
}

impl GcsFileSystem {
    pub async fn new(cfg: GcsConfig) -> FsResult<Self> {
        Ok(Self {
            config: cfg,
            stores: DashMap::new(),
        })
    }

    /// Parse `gs://bucket/object` (or `gcs://`).
    fn parse(path: &str) -> FsResult<(String, String)> {
        let rest = path
            .strip_prefix("gs://")
            .or_else(|| path.strip_prefix("gcs://"))
            .ok_or_else(|| FilesystemError::InvalidPath(format!("not a gcs path: {path}")))?;
        let (bucket, object) = rest.split_once('/').ok_or_else(|| {
            FilesystemError::InvalidPath(format!("missing object in gcs path: {path}"))
        })?;
        if bucket.is_empty() || object.is_empty() {
            return Err(FilesystemError::InvalidPath(format!("bad gcs path: {path}")));
        }
        Ok((bucket.to_string(), object.to_string()))
    }

    /// Build (and cache) an `ObjectStore` for a bucket.
    fn store_for(&self, bucket: &str) -> FsResult<Arc<dyn ObjectStore>> {
        if let Some(s) = self.stores.get(bucket) {
            return Ok(s.clone());
        }
        let mut builder = GoogleCloudStorageBuilder::new().with_bucket_name(bucket);
        if let Some(ep) = &self.config.endpoint_url {
            // fake-gcs-server: redirect the GCS base URL and disable OAuth via a
            // minimal service-account key (object_store reads `gcs_base_url` /
            // `disable_oauth` from the SA key — see object_store gcp::credential).
            let key = format!(
                r#"{{"private_key":"","private_key_id":"","client_email":"","gcs_base_url":"{ep}","disable_oauth":true}}"#
            );
            builder = builder
                .with_service_account_key(key)
                .with_client_options(ClientOptions::default().with_allow_http(true));
        } else if self.config.anonymous {
            builder = builder.with_skip_signature(true);
        }
        let store: Arc<dyn ObjectStore> = Arc::new(
            builder
                .build()
                .map_err(|e| FilesystemError::Config(format!("GCS store build: {e}")))?,
        );
        self.stores.insert(bucket.to_string(), store.clone());
        Ok(store)
    }

    fn net(ctx: &str, e: impl std::fmt::Display) -> FilesystemError {
        FilesystemError::Network(format!("{ctx}: {e}"))
    }
}

#[async_trait]
impl FileSystem for GcsFileSystem {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        let (bucket, object) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        let result = store
            .get(&ObjPath::from(object))
            .await
            .map_err(|e| Self::net("GCS get", e))?;
        let bytes = result.bytes().await.map_err(|e| Self::net("GCS body", e))?;
        Ok(bytes.to_vec())
    }

    /// ADR-023 cold path: a TRUE ranged GCS GET — fetches only `[offset, +length)`.
    async fn read_range(&self, path: &str, offset: u64, length: u64) -> FsResult<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }
        let (bucket, object) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        let bytes = store
            .get_range(&ObjPath::from(object), offset..(offset + length))
            .await
            .map_err(|e| Self::net("GCS get_range", e))?;
        Ok(bytes.to_vec())
    }

    async fn write(&self, path: &str, data: &[u8], _options: Option<FileOptions>) -> FsResult<()> {
        let (bucket, object) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        store
            .put(&ObjPath::from(object), PutPayload::from(data.to_vec()))
            .await
            .map_err(|e| Self::net("GCS put", e))?;
        Ok(())
    }

    async fn append(&self, _path: &str, _data: &[u8]) -> FsResult<()> {
        Err(FilesystemError::InvalidOperation(
            "GCS objects do not support append".to_string(),
        ))
    }

    async fn delete(&self, path: &str) -> FsResult<()> {
        let (bucket, object) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        store
            .delete(&ObjPath::from(object))
            .await
            .map_err(|e| Self::net("GCS delete", e))?;
        Ok(())
    }

    async fn exists(&self, path: &str) -> FsResult<bool> {
        let (bucket, object) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        Ok(store.head(&ObjPath::from(object)).await.is_ok())
    }

    async fn metadata(&self, path: &str) -> FsResult<FsFileMetadata> {
        let (bucket, object) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        let meta = store
            .head(&ObjPath::from(object))
            .await
            .map_err(|e| Self::net("GCS head", e))?;
        Ok(FsFileMetadata {
            path: path.to_string(),
            size: meta.size,
            is_directory: false,
            etag: meta.e_tag,
            ..Default::default()
        })
    }

    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        let (bucket, prefix) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        let mut stream = store.list(Some(&ObjPath::from(prefix)));
        let mut entries = Vec::new();
        while let Some(meta) = stream.next().await {
            let meta = meta.map_err(|e| Self::net("GCS list", e))?;
            let key = meta.location.to_string();
            let name = key.rsplit('/').next().unwrap_or(&key).to_string();
            entries.push(DirEntry {
                name,
                url: format!("gs://{bucket}/{key}"),
                metadata: FsFileMetadata {
                    path: format!("gs://{bucket}/{key}"),
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
        "gcs"
    }

    async fn sync(&self) -> FsResult<()> {
        Ok(())
    }

    async fn open_file(&self, _path: &str, _create: bool) -> FsResult<Box<dyn FilesystemFile>> {
        Err(FilesystemError::InvalidOperation(
            "streaming open_file is not supported on the GCS backend".to_string(),
        ))
    }
}
