//! Local-scratch staging for segment writers that target object-store URLs.
//!
//! The SST segment writers (`write_pax_segment_full`, `ArrowBlockWriter`) are
//! LOCAL-file writers. String-stripping a cloud staging URL into a "path"
//! creates a literal local `az:...` directory: the atomic promote then LISTs
//! the REMOTE staging prefix, finds nothing, moves nothing, and the operation
//! **false-succeeds** — on the flush path that let WAL deletion destroy the
//! only durable copy (TD-OBJSTORE-4 defect 6); on the compaction path it
//! would drop the merged output while the inputs get deleted. This primitive
//! makes the write coherent for BOTH bases: write locally, then upload the
//! bytes to the remote staging URL so the coordinator promotes a file that
//! actually exists. `file://`/bare-local bases keep the zero-copy direct
//! write (`remote_url = None`, finalize is a no-op).

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::storage::persistence::filesystem::FilesystemFactory;

/// A segment write staged for a possibly-remote target URL.
pub(crate) struct StagedSegmentWrite {
    /// Remote URL to upload to on finalize (`None` ⇒ direct local write).
    remote_url: Option<String>,
    /// Path the local-file segment writer must write to.
    local_path: String,
}

impl StagedSegmentWrite {
    /// Prepare a staged write for `target_url` (the full staging FILE url,
    /// e.g. `az://c/…/__flush/L0_x.pax` or `file:///…/__flush/L0_x.pax`).
    pub(crate) async fn begin(target_url: &str) -> Result<Self> {
        Self::begin_with_scratch(target_url, None).await
    }

    /// Prepare a staged write while placing remote-upload scratch below the
    /// caller's already-admitted local directory. Spill compaction must not
    /// escape its reserved filesystem by silently falling back to system tmp.
    pub(crate) async fn begin_in(target_url: &str, scratch_root: &std::path::Path) -> Result<Self> {
        Self::begin_with_scratch(target_url, Some(scratch_root)).await
    }

    async fn begin_with_scratch(
        target_url: &str,
        scratch_root: Option<&std::path::Path>,
    ) -> Result<Self> {
        let is_remote = target_url.contains("://") && !target_url.starts_with("file://");
        if is_remote {
            let scratch = scratch_root
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| std::env::temp_dir().join("proximadb-flush-staging"));
            tokio::fs::create_dir_all(&scratch)
                .await
                .context("create local segment-staging scratch dir")?;
            let name = target_url.rsplit('/').next().unwrap_or("segment.bin");
            let local = scratch.join(format!("{}-{}", uuid::Uuid::new_v4().simple(), name));
            Ok(Self {
                remote_url: Some(target_url.to_string()),
                local_path: local.to_string_lossy().into_owned(),
            })
        } else {
            let local = target_url
                .strip_prefix("file://")
                .unwrap_or(target_url)
                .to_string();
            if let Some(parent) = std::path::Path::new(&local).parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .context("create local staging parent dir")?;
            }
            Ok(Self {
                remote_url: None,
                local_path: local,
            })
        }
    }

    /// The local path the segment writer must write to.
    pub(crate) fn local_path(&self) -> &str {
        &self.local_path
    }

    /// Promote the staged bytes to the remote staging URL (no-op for local
    /// bases) and return the segment's byte count — callers must NOT probe the
    /// scratch path afterwards (it is removed here; the post-finalize size
    /// probe reading the deleted scratch file made cloud compaction fail 100%
    /// of the time in review round 1 of this fix). Sidecar-aware: an Arrow
    /// segment's `{path}.idx` (required by `ArrowBlockReader::open`) is
    /// uploaded alongside as `{remote}.idx` and its scratch removed too.
    pub(crate) async fn finalize(mut self, factory: &Arc<FilesystemFactory>) -> Result<u64> {
        if let Some(remote) = self.remote_url.take() {
            let fs = factory
                .get_filesystem(&remote)
                .map_err(|e| anyhow::anyhow!("staging filesystem for {remote}: {e}"))?;
            let bytes = fs
                .write_local_file(&remote, std::path::Path::new(&self.local_path), None)
                .await
                .map_err(|e| anyhow::anyhow!("upload staged segment to {remote}: {e}"))?;
            let _ = tokio::fs::remove_file(&self.local_path).await;
            // Arrow sidecar pair (best-effort presence, mandatory upload if present).
            let sidecar = format!("{}.idx", self.local_path);
            if tokio::fs::try_exists(&sidecar).await.unwrap_or(false) {
                fs.write_local_file(
                    &format!("{remote}.idx"),
                    std::path::Path::new(&sidecar),
                    None,
                )
                .await
                .map_err(|e| anyhow::anyhow!("upload Arrow sidecar to {remote}.idx: {e}"))?;
                let _ = tokio::fs::remove_file(&sidecar).await;
            }
            tracing::debug!(remote = %remote, bytes, "staged segment uploaded");
            Ok(bytes)
        } else {
            let meta = tokio::fs::metadata(&self.local_path)
                .await
                .context("stat locally written segment")?;
            Ok(meta.len())
        }
    }
}

