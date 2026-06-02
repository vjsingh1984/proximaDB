/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! AWS S3 (and S3-compatible: MinIO / LocalStack / R2) [`FileSystem`] backend on
//! the canonical `object_store` crate. Feature-gated behind `aws`.
//!
//! `object_store::get_range` is a TRUE byte-range S3 GET — the bandwidth-optimal
//! ADR-023 cold-load primitive. Using `object_store` here (rather than the full
//! `aws-sdk-s3` stack) keeps the cloud-FS layer on one canonical dependency
//! across S3 / Azure / GCS and drops a large transitive SDK tree.

use async_trait::async_trait;
use dashmap::DashMap;
use futures::StreamExt;
use object_store::ObjectStore;
use object_store::PutPayload;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjPath;
use std::sync::Arc;

use super::{
    DirEntry, FileOptions, FileSystem, FilesystemError, FilesystemFile, FsFileMetadata, FsResult,
};

/// Configuration for [`AwsS3FileSystem`]. For MinIO/LocalStack set
/// `endpoint_url` + `force_path_style = true` + static creds.
#[derive(Debug, Clone)]
pub struct AwsS3Config {
    /// AWS region (e.g. `us-east-1`). Required by SigV4 even for MinIO.
    pub region: String,
    /// Custom S3 endpoint (MinIO/LocalStack). `None` = real AWS.
    pub endpoint_url: Option<String>,
    /// Path-style addressing (`http://host/bucket/key`) — required by MinIO.
    pub force_path_style: bool,
    /// Static access key id (else env/instance credentials).
    pub access_key_id: Option<String>,
    /// Static secret access key.
    pub secret_access_key: Option<String>,
    /// Optional session token (STS).
    pub session_token: Option<String>,
}

impl Default for AwsS3Config {
    fn default() -> Self {
        Self {
            region: "us-east-1".to_string(),
            endpoint_url: None,
            force_path_style: false,
            access_key_id: None,
            secret_access_key: None,
            session_token: None,
        }
    }
}

/// AWS S3 `FileSystem` backend over `object_store`. One `ObjectStore` is built
/// (and cached) per bucket.
pub struct AwsS3FileSystem {
    config: AwsS3Config,
    stores: DashMap<String, Arc<dyn ObjectStore>>,
}

impl std::fmt::Debug for AwsS3FileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsS3FileSystem").finish()
    }
}

impl AwsS3FileSystem {
    pub async fn new(cfg: AwsS3Config) -> FsResult<Self> {
        Ok(Self {
            config: cfg,
            stores: DashMap::new(),
        })
    }

    /// Parse `s3://bucket/key` (or `s3a://`).
    fn parse(path: &str) -> FsResult<(String, String)> {
        let rest = path
            .strip_prefix("s3://")
            .or_else(|| path.strip_prefix("s3a://"))
            .ok_or_else(|| FilesystemError::InvalidPath(format!("not an s3 path: {path}")))?;
        let (bucket, key) = rest
            .split_once('/')
            .ok_or_else(|| FilesystemError::InvalidPath(format!("missing key in s3 path: {path}")))?;
        if bucket.is_empty() || key.is_empty() {
            return Err(FilesystemError::InvalidPath(format!("bad s3 path: {path}")));
        }
        Ok((bucket.to_string(), key.to_string()))
    }

    /// Build (and cache) an `ObjectStore` for a bucket.
    fn store_for(&self, bucket: &str) -> FsResult<Arc<dyn ObjectStore>> {
        if let Some(s) = self.stores.get(bucket) {
            return Ok(s.clone());
        }
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(bucket)
            .with_region(self.config.region.clone());
        if let Some(ep) = &self.config.endpoint_url {
            builder = builder.with_endpoint(ep.clone()).with_allow_http(true);
        }
        if self.config.force_path_style {
            // Path-style addressing: http://host:port/bucket/key (MinIO).
            builder = builder.with_virtual_hosted_style_request(false);
        }
        if let Some(ak) = &self.config.access_key_id {
            builder = builder.with_access_key_id(ak.clone());
        }
        if let Some(sk) = &self.config.secret_access_key {
            builder = builder.with_secret_access_key(sk.clone());
        }
        if let Some(tok) = &self.config.session_token {
            builder = builder.with_token(tok.clone());
        }
        let store: Arc<dyn ObjectStore> = Arc::new(
            builder
                .build()
                .map_err(|e| FilesystemError::Config(format!("S3 store build: {e}")))?,
        );
        self.stores.insert(bucket.to_string(), store.clone());
        Ok(store)
    }

    fn net(ctx: &str, e: impl std::fmt::Display) -> FilesystemError {
        FilesystemError::Network(format!("{ctx}: {e}"))
    }
}

#[async_trait]
impl FileSystem for AwsS3FileSystem {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        let (bucket, key) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        let result = store
            .get(&ObjPath::from(key))
            .await
            .map_err(|e| Self::net("S3 get", e))?;
        let bytes = result.bytes().await.map_err(|e| Self::net("S3 body", e))?;
        Ok(bytes.to_vec())
    }

    /// ADR-023 cold path: a TRUE ranged S3 GET — fetches only `[offset, +length)`.
    async fn read_range(&self, path: &str, offset: u64, length: u64) -> FsResult<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }
        let (bucket, key) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        let bytes = store
            .get_range(&ObjPath::from(key), offset..(offset + length))
            .await
            .map_err(|e| Self::net("S3 get_range", e))?;
        Ok(bytes.to_vec())
    }

    async fn write(&self, path: &str, data: &[u8], _options: Option<FileOptions>) -> FsResult<()> {
        let (bucket, key) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        store
            .put(&ObjPath::from(key), PutPayload::from(data.to_vec()))
            .await
            .map_err(|e| Self::net("S3 put", e))?;
        Ok(())
    }

    async fn append(&self, _path: &str, _data: &[u8]) -> FsResult<()> {
        Err(FilesystemError::InvalidOperation(
            "S3 objects do not support append".to_string(),
        ))
    }

    async fn delete(&self, path: &str) -> FsResult<()> {
        let (bucket, key) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        store
            .delete(&ObjPath::from(key))
            .await
            .map_err(|e| Self::net("S3 delete", e))?;
        Ok(())
    }

    async fn exists(&self, path: &str) -> FsResult<bool> {
        let (bucket, key) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        Ok(store.head(&ObjPath::from(key)).await.is_ok())
    }

    async fn metadata(&self, path: &str) -> FsResult<FsFileMetadata> {
        let (bucket, key) = Self::parse(path)?;
        let store = self.store_for(&bucket)?;
        let meta = store
            .head(&ObjPath::from(key))
            .await
            .map_err(|e| Self::net("S3 head", e))?;
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
            let meta = meta.map_err(|e| Self::net("S3 list", e))?;
            let key = meta.location.to_string();
            let name = key.rsplit('/').next().unwrap_or(&key).to_string();
            entries.push(DirEntry {
                name,
                url: format!("s3://{bucket}/{key}"),
                metadata: FsFileMetadata {
                    path: format!("s3://{bucket}/{key}"),
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
        "s3"
    }

    async fn sync(&self) -> FsResult<()> {
        Ok(())
    }

    async fn open_file(&self, _path: &str, _create: bool) -> FsResult<Box<dyn FilesystemFile>> {
        Err(FilesystemError::InvalidOperation(
            "streaming open_file is not supported on the S3 backend".to_string(),
        ))
    }
}
