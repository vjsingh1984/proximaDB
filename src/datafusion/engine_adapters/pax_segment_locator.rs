// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! # PAX segment discovery (TD-097 / TD-OLAP-1 slice 2 prerequisite)
//!
//! Lists a collection's `.pax` segment files and produces [`FileSplit`]s for
//! [`super::pax_adapter::PaxSplitReader`]. Mirrors the SST adapter's file
//! discovery (`sst_adapter.rs`).
//!
//! **Slice 1 (this):** one `FileSplit` per segment (file-level). `PaxSplitReader`
//! reads the whole segment via `PaxSegmentScanner` (it ignores `split.offset`/
//! `length`), so a per-segment split suffices. `block_id=0`, `record_count=0`
//! (unknown without reading the segment trailer). Per-block splits + real
//! `row_count`/`block_stats` from the trailer are slice 2+.
//!
//! A missing directory (`NotFound`) yields an empty vec, not an error — a
//! collection with no PAX data yet scans nothing.

use std::sync::Arc;

use crate::storage::formats::FileSplit;
use crate::storage::persistence::filesystem::{FilesystemError, FilesystemFactory};

/// Discover `.pax` segment files under `base_path` and return one `FileSplit`
/// per file (file-level; `PaxSplitReader` reads whole segments). `NotFound` ⇒
/// empty vec (a collection with no PAX data yet scans nothing).
pub async fn discover_pax_segments(
    base_path: &str,
    filesystem_factory: &Arc<FilesystemFactory>,
) -> Result<Vec<FileSplit>, FilesystemError> {
    let filesystem = filesystem_factory.get_filesystem(base_path)?;
    let entries = match filesystem.list(base_path).await {
        Ok(e) => e,
        Err(FilesystemError::NotFound(_)) => return Ok(Vec::new()),
        Err(FilesystemError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(e) => return Err(e),
    };
    let mut splits = Vec::new();
    for entry in entries {
        if !entry.name.ends_with(".pax") {
            continue;
        }
        let file_path = format!("{base_path}/{}", entry.name);
        let size = entry.metadata.size;
        // One split per segment; PaxSplitReader ignores offset/length and reads
        // the whole file via PaxSegmentScanner. record_count unknown without the
        // trailer (slice 2+).
        splits.push(FileSplit::new_block(file_path, 0, 0, size, 0));
    }
    Ok(splits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::sst::segment_format::write_pax_segment;
    use proximadb_block_format::VectorQuant;
    use proximadb_records::ProximaRecord;

    fn rec() -> ProximaRecord {
        ProximaRecord {
            oid: "r1".into(),
            tenant_id: "t".into(),
            created_at_ns: 1,
            updated_at_ns: 1,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn empty_dir_yields_no_splits() {
        let tmp = tempfile::tempdir().unwrap();
        let base = format!("{}", tmp.path().display());
        let fs = Arc::new(FilesystemFactory::create_default().await.unwrap());
        let splits = discover_pax_segments(&base, &fs).await.unwrap();
        assert!(splits.is_empty());
    }

    #[tokio::test]
    async fn missing_dir_yields_no_splits_not_error() {
        let fs = Arc::new(FilesystemFactory::create_default().await.unwrap());
        // A path that doesn't exist → NotFound → empty (not error).
        let splits = discover_pax_segments("/definitely/not/a/real/path/xyz", &fs)
            .await
            .unwrap();
        assert!(splits.is_empty());
    }

    #[tokio::test]
    async fn lists_pax_files_as_splits_and_ignores_others() {
        let tmp = tempfile::tempdir().unwrap();
        let base = format!("{}", tmp.path().display());
        // Two real .pax segments + one non-.pax file (must be ignored).
        write_pax_segment(
            &tmp.path().join("seg0.pax"),
            &[rec()],
            "col",
            0,
            VectorQuant::Auto,
            None,
        )
        .unwrap();
        write_pax_segment(
            &tmp.path().join("seg1.pax"),
            &[rec()],
            "col",
            0,
            VectorQuant::Auto,
            None,
        )
        .unwrap();
        std::fs::write(tmp.path().join("ignore.txt"), b"x").unwrap();

        let fs = Arc::new(FilesystemFactory::create_default().await.unwrap());
        let splits = discover_pax_segments(&base, &fs).await.unwrap();
        assert_eq!(splits.len(), 2, "two .pax segments ⇒ two splits");
        assert!(splits.iter().all(|s| s.file_path.ends_with(".pax")));
    }
}
