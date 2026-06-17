/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Google Cloud Storage [`FileSystem`] backend on the official
//! `google-cloud-storage` crate. Feature-gated behind `gcp`.
//!
//! The SDK handles OAuth/signing and a custom `storage_endpoint`, so this works
//! against `fake-gcs-server` (anonymous + custom endpoint) as well as real GCS.
//! `read_range` issues a ranged object download — the ADR-023 cold-load primitive.

use async_trait::async_trait;
use google_cloud_storage::client::{Client, ClientConfig};
use google_cloud_storage::http::objects::delete::DeleteObjectRequest;
use google_cloud_storage::http::objects::download::Range;
use google_cloud_storage::http::objects::get::GetObjectRequest;
use google_cloud_storage::http::objects::list::ListObjectsRequest;
use google_cloud_storage::http::objects::upload::{Media, UploadObjectRequest, UploadType};

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

/// GCS `FileSystem` backend over `google-cloud-storage`.
pub struct GcsFileSystem {
    client: Client,
}

impl std::fmt::Debug for GcsFileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcsFileSystem").finish()
    }
}

impl GcsFileSystem {
    pub async fn new(cfg: GcsConfig) -> FsResult<Self> {
        let mut config = if cfg.anonymous {
            ClientConfig::default().anonymous()
        } else {
            ClientConfig::default()
                .with_auth()
                .await
                .map_err(|e| FilesystemError::Auth(format!("GCS auth: {e}")))?
        };
        if let Some(endpoint) = cfg.endpoint_url {
            config.storage_endpoint = endpoint;
        }
        if cfg.project_id.is_some() {
            config.project_id = cfg.project_id;
        }
        Ok(Self {
            client: Client::new(config),
        })
    }

    /// Parse `gs://bucket/object` (or `gcs://`) into `(bucket, object)`.
    fn parse(path: &str) -> FsResult<(String, String)> {
        let rest = path
            .strip_prefix("gs://")
            .or_else(|| path.strip_prefix("gcs://"))
            .ok_or_else(|| FilesystemError::InvalidPath(format!("not a gs:// path: {path}")))?;
        let (bucket, object) = rest.split_once('/').ok_or_else(|| {
            FilesystemError::InvalidPath(format!("missing object in gs path: {path}"))
        })?;
        if bucket.is_empty() || object.is_empty() {
            return Err(FilesystemError::InvalidPath(format!("bad gs path: {path}")));
        }
        Ok((bucket.to_string(), object.to_string()))
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
        self.client
            .download_object(
                &GetObjectRequest {
                    bucket,
                    object,
                    ..Default::default()
                },
                &Range::default(),
            )
            .await
            .map_err(|e| Self::net("GCS download_object", e))
    }

    /// ADR-023 cold path: a ranged object download — fetches only `[offset, +length)`.
    async fn read_range(&self, path: &str, offset: u64, length: u64) -> FsResult<Vec<u8>> {
        if length == 0 {
            return Ok(Vec::new());
        }
        let (bucket, object) = Self::parse(path)?;
        self.client
            .download_object(
                &GetObjectRequest {
                    bucket,
                    object,
                    ..Default::default()
                },
                &Range(Some(offset), Some(offset + length - 1)),
            )
            .await
            .map_err(|e| Self::net("GCS download_object range", e))
    }

    async fn write(&self, path: &str, data: &[u8], _options: Option<FileOptions>) -> FsResult<()> {
        let (bucket, object) = Self::parse(path)?;
        let upload_type = UploadType::Simple(Media::new(object));
        self.client
            .upload_object(
                &UploadObjectRequest {
                    bucket,
                    ..Default::default()
                },
                data.to_vec(),
                &upload_type,
            )
            .await
            .map_err(|e| Self::net("GCS upload_object", e))?;
        Ok(())
    }

    async fn append(&self, _path: &str, _data: &[u8]) -> FsResult<()> {
        Err(FilesystemError::InvalidOperation(
            "GCS objects are immutable; append is unsupported".to_string(),
        ))
    }

    async fn delete(&self, path: &str) -> FsResult<()> {
        let (bucket, object) = Self::parse(path)?;
        self.client
            .delete_object(&DeleteObjectRequest {
                bucket,
                object,
                ..Default::default()
            })
            .await
            .map_err(|e| Self::net("GCS delete_object", e))
    }

    async fn exists(&self, path: &str) -> FsResult<bool> {
        let (bucket, object) = Self::parse(path)?;
        Ok(self
            .client
            .get_object(&GetObjectRequest {
                bucket,
                object,
                ..Default::default()
            })
            .await
            .is_ok())
    }

    async fn metadata(&self, path: &str) -> FsResult<FsFileMetadata> {
        let (bucket, object) = Self::parse(path)?;
        let obj = self
            .client
            .get_object(&GetObjectRequest {
                bucket,
                object,
                ..Default::default()
            })
            .await
            .map_err(|e| Self::net("GCS get_object", e))?;
        Ok(FsFileMetadata {
            path: path.to_string(),
            size: obj.size.max(0) as u64,
            is_directory: false,
            etag: Some(obj.etag),
            ..Default::default()
        })
    }

    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        let (bucket, prefix) = Self::parse(path)?;
        let res = self
            .client
            .list_objects(&ListObjectsRequest {
                bucket: bucket.clone(),
                prefix: Some(prefix),
                ..Default::default()
            })
            .await
            .map_err(|e| Self::net("GCS list_objects", e))?;
        let mut entries = Vec::new();
        for obj in res.items.unwrap_or_default() {
            let name = obj.name.rsplit('/').next().unwrap_or(&obj.name).to_string();
            entries.push(DirEntry {
                name,
                url: format!("gs://{bucket}/{}", obj.name),
                metadata: FsFileMetadata {
                    path: format!("gs://{bucket}/{}", obj.name),
                    size: obj.size.max(0) as u64,
                    is_directory: false,
                    etag: Some(obj.etag),
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
        // GCS has a native copy/rewrite; download+upload keeps this backend lean
        // and is sufficient for the cold-path use (small index objects).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_gs_and_gcs_schemes() {
        assert_eq!(
            GcsFileSystem::parse("gs://bucket/obj.bin").unwrap(),
            ("bucket".to_string(), "obj.bin".to_string())
        );
        assert_eq!(
            GcsFileSystem::parse("gcs://bucket/a/b.bin").unwrap(),
            ("bucket".to_string(), "a/b.bin".to_string())
        );
    }

    #[test]
    fn parse_rejects_non_gcs_and_incomplete() {
        assert!(GcsFileSystem::parse("s3://b/k").is_err());
        assert!(GcsFileSystem::parse("gs://bucket-only").is_err());
        assert!(GcsFileSystem::parse("gs://b/").is_err());
        assert!(GcsFileSystem::parse("gs:///obj").is_err());
    }

    #[tokio::test]
    async fn anonymous_endpoint_config_builds_a_client() {
        // fake-gcs shape: anonymous + custom endpoint must construct without auth I/O.
        let fs = GcsFileSystem::new(GcsConfig {
            endpoint_url: Some("http://127.0.0.1:4443".into()),
            anonymous: true,
            project_id: None,
        })
        .await;
        assert!(fs.is_ok(), "anonymous+endpoint client should build offline");
    }
}
