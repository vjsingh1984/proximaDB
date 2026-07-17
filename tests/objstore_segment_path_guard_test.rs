//! Source-guard (TD-OBJSTORE-4 defect-6 class): SST segment writers/readers must
//! never derive a local path by string-stripping a URL — a CLOUD URL stripped
//! into a "path" becomes a literal local `az:...` directory (write side: the
//! atomic promote stages nothing and the operation false-succeeds, letting WAL
//! deletion destroy the only durable copy; read side: the object can never be
//! found). The ONE sanctioned home for the file:// strip is
//! `sst/staged_write.rs` (`StagedSegmentWrite` / `read_object_bytes` /
//! `LocalizedSegment`), which routes cloud URLs through the FileSystem.

#[test]
fn sst_flush_compaction_search_never_strip_urls_to_paths() {
    for file in [
        "src/storage/engines/sst/flush/mod.rs",
        "src/storage/engines/sst/compaction.rs",
        "src/storage/engines/sst/search/mod.rs",
    ] {
        let src = std::fs::read_to_string(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), file))
            .unwrap_or_else(|e| panic!("read {file}: {e}"));
        assert!(
            !src.contains("strip_prefix(\"file://\")"),
            "{file} must not strip URLs into local paths — use \
             sst::staged_write (StagedSegmentWrite / read_object_bytes / \
             LocalizedSegment), the single sanctioned URL→path boundary"
        );
    }
}
