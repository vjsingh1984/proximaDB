//! External source reader (Phase 8 F5 / TD-090).
//!
//! Reads dense vectors + ids from an external Parquet source into
//! `ProximaRecord`s for in-place index building — the source is **never
//! copied** into ProximaDB storage; these records exist only long enough to
//! train/populate the AXIS index. Also computes a `snapshot_fingerprint` of the
//! source that is recorded as the projection lineage (`source_range`).
//!
//! The Parquet read uses the same `ParquetRecordBatchReaderBuilder` path as
//! `src/storage/formats/open/iceberg.rs` (`IcebergFormat::read_parquet_file`),
//! which is private to that module; the Arrow vector extraction mirrors
//! `columnar_query_reader.rs` (FixedSizeList preferred, List fallback).

use anyhow::{Context, Result};
use arrow_array::{FixedSizeListArray, Float32Array, ListArray, RecordBatch, StringArray};
use proximadb_records::{EmbeddingCell, ProximaRecord};

use super::types::ExternalCollectionSpec;

const READ_BATCH_SIZE: usize = 4096;

/// Read all `(id, vector)` rows from the external source described by `spec`
/// into `ProximaRecord`s (one dense embedding each). Validates that every
/// vector matches `spec.dimension`.
pub fn read_external_records(spec: &ExternalCollectionSpec) -> Result<Vec<ProximaRecord>> {
    let files = list_parquet_files(&spec.location)?;
    if files.is_empty() {
        anyhow::bail!(
            "external collection '{}': no Parquet files found at '{}'",
            spec.name,
            spec.location
        );
    }

    let mut records = Vec::new();
    for file in &files {
        for batch in read_parquet_batches(file, &spec.id_column, &spec.vector_column)? {
            extract_records(&batch, spec, &mut records)?;
        }
    }
    Ok(records)
}

/// Stable content fingerprint of the external source: an FNV-1a hash over the
/// sorted `(file_name, len, mtime_nanos)` tuples. Deterministic across runs so
/// Slice 2 can detect source-commit advance by recomputing and comparing.
pub fn snapshot_fingerprint(location: &str) -> Result<String> {
    let files = list_parquet_files(location)?;
    // FNV-1a (64-bit) over a canonical description of the file set.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let prime: u64 = 0x0000_0100_0000_01b3;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(prime);
        }
    };
    for path in &files {
        let meta = std::fs::metadata(path)
            .with_context(|| format!("stat external file '{}'", path.display()))?;
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        mix(name.as_bytes());
        mix(&meta.len().to_le_bytes());
        mix(&mtime_ns.to_le_bytes());
    }
    Ok(format!("fnv1a:{:016x}", hash))
}

/// Resolve `location` to a sorted list of Parquet files. Accepts either a
/// single `.parquet` file or a directory containing `.parquet` files.
fn list_parquet_files(location: &str) -> Result<Vec<std::path::PathBuf>> {
    let path = std::path::Path::new(location);
    let meta = std::fs::metadata(path)
        .with_context(|| format!("external location '{location}' is not accessible"))?;
    let mut files = Vec::new();
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)
            .with_context(|| format!("read external directory '{location}'"))?
        {
            let entry = entry?;
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("parquet") {
                files.push(p);
            }
        }
    } else {
        files.push(path.to_path_buf());
    }
    files.sort();
    Ok(files)
}

/// Read a single Parquet file, projecting just the id + vector columns.
/// Mirrors `IcebergFormat::read_parquet_file` (private to that module).
fn read_parquet_batches(
    path: &std::path::Path,
    id_column: &str,
    vector_column: &str,
) -> Result<Vec<RecordBatch>> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use std::fs::File;

    let file = File::open(path)
        .with_context(|| format!("open external Parquet file '{}'", path.display()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?.with_batch_size(READ_BATCH_SIZE);

    let schema = builder.schema();
    let indices: Vec<usize> = [id_column, vector_column]
        .iter()
        .filter_map(|name| schema.index_of(name).ok())
        .collect();
    let mask = parquet::arrow::ProjectionMask::roots(builder.parquet_schema(), indices);
    let reader = builder.with_projection(mask).build()?;
    Ok(reader.filter_map(|r| r.ok()).collect())
}

/// Extract `(id, vector)` rows from one batch into `ProximaRecord`s.
fn extract_records(
    batch: &RecordBatch,
    spec: &ExternalCollectionSpec,
    out: &mut Vec<ProximaRecord>,
) -> Result<()> {
    let num_rows = batch.num_rows();
    if num_rows == 0 {
        return Ok(());
    }

    let id_col = batch
        .column_by_name(&spec.id_column)
        .ok_or_else(|| anyhow::anyhow!("id column '{}' missing in source", spec.id_column))?;
    let ids = id_col
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| anyhow::anyhow!("id column '{}' is not Utf8", spec.id_column))?;

    let vec_col = batch.column_by_name(&spec.vector_column).ok_or_else(|| {
        anyhow::anyhow!("vector column '{}' missing in source", spec.vector_column)
    })?;
    let vectors = extract_vectors(vec_col, num_rows, &spec.vector_column)?;

    for (i, vector) in vectors.iter().enumerate() {
        let oid = ids.value(i).to_string();
        if vector.len() != spec.dimension {
            anyhow::bail!(
                "external collection '{}': row '{}' has dimension {} (expected {})",
                spec.name,
                oid,
                vector.len(),
                spec.dimension
            );
        }
        out.push(ProximaRecord {
            oid,
            embeddings: vec![EmbeddingCell::new_fp32(
                "external",
                "dense_vector",
                spec.dimension as u32,
                vector.clone(),
            )],
            ..Default::default()
        });
    }
    Ok(())
}

