//! Object-store tier — archives sealed disk segments via `FilesystemFactory`
//! (`adls://`, `s3://`, `gcs://`, `file://`). Sealed segments are uploaded
//! asynchronously; the disk reaper deletes local copies after consumers
//! commit past the segment's last offset.
//!
//! ## Phase 1B scaffold
//!
//! Wired in a follow-up commit. The interface here exists so the rest of
//! the crate (and future integration code in `proximadb-server` startup)
//! can reference the upload + recover primitives.

#[allow(dead_code)]
pub struct ObjectTierUploader {
    /// Will hold the FilesystemFactory handle for the archive root.
    archive_root: Option<String>,
}

impl ObjectTierUploader {
    pub fn new(archive_root: Option<String>) -> Self {
        Self { archive_root }
    }

    /// Upload the named sealed segment from disk to the object archive.
    /// No-op until wired.
    pub async fn upload_sealed(&self, _segment_path: &str) -> crate::Result<()> {
        Ok(())
    }
}