impl Drop for StagedSegmentWrite {
    /// Leak guard: a writer failure between `begin` and `finalize` must not
    /// strand the scratch file (+ possible Arrow sidecar). After a successful
    /// `finalize` (which takes `remote_url`) this is a no-op.
    fn drop(&mut self) {
        if self.remote_url.is_some() {
            let _ = std::fs::remove_file(&self.local_path);
            let _ = std::fs::remove_file(format!("{}.idx", self.local_path));
        }
    }
}

/// Read a whole segment object, routing cloud URLs through the `FileSystem`
/// (the defect-6 READ class: string-stripping a cloud URL and `tokio::fs::read`ing
/// the result can never find the object).
pub(crate) async fn read_object_bytes(
    factory: &Arc<FilesystemFactory>,
    url: &str,
) -> Result<Vec<u8>> {
    if url.contains("://") && !url.starts_with("file://") {
        let fs = factory
            .get_filesystem(url)
            .map_err(|e| anyhow::anyhow!("segment filesystem for {url}: {e}"))?;
        fs.read(url)
            .await
            .map_err(|e| anyhow::anyhow!("read segment object {url}: {e}"))
    } else {
        let local = url.strip_prefix("file://").unwrap_or(url);
        tokio::fs::read(local)
            .await
            .with_context(|| format!("read local segment {url}"))
    }
}

/// A segment made readable at a LOCAL path for path-based readers
/// (`ArrowBlockReader::open`). Cloud objects download to a scratch file that is
/// removed on drop; local paths pass through untouched.
pub(crate) struct LocalizedSegment {
    path: String,
    scratch: bool,
}

impl LocalizedSegment {
    pub(crate) async fn fetch(factory: &Arc<FilesystemFactory>, url: &str) -> Result<Self> {
        if url.contains("://") && !url.starts_with("file://") {
            let bytes = read_object_bytes(factory, url).await?;
            let dir = std::env::temp_dir().join("proximadb-segment-scratch");
            tokio::fs::create_dir_all(&dir)
                .await
                .context("create segment scratch dir")?;
            let name = url.rsplit('/').next().unwrap_or("segment.bin");
            let path = dir.join(format!("{}-{}", uuid::Uuid::new_v4().simple(), name));
            tokio::fs::write(&path, &bytes)
                .await
                .context("stage segment for local reader")?;
            // Sidecar pair: Arrow readers hard-require `{path}.idx` — fetch it
            // when the remote has one (absent for PAX; best-effort probe).
            if let Ok(fs) = factory.get_filesystem(url) {
                let remote_idx = format!("{url}.idx");
                if let Ok(idx_bytes) = fs.read(&remote_idx).await {
                    let _ = tokio::fs::write(format!("{}.idx", path.display()), &idx_bytes).await;
                }
            }
            Ok(Self {
                path: path.to_string_lossy().into_owned(),
                scratch: true,
            })
        } else {
            Ok(Self {
                path: url.strip_prefix("file://").unwrap_or(url).to_string(),
                scratch: false,
            })
        }
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }
}

impl Drop for LocalizedSegment {
    fn drop(&mut self) {
        if self.scratch {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(format!("{}.idx", self.path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_target_writes_direct_and_finalize_is_noop() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = format!("file://{}/coll/data/__flush/L0_x.pax", tmp.path().display());
        let staged = StagedSegmentWrite::begin(&target).await.expect("begin");
        // Direct local path (parent pre-created), no scratch indirection.
        assert!(staged.local_path().ends_with("/coll/data/__flush/L0_x.pax"));
        assert!(!staged.local_path().contains("proximadb-flush-staging"));
        std::fs::write(staged.local_path(), b"segment").expect("write");
        let factory = Arc::new(
            FilesystemFactory::create(Default::default())
                .await
                .expect("factory"),
        );
        staged.finalize(&factory).await.expect("finalize");
        assert!(
            tmp.path().join("coll/data/__flush/L0_x.pax").exists(),
            "local write must land at the target path"
        );
    }

    #[tokio::test]
    async fn remote_target_stages_to_scratch_never_a_literal_scheme_dir() {
        let staged = StagedSegmentWrite::begin("az://container/coll/data/__flush/L0_y.pax")
            .await
            .expect("begin");
        // The writer path must be a REAL local scratch file — never a literal
        // `az:...` path (the URL-as-local-path artifact class).
        assert!(staged.local_path().contains("proximadb-flush-staging"));
        assert!(!staged.local_path().contains("az:"));
    }
}