/// Decode the vector column (FixedSizeList preferred, List fallback) to
/// `Vec<Vec<f32>>`. Mirrors `columnar_query_reader.rs`.
fn extract_vectors(
    col: &arrow_array::ArrayRef,
    num_rows: usize,
    column_name: &str,
) -> Result<Vec<Vec<f32>>> {
    if let Some(fixed) = col.as_any().downcast_ref::<FixedSizeListArray>() {
        let values = fixed
            .values()
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| anyhow::anyhow!("vector column '{column_name}' values are not Float32"))?;
        let dim = fixed.value_length() as usize;
        let mut out = Vec::with_capacity(num_rows);
        for i in 0..num_rows {
            let start = i * dim;
            out.push((start..start + dim).map(|idx| values.value(idx)).collect());
        }
        Ok(out)
    } else if let Some(list) = col.as_any().downcast_ref::<ListArray>() {
        let mut out = Vec::with_capacity(num_rows);
        for i in 0..num_rows {
            let arr = list.value(i);
            let floats = arr
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| anyhow::anyhow!("vector column '{column_name}' values are not Float32"))?;
            out.push((0..floats.len()).map(|j| floats.value(j)).collect());
        }
        Ok(out)
    } else {
        anyhow::bail!("vector column '{column_name}' is neither FixedSizeList nor List of Float32")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use arrow_array::builder::{FixedSizeListBuilder, Float32Builder, StringBuilder};
    use arrow_schema::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;

    /// Write a tiny Parquet file with `id: Utf8` + `vector: FixedSizeList<Float32, dim>`.
    fn write_parquet(path: &std::path::Path, ids: &[&str], vectors: &[Vec<f32>], dim: i32) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    dim,
                ),
                false,
            ),
        ]));

        let mut id_builder = StringBuilder::new();
        for id in ids {
            id_builder.append_value(id);
        }
        let mut vec_builder = FixedSizeListBuilder::new(Float32Builder::new(), dim);
        for v in vectors {
            for x in v {
                vec_builder.values().append_value(*x);
            }
            vec_builder.append(true);
        }

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(id_builder.finish()), Arc::new(vec_builder.finish())],
        )
        .unwrap();

        let file = std::fs::File::create(path).unwrap();
        let mut writer = ArrowWriter::try_new(file, schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
    }

    fn tmp_path(suffix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "proximadb_extsrc_{}_{}.parquet",
            uuid::Uuid::new_v4().simple(),
            suffix
        ))
    }

    #[test]
    fn reads_ids_and_vectors_with_correct_dim() {
        let path = tmp_path("read");
        let ids = ["a", "b", "c"];
        let vectors = vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
        ];
        write_parquet(&path, &ids, &vectors, 4);

        let spec = ExternalCollectionSpec::parquet(
            "docs",
            path.to_str().unwrap(),
            "id",
            "vector",
            4,
        );
        let records = read_external_records(&spec).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].oid, "a");
        assert_eq!(records[1].embeddings[0].values.to_fp32_owned(), vectors[1]);
        assert_eq!(records[2].embeddings[0].dim, 4);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dimension_mismatch_is_rejected() {
        let path = tmp_path("dimmismatch");
        write_parquet(&path, &["a"], &[vec![1.0, 2.0, 3.0, 4.0]], 4);
        let spec =
            ExternalCollectionSpec::parquet("docs", path.to_str().unwrap(), "id", "vector", 8);
        assert!(read_external_records(&spec).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fingerprint_is_stable_and_changes_with_content() {
        let dir = std::env::temp_dir().join(format!(
            "proximadb_extsrc_dir_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        write_parquet(&dir.join("part-0.parquet"), &["a"], &[vec![1.0, 0.0]], 2);

        let loc = dir.to_str().unwrap();
        let fp1 = snapshot_fingerprint(loc).unwrap();
        let fp2 = snapshot_fingerprint(loc).unwrap();
        assert_eq!(fp1, fp2, "fingerprint must be stable across reads");
        assert!(fp1.starts_with("fnv1a:"));

        // Adding a file changes the fingerprint.
        write_parquet(&dir.join("part-1.parquet"), &["b"], &[vec![0.0, 1.0]], 2);
        let fp3 = snapshot_fingerprint(loc).unwrap();
        assert_ne!(fp1, fp3, "adding a file must change the fingerprint");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
