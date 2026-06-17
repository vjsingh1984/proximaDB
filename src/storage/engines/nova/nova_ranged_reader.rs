//! TD-040 — async, ranged parquet row-group reader for NOVA cold search.
//!
//! Reads ONLY the requested parquet row groups via object_store range GETs
//! (`ParquetObjectReader` + `ParquetRecordBatchStreamBuilder::with_row_groups`),
//! so vector-bounds pruning can physically skip row groups instead of pulling
//! the whole file (which `UnifiedParquetReader::read_all_records` does — it
//! `filesystem.read`s the entire object into memory).
//!
//! Batches are decoded with `ParquetReader::batch_to_records`, the SAME codec
//! the full-file path uses, so ranged-read records are byte-for-byte identical
//! to a full read (the recall-equality test depends on this).
//!
//! Returns `Ok(None)` when a ranged reader can't be built for the file's scheme
//! (unknown scheme, cloud feature/credentials absent, `head` failure) — the
//! caller falls back to a full read, so the search never fails for this reason.

use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use object_store::ObjectStore;
use parquet::arrow::async_reader::{ParquetObjectReader, ParquetRecordBatchStreamBuilder};
use proximadb_records::ProximaRecord;
use url::Url;

use crate::storage::engines::core::formats::columnar::columnar_query_engine::{
    CacheStrategy, ParquetReader, QueryConfig,
};

/// Streaming batch size for the ranged read.
const RANGED_BATCH_SIZE: usize = 1024;

/// Build a scheme-qualified URL for `file_path`. NOVA file paths are either
/// already-qualified (`file://…`, `s3://…`) or bare absolute local paths
/// (tests, local deployments); the latter become `file://` URLs.
fn to_object_url(file_path: &str) -> Option<Url> {
    if file_path.contains("://") {
        Url::parse(file_path).ok()
    } else {
        Url::from_file_path(file_path).ok()
    }
}

/// Read only `rg_indices` (parquet row-group indices) from `file_path`, decoding
/// to `ProximaRecord`s identical to a full read. `Ok(None)` ⇒ no ranged reader
/// is available for this scheme/file and the caller should fall back to a full
/// read. An empty `rg_indices` yields `Ok(Some(vec![]))`.
pub(crate) async fn read_selected_row_groups(
    file_path: &str,
    rg_indices: &[usize],
) -> Result<Option<Vec<ProximaRecord>>> {
    if rg_indices.is_empty() {
        return Ok(Some(Vec::new()));
    }

    let Some(url) = to_object_url(file_path) else {
        return Ok(None);
    };
    // `parse_url` maps the scheme to a concrete ObjectStore (file:// + memory://
    // always; cloud schemes only when their feature/credentials are present).
    let (store, path) = match object_store::parse_url(&url) {
        Ok(pair) => pair,
        Err(e) => {
            tracing::debug!("NOVA ranged reader: parse_url({url}) failed, full-read fallback: {e}");
            return Ok(None);
        }
    };
    let store: Arc<dyn ObjectStore> = Arc::from(store);
    let file_size = match store.head(&path).await {
        Ok(meta) => meta.size,
        Err(e) => {
            tracing::debug!("NOVA ranged reader: head({path}) failed, full-read fallback: {e}");
            return Ok(None);
        }
    };

    let reader = ParquetObjectReader::new(store, path).with_file_size(file_size);
    let stream_builder = match ParquetRecordBatchStreamBuilder::new(reader).await {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!("NOVA ranged reader: open failed, full-read fallback: {e}");
            return Ok(None);
        }
    };

    // `with_row_groups` makes the parquet reader fetch only the selected row
    // groups' column-chunk byte ranges (real ranged GETs), not the whole file.
    let mut stream = stream_builder
        .with_row_groups(rg_indices.to_vec())
        .with_batch_size(RANGED_BATCH_SIZE)
        .build()?;

    // Decode with the same codec as the full-file path so records match exactly.
    let decoder = ParquetReader::new(QueryConfig {
        enable_pushdown: false,
        enable_projection: false,
        enable_statistics: false,
        cache_strategy: CacheStrategy::None,
        limit: None,
        enable_parallel: false,
        parallel_workers: 1,
    });

    let mut records = Vec::new();
    while let Some(batch) = stream.next().await {
        let batch = batch?;
        records.extend(decoder.batch_to_records(batch)?);
    }
    Ok(Some(records))
}

/// Read the per-row-group row counts from `file_path`'s parquet footer (cheap —
/// metadata only, no column data). Used at flush time to align the TD-040 bounds
/// sidecar's row groups with the file's PHYSICAL parquet row groups. `Ok(None)`
/// ⇒ no ranged reader available for this scheme (caller skips the sidecar).
pub(crate) async fn read_row_group_row_counts(file_path: &str) -> Result<Option<Vec<usize>>> {
    let Some(url) = to_object_url(file_path) else {
        return Ok(None);
    };
    let (store, path) = match object_store::parse_url(&url) {
        Ok(pair) => pair,
        Err(_) => return Ok(None),
    };
    let store: Arc<dyn ObjectStore> = Arc::from(store);
    let file_size = match store.head(&path).await {
        Ok(meta) => meta.size,
        Err(_) => return Ok(None),
    };
    let reader = ParquetObjectReader::new(store, path).with_file_size(file_size);
    let builder = match ParquetRecordBatchStreamBuilder::new(reader).await {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let counts = builder
        .metadata()
        .row_groups()
        .iter()
        .map(|rg| rg.num_rows().max(0) as usize)
        .collect();
    Ok(Some(counts))
}
