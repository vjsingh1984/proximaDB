/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! AWS S3 (and S3-compatible: MinIO / LocalStack) [`FileSystem`] backend, built
//! on the official `aws-sdk-s3` crate. Feature-gated behind `aws`.
//!
//! Unlike the legacy `s3.rs` (which hardcoded AWS virtual-host URLs and a
//! "simplified" SigV4 signer), this delegates SigV4 signing, custom endpoints,
//! and path-style addressing to the SDK — so it works against MinIO/LocalStack as
//! well as real AWS. It exists to give the canonical `FileSystem` trait a *real*
//! S3 range backend for the ADR-023 cold-load path (`read_range` → S3 `Range:` GET).

use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;

use super::{
    DirEntry, FileOptions, FileSystem, FilesystemError, FilesystemFile, FsFileMetadata, FsResult,
};

/// Configuration for [`AwsS3FileSystem`]. `endpoint_url` + `force_path_style`
/// target S3-compatible stores (MinIO/LocalStack); leave `endpoint_url` `None`
/// for real AWS. Static credentials are used when both keys are set, else the
/// SDK's default credential chain.
#[derive(Debug, Clone)]
pub struct AwsS3Config {
    /// AWS region (any value for MinIO, e.g. `us-east-1`).
    pub region: String,
    /// Custom endpoint for S3-compatible stores (e.g. `http://localhost:9000`).
    pub endpoint_url: Option<String>,
    /// Path-style addressing (`{endpoint}/{bucket}/{key}`) — required for MinIO.
    pub force_path_style: bool,
    /// Static access key id (else default credential chain).
    pub access_key_id: Option<String>,
    /// Static secret access key.
    pub secret_access_key: Option<String>,
    /// Optional session token (temporary credentials).
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

/// S3 `FileSystem` backend over `aws-sdk-s3`.
pub struct AwsS3FileSystem {
    client: Client,
}

impl std::fmt::Debug for AwsS3FileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsS3FileSystem").finish()
    }
}

