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
use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use tokio::io::AsyncReadExt;

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
    const RESUMABLE_CHUNK_BYTES: u64 = 8 * 1024 * 1024;

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

    fn next_resumable_chunk(offset: u64, total: u64) -> Option<(usize, bool)> {
        let remaining = total.checked_sub(offset)?;
        if remaining == 0 {
            return None;
        }
        let bytes = remaining.min(Self::RESUMABLE_CHUNK_BYTES);
        Some((bytes as usize, bytes == remaining))
    }

    fn parse_resumable_range(value: &str) -> Option<u64> {
        value
            .strip_prefix("bytes=0-")
            .and_then(|end| end.parse::<u64>().ok())
    }

    async fn list_paginated(
        &self,
        path: &str,
        max_results: Option<i32>,
    ) -> FsResult<Vec<DirEntry>> {
        let (bucket, prefix) = Self::parse(path)?;
        let mut entries = Vec::new();
        let mut page_token = None;

        loop {
            let requested_page_token = page_token.clone();
            let res = self
                .client
                .list_objects(&ListObjectsRequest {
                    bucket: bucket.clone(),
                    prefix: Some(prefix.clone()),
                    max_results,
                    page_token: requested_page_token.clone(),
                    ..Default::default()
                })
                .await
                .map_err(|e| Self::net("GCS list_objects", e))?;

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

            page_token = res.next_page_token.filter(|token| !token.is_empty());
            match &page_token {
                None => break,
                Some(next) if Some(next) == requested_page_token.as_ref() => {
                    return Err(FilesystemError::Network(format!(
                        "GCS list_objects returned a repeated page token for {path}"
                    )));
                }
                Some(_) => {}
            }
        }

        Ok(entries)
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
        let bytes = self
            .client
            .download_object(
                &GetObjectRequest {
                    bucket,
                    object,
                    ..Default::default()
                },
                &Range(Some(offset), Some(offset + length - 1)),
            )
            .await
            .map_err(|e| Self::net("GCS download_object range", e))?;
        // ADR-030 / TD-158: physical GET boundary — feed the per-query I/O accumulator
        // (task-local; no-op outside a query scope). Always-on core counters.
        crate::observability::io_trace::record_range_gets(1);
        crate::observability::io_trace::record_bytes_read(bytes.len() as u64);
        Ok(bytes)
    }

    async fn write(&self, path: &str, data: &[u8], _options: Option<FileOptions>) -> FsResult<()> {
        // NOTE (ADR-036): the canonical `FileOptions.storage_class` → GCS storage
        // class (`ObjectAccessTier::as_gcs_storage_class`) is not yet wired here.
        // Unlike the object_store-backed Azure/S3 backends (which set the class via
        // `Attribute::StorageClass` on the PUT), this backend uses the native
        // `google-cloud-storage` Simple media upload, which cannot carry a storage
        // class — that needs a metadata/resumable upload. GCS is not the MVP cloud
        // (Azure is); tracked as a follow-up under TD-173. Writes use the bucket
        // default class today.
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

    fn supports_bounded_local_file_write(&self) -> bool {
        true
    }

    async fn write_local_file(
        &self,
        path: &str,
        local_path: &std::path::Path,
        _options: Option<FileOptions>,
    ) -> FsResult<u64> {
        let (bucket, object) = Self::parse(path)?;
        let mut file = tokio::fs::File::open(local_path).await?;
        let bytes = file.metadata().await?.len();
        let mut media = Media::new(object);
        media.content_length = Some(bytes);
        let upload_type = UploadType::Simple(media);
        let session = self
            .client
            .prepare_resumable_upload(
                &UploadObjectRequest {
                    bucket,
                    ..Default::default()
                },
                &upload_type,
            )
            .await
            .map_err(|error| Self::net("GCS resumable begin", error))?;

        if bytes == 0 {
            session
                .upload_single_chunk(Vec::<u8>::new(), 0)
                .await
                .map_err(|error| Self::net("GCS resumable empty upload", error))?;
            return Ok(0);
        }

        // The 0.15 SDK's `UploadStatus::ResumeIncomplete` drops GCS's `Range`
        // response header. The JSON API explicitly requires clients to advance
        // from the acknowledged range rather than assume a whole request was
        // persisted. Use the authenticated session URL directly and fail
        // closed on a partial acknowledgement; compaction can retry from its
        // authoritative inputs without risking a corrupt final object.
        let upload_http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| Self::net("GCS resumable HTTP client", error))?;
        let mut buffer = vec![0u8; Self::RESUMABLE_CHUNK_BYTES as usize];
        let mut offset = 0u64;
        while let Some((chunk_bytes, final_chunk)) = Self::next_resumable_chunk(offset, bytes) {
            file.read_exact(&mut buffer[..chunk_bytes]).await?;
            let end = offset + chunk_bytes as u64 - 1;
            let response = match upload_http
                .put(session.url())
                .header(CONTENT_RANGE, format!("bytes {offset}-{end}/{bytes}"))
                .header(CONTENT_LENGTH, chunk_bytes)
                .body(buffer[..chunk_bytes].to_vec())
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    let _ = session.cancel().await;
                    return Err(Self::net("GCS resumable chunk", error));
                }
            };
            let accepted = if final_chunk {
                response.status().is_success()
            } else {
                response.status().as_u16() == 308
                    && response
                        .headers()
                        .get(RANGE)
                        .and_then(|value| value.to_str().ok())
                        .and_then(Self::parse_resumable_range)
                        == Some(end)
            };
            if !accepted {
                let status = response.status();
                let acknowledged = response
                    .headers()
                    .get(RANGE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("missing");
                let _ = session.cancel().await;
                return Err(FilesystemError::Network(format!(
                    "GCS resumable chunk {offset}-{end}/{bytes} was not fully acknowledged: \
                     status={status}, range={acknowledged}"
                )));
            }
            offset = end + 1;
        }
        Ok(bytes)
    }

    async fn write_if_absent(
        &self,
        path: &str,
        data: &[u8],
        _options: Option<FileOptions>,
    ) -> FsResult<()> {
        let (bucket, object) = Self::parse(path)?;
        let upload_type = UploadType::Simple(Media::new(object));
        let result = self
            .client
            .upload_object(
                &UploadObjectRequest {
                    bucket,
                    if_generation_match: Some(0),
                    ..Default::default()
                },
                data.to_vec(),
                &upload_type,
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(google_cloud_storage::http::Error::Response(response))
                if response.code == 409 || response.code == 412 =>
            {
                Err(FilesystemError::AlreadyExists(path.to_string()))
            }
            Err(error) => Err(Self::net("GCS conditional upload_object", error)),
        }
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
        self.list_paginated(path, None).await
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

    #[test]
    fn resumable_chunk_plan_is_bounded_contiguous_and_finalized_once() {
        let total = 2 * GcsFileSystem::RESUMABLE_CHUNK_BYTES + 19;
        let first = GcsFileSystem::next_resumable_chunk(0, total).unwrap();
        let second = GcsFileSystem::next_resumable_chunk(first.0 as u64, total).unwrap();
        let third =
            GcsFileSystem::next_resumable_chunk(first.0 as u64 + second.0 as u64, total).unwrap();

        assert_eq!(
            first,
            (GcsFileSystem::RESUMABLE_CHUNK_BYTES as usize, false)
        );
        assert_eq!(
            second,
            (GcsFileSystem::RESUMABLE_CHUNK_BYTES as usize, false)
        );
        assert_eq!(third, (19, true));
        assert_eq!(GcsFileSystem::next_resumable_chunk(total, total), None);
    }

    #[test]
    fn resumable_range_parser_requires_a_contiguous_zero_based_ack() {
        assert_eq!(
            GcsFileSystem::parse_resumable_range("bytes=0-8388607"),
            Some(8_388_607)
        );
        assert_eq!(
            GcsFileSystem::parse_resumable_range("bytes=1-8388607"),
            None
        );
        assert_eq!(GcsFileSystem::parse_resumable_range("8388607"), None);
        assert_eq!(GcsFileSystem::parse_resumable_range("bytes=0-nope"), None);
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

    #[tokio::test]
    #[ignore = "needs fake-gcs-server — set PROXIMADB_GCS_TEST_ENDPOINT"]
    async fn list_consumes_all_pages_against_fake_gcs() {
        let Ok(endpoint) = std::env::var("PROXIMADB_GCS_TEST_ENDPOINT") else {
            eprintln!("skip: set PROXIMADB_GCS_TEST_ENDPOINT with fake-gcs-server running");
            return;
        };
        let bucket = std::env::var("PROXIMADB_GCS_TEST_BUCKET")
            .unwrap_or_else(|_| "proximadb-test".to_string());
        let fs = GcsFileSystem::new(GcsConfig {
            endpoint_url: Some(endpoint),
            anonymous: true,
            project_id: Some("proximadb".to_string()),
        })
        .await
        .expect("fake-gcs client");
        let prefix = format!("td-objstore-4/{}/", uuid::Uuid::new_v4());

        for index in 0..5 {
            let path = format!("gs://{bucket}/{prefix}{index}.bcwal");
            fs.write(&path, &[index], None).await.expect("seed object");
        }

        let prefix_path = format!("gs://{bucket}/{prefix}");
        assert!(
            !fs.exists(&prefix_path).await.expect("prefix HEAD"),
            "a flat-key prefix must not exist as an exact object"
        );
        let entries = fs
            .list_paginated(&prefix_path, Some(2))
            .await
            .expect("multi-page prefix LIST");
        assert_eq!(entries.len(), 5, "LIST must consume every GCS page");

        for entry in entries {
            fs.delete(&entry.url).await.expect("cleanup object");
        }
    }
}