impl AwsS3FileSystem {
    /// Build a client from [`AwsS3Config`]. Async to match the other backends'
    /// constructors (no network call is made here).
    pub async fn new(cfg: AwsS3Config) -> FsResult<Self> {
        let mut builder = aws_sdk_s3::config::Builder::default()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(cfg.region))
            .force_path_style(cfg.force_path_style);
        if let Some(endpoint) = cfg.endpoint_url {
            builder = builder.endpoint_url(endpoint);
        }
        if let (Some(ak), Some(sk)) = (cfg.access_key_id, cfg.secret_access_key) {
            let creds = Credentials::new(ak, sk, cfg.session_token, None, "proximadb-static");
            builder = builder.credentials_provider(creds);
        }
        Ok(Self {
            client: Client::from_conf(builder.build()),
        })
    }

    /// Parse `s3://bucket/key` into `(bucket, key)`.
    fn parse(path: &str) -> FsResult<(String, String)> {
        let rest = path
            .strip_prefix("s3://")
            .or_else(|| path.strip_prefix("s3a://"))
            .ok_or_else(|| FilesystemError::InvalidPath(format!("not an s3:// path: {path}")))?;
        let (bucket, key) = rest
            .split_once('/')
            .ok_or_else(|| FilesystemError::InvalidPath(format!("missing key in s3 path: {path}")))?;
        if bucket.is_empty() || key.is_empty() {
            return Err(FilesystemError::InvalidPath(format!("bad s3 path: {path}")));
        }
        Ok((bucket.to_string(), key.to_string()))
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
        let out = self
            .client
            .get_object()
            .bucket(&bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| Self::net("S3 get_object", e))?;
        let body = out
            .body
            .collect()
            .await
            .map_err(|e| Self::net("S3 body", e))?;
        Ok(body.into_bytes().to_vec())
    }

    /// ADR-023 cold path: a true S3 `Range:` GET — fetches only `[offset, +length)`.
    async fn read_range(&self, path: &str, offset: u64, length: u64) -> FsResult<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }
        let (bucket, key) = Self::parse(path)?;
        let end = offset + length - 1; // HTTP byte ranges are inclusive
        let out = self
            .client
            .get_object()
            .bucket(&bucket)
            .key(&key)
            .range(format!("bytes={offset}-{end}"))
            .send()
            .await
            .map_err(|e| Self::net("S3 get_object range", e))?;
        let body = out
            .body
            .collect()
            .await
            .map_err(|e| Self::net("S3 body", e))?;
        Ok(body.into_bytes().to_vec())
    }

    async fn write(&self, path: &str, data: &[u8], _options: Option<FileOptions>) -> FsResult<()> {
        let (bucket, key) = Self::parse(path)?;
        self.client
            .put_object()
            .bucket(&bucket)
            .key(&key)
            .body(ByteStream::from(data.to_vec()))
            .send()
            .await
            .map_err(|e| Self::net("S3 put_object", e))?;
        Ok(())
    }

    async fn append(&self, _path: &str, _data: &[u8]) -> FsResult<()> {
        Err(FilesystemError::InvalidOperation(
            "S3 objects are immutable; append is unsupported".to_string(),
        ))
    }

    async fn delete(&self, path: &str) -> FsResult<()> {
        let (bucket, key) = Self::parse(path)?;
        self.client
            .delete_object()
            .bucket(&bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| Self::net("S3 delete_object", e))?;
        Ok(())
    }

    async fn exists(&self, path: &str) -> FsResult<bool> {
        let (bucket, key) = Self::parse(path)?;
        Ok(self
            .client
            .head_object()
            .bucket(&bucket)
            .key(&key)
            .send()
            .await
            .is_ok())
    }

    async fn metadata(&self, path: &str) -> FsResult<FsFileMetadata> {
        let (bucket, key) = Self::parse(path)?;
        let head = self
            .client
            .head_object()
            .bucket(&bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| Self::net("S3 head_object", e))?;
        Ok(FsFileMetadata {
            path: path.to_string(),
            size: head.content_length().unwrap_or(0).max(0) as u64,
            is_directory: false,
            etag: head.e_tag().map(|s| s.to_string()),
            storage_class: head.storage_class().map(|s| s.as_str().to_string()),
            ..Default::default()
        })
    }

    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        let (bucket, prefix) = Self::parse(path)?;
        let out = self
            .client
            .list_objects_v2()
            .bucket(&bucket)
            .prefix(&prefix)
            .send()
            .await
            .map_err(|e| Self::net("S3 list_objects_v2", e))?;
        let mut entries = Vec::new();
        for obj in out.contents() {
            let key = obj.key().unwrap_or_default().to_string();
            let name = key.rsplit('/').next().unwrap_or(&key).to_string();
            entries.push(DirEntry {
                name,
                url: format!("s3://{bucket}/{key}"),
                metadata: FsFileMetadata {
                    path: format!("s3://{bucket}/{key}"),
                    size: obj.size().unwrap_or(0).max(0) as u64,
                    is_directory: false,
                    etag: obj.e_tag().map(|s| s.to_string()),
                    ..Default::default()
                },
            });
        }
        Ok(entries)
    }

    /// S3 has no real directories; object keys carry the hierarchy. No-op.
    async fn create_dir(&self, _path: &str) -> FsResult<()> {
        Ok(())
    }

    async fn create_dir_all(&self, _path: &str) -> FsResult<()> {
        Ok(())
    }

    async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        let (from_bucket, from_key) = Self::parse(from)?;
        let (to_bucket, to_key) = Self::parse(to)?;
        self.client
            .copy_object()
            .bucket(&to_bucket)
            .key(&to_key)
            .copy_source(format!("{from_bucket}/{from_key}"))
            .send()
            .await
            .map_err(|e| Self::net("S3 copy_object", e))?;
        Ok(())
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
