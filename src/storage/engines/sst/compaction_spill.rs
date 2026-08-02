//! Deterministic, bounded local-scratch primitives for canonical compaction.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use proximadb_block_format::{
    FlatRow, PaxBlockReader, sq8_codes_offset, sq8_decode_codes, sq8_region_header_len,
};
use proximadb_records::{EmbeddingCell, EmbeddingValues, ProximaRecord};
use proximadb_storage_common::segment_layout::{SegmentFooterIndex, SegmentHeaderPrefix};
use serde::{Deserialize, Serialize};

use crate::core::search::mvcc_resolution::{effective_version, is_append_only_oid};
use crate::storage::engines::sst::error::{Result, SstError};
use crate::storage::persistence::filesystem::FilesystemFactory;

const RUN_MAGIC: &[u8; 8] = b"PXSPLRUN";
const RUN_FORMAT_VERSION: u16 = 1;
const FRAME_HEADER_BYTES: u64 = 8;
const MAX_FRAME_BYTES: u64 = 1 << 30;

/// A canonical record plus its stable position in the compaction input stream.
///
/// `ProximaRecord::schema_version` is intentionally skipped by serde because
/// the durable WAL has its own version envelope. Scratch runs are ephemeral,
/// but must preserve precision-aware records within a running process, so this
/// wrapper carries the field explicitly and restores it after decoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SpillRecord {
    source_ordinal: u64,
    schema_version: u8,
    record: ProximaRecord,
}

impl SpillRecord {
    pub(crate) fn new(source_ordinal: u64, record: ProximaRecord) -> Self {
        Self {
            source_ordinal,
            schema_version: record.schema_version,
            record,
        }
    }

    fn restore_schema_version(mut self) -> Self {
        self.record.schema_version = self.schema_version;
        self
    }
}

/// Auditable resource-shape statistics for one external MVCC pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ExternalMvccStats {
    pub(crate) input_records: u64,
    pub(crate) output_records: u64,
    pub(crate) initial_run_count: usize,
    pub(crate) merge_pass_count: usize,
    pub(crate) max_open_runs: usize,
    pub(crate) peak_run_buffer_bytes: u64,
    pub(crate) scratch_bytes_written: u64,
}

/// Owns the task scratch directory. Dropping this value reclaims every run,
/// including after a downstream error or cancellation.
pub(crate) struct ExternalMvccOutput {
    task_directory: tempfile::TempDir,
    output_path: PathBuf,
    stats: ExternalMvccStats,
}

impl ExternalMvccOutput {
    pub(crate) fn output_path(&self) -> &Path {
        &self.output_path
    }

    pub(crate) fn stats(&self) -> &ExternalMvccStats {
        &self.stats
    }

    #[cfg(test)]
    fn task_path(&self) -> &Path {
        self.task_directory.path()
    }

    #[cfg(test)]
    fn read_records(&self) -> Result<Vec<SpillRecord>> {
        let mut reader = FramedRunReader::open(&self.output_path)?;
        let mut records = Vec::new();
        while let Some(record) = reader.read_next()? {
            records.push(record);
        }
        Ok(records)
    }
}

/// Resolve an input stream through deterministic bounded external runs.
///
/// `max_run_buffer_bytes` bounds each in-memory sort batch by a conservative
/// serialized-size estimate. `max_merge_fan_in` bounds both open file handles
/// and one-record-per-run merge memory. When there are more runs, sorted
/// intermediate passes collapse them before the final MVCC pass.
pub(crate) fn resolve_external_mvcc<I>(
    scratch_root: &Path,
    records: I,
    max_run_buffer_bytes: u64,
    max_merge_fan_in: usize,
    current_timestamp_ns: i64,
) -> Result<ExternalMvccOutput>
where
    I: IntoIterator<Item = Result<SpillRecord>>,
{
    let mut builder = ExternalMvccBuilder::new(
        scratch_root,
        max_run_buffer_bytes,
        max_merge_fan_in,
        current_timestamp_ns,
    )?;
    for record in records {
        builder.push(record?)?;
    }
    builder.finish()
}

/// Incremental front-end for the external MVCC pipeline. Async ranged readers
/// push one decoded block at a time without first materializing the corpus.
pub(crate) struct ExternalMvccBuilder {
    task_directory: tempfile::TempDir,
    max_run_buffer_bytes: u64,
    max_merge_fan_in: usize,
    current_timestamp_ns: i64,
    stats: ExternalMvccStats,
    runs: Vec<PathBuf>,
    buffer: Vec<SpillRecord>,
    buffered_bytes: u64,
}

impl ExternalMvccBuilder {
    pub(crate) fn new(
        scratch_root: &Path,
        max_run_buffer_bytes: u64,
        max_merge_fan_in: usize,
        current_timestamp_ns: i64,
    ) -> Result<Self> {
        if max_run_buffer_bytes == 0 {
            return Err(SstError::InvalidArgument(
                "spill MVCC run buffer must be greater than zero".to_string(),
            ));
        }
        if max_merge_fan_in < 2 {
            return Err(SstError::InvalidArgument(
                "spill MVCC merge fan-in must be at least two".to_string(),
            ));
        }

        Ok(Self {
            task_directory: tempfile::Builder::new()
                .prefix("proximadb-compaction-spill-")
                .tempdir_in(scratch_root)?,
            max_run_buffer_bytes,
            max_merge_fan_in,
            current_timestamp_ns,
            stats: ExternalMvccStats::default(),
            runs: Vec::new(),
            buffer: Vec::new(),
            buffered_bytes: 0,
        })
    }

    pub(crate) fn push(&mut self, record: SpillRecord) -> Result<()> {
        let record_bytes = estimated_buffer_bytes(&record)?;
        if !self.buffer.is_empty()
            && self.buffered_bytes.saturating_add(record_bytes) > self.max_run_buffer_bytes
        {
            self.flush_run()?;
        }
        self.buffered_bytes = self.buffered_bytes.saturating_add(record_bytes);
        self.stats.peak_run_buffer_bytes =
            self.stats.peak_run_buffer_bytes.max(self.buffered_bytes);
        self.stats.input_records = self.stats.input_records.saturating_add(1);
        self.buffer.push(record);
        Ok(())
    }

    fn flush_run(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let path = self
            .task_directory
            .path()
            .join(format!("oid-pass-0000-run-{:06}.pxrun", self.runs.len()));
        self.stats.scratch_bytes_written = self
            .stats
            .scratch_bytes_written
            .saturating_add(write_sorted_run(&path, &mut self.buffer)?);
        self.runs.push(path);
        self.buffered_bytes = 0;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<ExternalMvccOutput> {
        self.flush_run()?;
        self.stats.initial_run_count = self.runs.len();

        let mut pass = 0usize;
        while self.runs.len() > self.max_merge_fan_in {
            pass = pass.saturating_add(1);
            let mut next_runs = Vec::with_capacity(self.runs.len().div_ceil(self.max_merge_fan_in));
            for (group_index, group) in self.runs.chunks(self.max_merge_fan_in).enumerate() {
                let output = self
                    .task_directory
                    .path()
                    .join(format!("oid-pass-{pass:04}-run-{group_index:06}.pxrun"));
                self.stats.max_open_runs = self.stats.max_open_runs.max(group.len());
                self.stats.scratch_bytes_written = self
                    .stats
                    .scratch_bytes_written
                    .saturating_add(merge_sorted_runs(group, &output)?);
                next_runs.push(output);
            }
            for old_run in &self.runs {
                std::fs::remove_file(old_run)?;
            }
            self.runs = next_runs;
        }
        self.stats.merge_pass_count = pass;

        let output_path = self.task_directory.path().join("mvcc-winners.pxrun");
        self.stats.max_open_runs = self.stats.max_open_runs.max(self.runs.len());
        let (bytes_written, output_records) =
            write_mvcc_winners(&self.runs, &output_path, self.current_timestamp_ns)?;
        self.stats.scratch_bytes_written = self
            .stats
            .scratch_bytes_written
            .saturating_add(bytes_written);
        self.stats.output_records = output_records;
        for old_run in &self.runs {
            std::fs::remove_file(old_run)?;
        }

        Ok(ExternalMvccOutput {
            task_directory: self.task_directory,
            output_path,
            stats: self.stats,
        })
    }
}

/// Read port used by spill compaction. Keeping it smaller than `FileSystem`
/// makes the one-block-at-a-time contract directly testable with an in-memory
/// range source while production delegates to the scheme-aware factory.
#[async_trait]
pub(crate) trait CompactionRangeSource: Send + Sync {
    async fn size(&self, path: &str) -> Result<u64>;
    async fn read_range(&self, path: &str, offset: u64, length: u64) -> Result<Vec<u8>>;
    async fn read_all(&self, path: &str) -> Result<Vec<u8>>;
}

#[async_trait]
impl CompactionRangeSource for FilesystemFactory {
    async fn size(&self, path: &str) -> Result<u64> {
        Ok(self.metadata(path).await?.size)
    }

    async fn read_range(&self, path: &str, offset: u64, length: u64) -> Result<Vec<u8>> {
        Ok(FilesystemFactory::read_range(self, path, offset, length).await?)
    }

    async fn read_all(&self, path: &str) -> Result<Vec<u8>> {
        Ok(FilesystemFactory::read(self, path).await?)
    }
}

/// Physical shape of the compaction input pass. `largest_range_bytes` is the
/// concrete assertion that coalesced inputs were never whole-object decoded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RangedInputStats {
    pub(crate) input_files: usize,
    pub(crate) coalesced_files: usize,
    pub(crate) legacy_whole_file_fallbacks: usize,
    pub(crate) block_range_reads: u64,
    pub(crate) sq8_range_reads: u64,
    pub(crate) largest_range_bytes: u64,
}

impl RangedInputStats {
    fn observe_range(&mut self, length: u64) {
        self.largest_range_bytes = self.largest_range_bytes.max(length);
    }
}

/// Range-decode immutable inputs into external MVCC runs.
///
/// Coalesced PAX reads its header and footer once, then alternates one Region-D
/// block with only that block's fixed-stride Region-B SQ8 rows. Legacy inputs
/// remain mixed-read-safe through the canonical whole-file router, but only one
/// legacy file is resident at a time. This fallback is observable and is not a
/// claim of bounded memory for obsolete formats.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn resolve_external_mvcc_from_segments(
    source: &dyn CompactionRangeSource,
    input_paths: &[String],
    scratch_root: &Path,
    max_run_buffer_bytes: u64,
    max_merge_fan_in: usize,
    current_timestamp_ns: i64,
    embedding_model_ids: &[String],
    user_column_keys: &[String],
    tenant_ctx: Option<&str>,
) -> Result<(ExternalMvccOutput, RangedInputStats)> {
    let mut builder = ExternalMvccBuilder::new(
        scratch_root,
        max_run_buffer_bytes,
        max_merge_fan_in,
        current_timestamp_ns,
    )?;
    let mut ordinal = 0u64;
    let mut stats = RangedInputStats {
        input_files: input_paths.len(),
        ..RangedInputStats::default()
    };

    for path in input_paths {
        let size = source.size(path).await?;
        let prefix_len = size.min(72);
        let prefix = source.read_range(path, 0, prefix_len).await?;
        stats.observe_range(prefix_len);

        match SegmentHeaderPrefix::parse(&prefix) {
            Ok(header) => {
                read_coalesced_segment_into_builder(
                    source,
                    path,
                    size,
                    &header,
                    embedding_model_ids,
                    user_column_keys,
                    tenant_ctx,
                    &mut ordinal,
                    &mut builder,
                    &mut stats,
                )
                .await?;
                stats.coalesced_files = stats.coalesced_files.saturating_add(1);
            }
            Err(_) => {
                let bytes = source.read_all(path).await?;
                let records = crate::storage::engines::sst::segment_format::read_segment_records(
                    &bytes,
                    embedding_model_ids,
                    user_column_keys,
                    tenant_ctx,
                )?;
                stats.legacy_whole_file_fallbacks =
                    stats.legacy_whole_file_fallbacks.saturating_add(1);
                for record in records {
                    builder.push(SpillRecord::new(ordinal, record))?;
                    ordinal = ordinal.saturating_add(1);
                }
            }
        }
    }

    Ok((builder.finish()?, stats))
}

#[allow(clippy::too_many_arguments)]
async fn read_coalesced_segment_into_builder(
    source: &dyn CompactionRangeSource,
    path: &str,
    segment_size: u64,
    header: &SegmentHeaderPrefix,
    embedding_model_ids: &[String],
    user_column_keys: &[String],
    tenant_ctx: Option<&str>,
    ordinal: &mut u64,
    builder: &mut ExternalMvccBuilder,
    stats: &mut RangedInputStats,
) -> Result<()> {
    validate_extent("footer", header.footer_off, header.footer_len, segment_size)?;
    let footer_bytes = source
        .read_range(path, header.footer_off, header.footer_len)
        .await?;
    stats.observe_range(header.footer_len);
    let footer = SegmentFooterIndex::parse(&footer_bytes)?;
    if footer.rabitq_off != header.rabitq_off
        || footer.rabitq_len != header.rabitq_len
        || footer.sq8_off != header.sq8_off
        || footer.sq8_len != header.sq8_len
    {
        return Err(SstError::Compaction(format!(
            "spill input {path} header/footer region extents disagree"
        )));
    }
    if footer.has_f32_tier {
        return Err(SstError::Compaction(format!(
            "spill input {path} has an exact-f32 tier; bounded exact-tier projection is required before spill"
        )));
    }
    if footer.embed_count > 1 {
        return Err(SstError::Compaction(format!(
            "spill input {path} has {} embedding columns; bounded multi-embedding projection is not implemented",
            footer.embed_count
        )));
    }

    let (validity, codes_base, sq8_params) = if footer.sq8_len > 0 {
        validate_extent("SQ8", footer.sq8_off, footer.sq8_len, segment_size)?;
        let row_count = usize::try_from(footer.row_count).map_err(|_| {
            SstError::Compaction(format!("spill input {path} row count exceeds usize"))
        })?;
        let dim = usize::try_from(footer.embed_dim).map_err(|_| {
            SstError::Compaction(format!("spill input {path} dimension exceeds usize"))
        })?;
        let bitmap_len = proximadb_block_format::coalesced_sq8::bitmap_len(row_count);
        let codes_offset = sq8_codes_offset(row_count);
        let expected_len = codes_offset
            .checked_add(row_count.checked_mul(dim).ok_or_else(|| {
                SstError::Compaction(format!("spill input {path} SQ8 size overflows usize"))
            })?)
            .ok_or_else(|| {
                SstError::Compaction(format!("spill input {path} SQ8 extent overflows usize"))
            })?;
        if expected_len as u64 > footer.sq8_len {
            return Err(SstError::Compaction(format!(
                "spill input {path} SQ8 region is truncated: expected {expected_len}, got {}",
                footer.sq8_len
            )));
        }
        let bitmap_offset = footer
            .sq8_off
            .saturating_add(sq8_region_header_len() as u64);
        let validity = source
            .read_range(path, bitmap_offset, bitmap_len as u64)
            .await?;
        stats.observe_range(bitmap_len as u64);
        (
            validity,
            footer.sq8_off.saturating_add(codes_offset as u64),
            Some(
                proximadb_block_format::coalesced_sq8::params_from_min_scale(
                    footer.sq8_min,
                    footer.sq8_scale,
                ),
            ),
        )
    } else {
        (Vec::new(), 0, None)
    };

    let mut global_row = 0usize;
    for block in &footer.blocks {
        validate_extent(
            "Region D block",
            block.offset,
            block.size as u64,
            segment_size,
        )?;
        let block_bytes = source
            .read_range(path, block.offset, block.size as u64)
            .await?;
        stats.block_range_reads = stats.block_range_reads.saturating_add(1);
        stats.observe_range(block.size as u64);
        let reader = PaxBlockReader::open(&block_bytes)?;
        let mut records = FlatRow::from_block_reader(&reader)?
            .into_iter()
            .map(|flat| flat.into_record(embedding_model_ids, user_column_keys, tenant_ctx))
            .collect::<anyhow::Result<Vec<_>>>()?;
        if records.len() != block.row_count as usize {
            return Err(SstError::Compaction(format!(
                "spill input {path} block row count mismatch: footer={}, decoded={}",
                block.row_count,
                records.len()
            )));
        }

        if let Some(params) = &sq8_params {
            let dim = footer.embed_dim as usize;
            let code_len = records.len().checked_mul(dim).ok_or_else(|| {
                SstError::Compaction(format!("spill input {path} block SQ8 size overflows"))
            })?;
            let code_offset = codes_base
                .saturating_add((global_row as u64).saturating_mul(footer.embed_dim as u64));
            let codes = source
                .read_range(path, code_offset, code_len as u64)
                .await?;
            stats.sq8_range_reads = stats.sq8_range_reads.saturating_add(1);
            stats.observe_range(code_len as u64);
            if codes.len() != code_len {
                return Err(SstError::Compaction(format!(
                    "spill input {path} returned a short SQ8 block range"
                )));
            }
            for (row_in_block, record) in records.iter_mut().enumerate() {
                let row = global_row.saturating_add(row_in_block);
                let present = validity
                    .get(row >> 3)
                    .is_some_and(|byte| (byte >> (row & 7)) & 1 == 1);
                if present && record.embeddings.is_empty() {
                    let start = row_in_block.saturating_mul(dim);
                    let end = start.saturating_add(dim);
                    let vector = sq8_decode_codes(&codes[start..end], params);
                    record.embeddings.push(EmbeddingCell {
                        modality: "dense".to_string(),
                        dim: footer.embed_dim,
                        values: EmbeddingValues::Fp32(vector),
                        ..EmbeddingCell::default()
                    });
                }
            }
        }

        for record in records {
            builder.push(SpillRecord::new(*ordinal, record))?;
            *ordinal = (*ordinal).saturating_add(1);
        }
        global_row = global_row.saturating_add(block.row_count as usize);
    }
    if global_row as u64 != footer.row_count {
        return Err(SstError::Compaction(format!(
            "spill input {path} footer rows={} but block table rows={global_row}",
            footer.row_count
        )));
    }
    Ok(())
}

fn validate_extent(name: &str, offset: u64, length: u64, segment_size: u64) -> Result<()> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| SstError::Compaction(format!("spill input {name} extent overflows u64")))?;
    if end > segment_size {
        return Err(SstError::Compaction(format!(
            "spill input {name} extent {offset}..{end} exceeds segment size {segment_size}"
        )));
    }
    Ok(())
}

fn estimated_buffer_bytes(record: &SpillRecord) -> Result<u64> {
    let serialized = bincode::serialized_size(record)?;
    // The decoded form owns strings, vectors, and property nodes in addition
    // to the serialized payload. Two payload widths plus the root struct is a
    // conservative, deterministic accounting unit for run admission.
    Ok(serialized
        .saturating_mul(2)
        .saturating_add(std::mem::size_of::<SpillRecord>() as u64))
}

fn spill_order(left: &SpillRecord, right: &SpillRecord) -> Ordering {
    left.record
        .oid
        .cmp(&right.record.oid)
        .then_with(|| effective_version(&left.record).cmp(&effective_version(&right.record)))
        .then_with(|| left.record.created_at_ns.cmp(&right.record.created_at_ns))
        .then_with(|| left.record.updated_at_ns.cmp(&right.record.updated_at_ns))
        .then_with(|| left.source_ordinal.cmp(&right.source_ordinal))
}

fn write_sorted_run(path: &Path, records: &mut Vec<SpillRecord>) -> Result<u64> {
    records.sort_unstable_by(spill_order);
    let mut writer = FramedRunWriter::create(path)?;
    for record in records.drain(..) {
        writer.write_record(&record)?;
    }
    writer.finish()
}

#[derive(Debug)]
struct HeapEntry {
    run_index: usize,
    record: SpillRecord,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.run_index == other.run_index && spill_order(&self.record, &other.record).is_eq()
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap; reverse the record ordering so the global
        // minimum is popped first. Run index is a deterministic final tie.
        spill_order(&other.record, &self.record).then_with(|| other.run_index.cmp(&self.run_index))
    }
}

fn open_merge_heap(paths: &[PathBuf]) -> Result<(Vec<FramedRunReader>, BinaryHeap<HeapEntry>)> {
    let mut readers = Vec::with_capacity(paths.len());
    let mut heap = BinaryHeap::with_capacity(paths.len());
    for (run_index, path) in paths.iter().enumerate() {
        let mut reader = FramedRunReader::open(path)?;
        if let Some(record) = reader.read_next()? {
            heap.push(HeapEntry { run_index, record });
        }
        readers.push(reader);
    }
    Ok((readers, heap))
}

fn advance_run(
    readers: &mut [FramedRunReader],
    heap: &mut BinaryHeap<HeapEntry>,
    run_index: usize,
) -> Result<()> {
    if let Some(record) = readers[run_index].read_next()? {
        heap.push(HeapEntry { run_index, record });
    }
    Ok(())
}

fn merge_sorted_runs(paths: &[PathBuf], output: &Path) -> Result<u64> {
    let (mut readers, mut heap) = open_merge_heap(paths)?;
    let mut writer = FramedRunWriter::create(output)?;
    while let Some(entry) = heap.pop() {
        let run_index = entry.run_index;
        writer.write_record(&entry.record)?;
        advance_run(&mut readers, &mut heap, run_index)?;
    }
    writer.finish()
}

struct VersionGroup {
    oid: String,
    expected_version: u64,
    blocked_by_gap: bool,
    deleted: bool,
    winner: Option<SpillRecord>,
}

impl VersionGroup {
    fn begin(record: &SpillRecord) -> Self {
        let start = effective_version(&record.record);
        Self {
            oid: record.record.oid.clone(),
            expected_version: start,
            blocked_by_gap: start > 1,
            deleted: false,
            winner: None,
        }
    }

    fn push(&mut self, record: SpillRecord, current_timestamp_ns: i64) {
        if is_expired(&record.record, current_timestamp_ns) {
            self.deleted = true;
            self.winner = None;
            return;
        }
        if self.deleted || self.blocked_by_gap {
            return;
        }
        let version = effective_version(&record.record);
        if version == self.expected_version {
            self.winner = Some(record);
            self.expected_version = self.expected_version.saturating_add(1);
        } else if version > self.expected_version {
            // Match MvccResolver: retain the last contiguous winner and ignore
            // every version after the first gap.
            self.blocked_by_gap = true;
        }
        // version < expected is a later duplicate of an already selected
        // version, ordered after it by timestamp/ordinal, so it is ignored.
    }

    fn finish(self) -> Option<SpillRecord> {
        if self.deleted { None } else { self.winner }
    }
}

fn write_mvcc_winners(
    paths: &[PathBuf],
    output: &Path,
    current_timestamp_ns: i64,
) -> Result<(u64, u64)> {
    let (mut readers, mut heap) = open_merge_heap(paths)?;
    let mut writer = FramedRunWriter::create(output)?;
    let mut group: Option<VersionGroup> = None;
    let mut output_records = 0u64;

    while let Some(entry) = heap.pop() {
        let run_index = entry.run_index;
        let record = entry.record;
        let is_new_group = group
            .as_ref()
            .is_some_and(|current| current.oid != record.record.oid);
        if is_new_group && let Some(winner) = group.take().and_then(VersionGroup::finish) {
            writer.write_record(&winner)?;
            output_records = output_records.saturating_add(1);
        }

        if is_append_only_oid(&record.record.oid) {
            if !is_expired(&record.record, current_timestamp_ns) {
                writer.write_record(&record)?;
                output_records = output_records.saturating_add(1);
            }
        } else {
            if group.is_none() {
                group = Some(VersionGroup::begin(&record));
            }
            if let Some(current) = group.as_mut() {
                current.push(record, current_timestamp_ns);
            }
        }
        advance_run(&mut readers, &mut heap, run_index)?;
    }

    if let Some(winner) = group.and_then(VersionGroup::finish) {
        writer.write_record(&winner)?;
        output_records = output_records.saturating_add(1);
    }
    Ok((writer.finish()?, output_records))
}

fn is_expired(record: &ProximaRecord, current_timestamp_ns: i64) -> bool {
    record
        .valid_to_ns
        .is_some_and(|valid_to| valid_to < current_timestamp_ns)
}

struct FramedRunWriter {
    writer: BufWriter<File>,
    bytes_written: u64,
}

impl FramedRunWriter {
    fn create(path: &Path) -> Result<Self> {
        let file = OpenOptions::new().create_new(true).write(true).open(path)?;
        let mut writer = BufWriter::new(file);
        writer.write_all(RUN_MAGIC)?;
        writer.write_all(&RUN_FORMAT_VERSION.to_le_bytes())?;
        Ok(Self {
            writer,
            bytes_written: RUN_MAGIC.len() as u64 + 2,
        })
    }

    fn write_record(&mut self, record: &SpillRecord) -> Result<()> {
        let payload = bincode::serialize(record)?;
        let length = u32::try_from(payload.len()).map_err(|_| {
            SstError::Compaction(format!(
                "spill record frame exceeds u32 length: {} bytes",
                payload.len()
            ))
        })?;
        let checksum = crc32fast::hash(&payload);
        self.writer.write_all(&length.to_le_bytes())?;
        self.writer.write_all(&checksum.to_le_bytes())?;
        self.writer.write_all(&payload)?;
        self.bytes_written = self
            .bytes_written
            .saturating_add(FRAME_HEADER_BYTES)
            .saturating_add(payload.len() as u64);
        Ok(())
    }

    fn finish(mut self) -> Result<u64> {
        self.writer.flush()?;
        self.writer.get_ref().sync_data()?;
        Ok(self.bytes_written)
    }
}

struct FramedRunReader {
    reader: BufReader<File>,
}

impl FramedRunReader {
    fn open(path: &Path) -> Result<Self> {
        let mut reader = BufReader::new(File::open(path)?);
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic).map_err(|error| {
            SstError::Compaction(format!(
                "truncated spill run header {}: {error}",
                path.display()
            ))
        })?;
        if &magic != RUN_MAGIC {
            return Err(SstError::Compaction(format!(
                "invalid spill run magic in {}",
                path.display()
            )));
        }
        let mut version = [0u8; 2];
        reader.read_exact(&mut version).map_err(|error| {
            SstError::Compaction(format!(
                "truncated spill run version {}: {error}",
                path.display()
            ))
        })?;
        let version = u16::from_le_bytes(version);
        if version != RUN_FORMAT_VERSION {
            return Err(SstError::Compaction(format!(
                "unsupported spill run version {version} in {}",
                path.display()
            )));
        }
        Ok(Self { reader })
    }

    fn read_next(&mut self) -> Result<Option<SpillRecord>> {
        let mut header = [0u8; 8];
        let read = self.reader.read(&mut header[..1])?;
        if read == 0 {
            return Ok(None);
        }
        self.reader.read_exact(&mut header[1..]).map_err(|error| {
            SstError::Compaction(format!("truncated spill frame header: {error}"))
        })?;
        let length = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as u64;
        if length > MAX_FRAME_BYTES {
            return Err(SstError::Compaction(format!(
                "spill frame length {length} exceeds {MAX_FRAME_BYTES} byte safety limit"
            )));
        }
        let expected_checksum = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let mut payload = vec![0u8; length as usize];
        self.reader.read_exact(&mut payload).map_err(|error| {
            SstError::Compaction(format!("truncated spill frame payload: {error}"))
        })?;
        let actual_checksum = crc32fast::hash(&payload);
        if actual_checksum != expected_checksum {
            return Err(SstError::Compaction(format!(
                "spill frame checksum mismatch: expected {expected_checksum:#010x}, got {actual_checksum:#010x}"
            )));
        }
        let record = bincode::deserialize::<SpillRecord>(&payload)?.restore_schema_version();
        Ok(Some(record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::mvcc_resolution::MvccResolver;
    use proximadb_block_format::{BlockCompression, BlockMode, VectorQuant};
    use proximadb_records::{EmbeddingCell, EmbeddingValues, ProximaRecord};
    use proximadb_storage_common::pax_block::PaxSegmentWriter;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    fn record(oid: &str, version: u64, created_at_ns: i64) -> ProximaRecord {
        ProximaRecord {
            oid: oid.to_string(),
            record_version: version,
            created_at_ns,
            updated_at_ns: created_at_ns,
            ..ProximaRecord::default()
        }
    }

    fn spill(records: Vec<ProximaRecord>) -> Vec<SpillRecord> {
        records
            .into_iter()
            .enumerate()
            .map(|(ordinal, record)| SpillRecord::new(ordinal as u64, record))
            .collect()
    }

    fn vector_record(oid: &str, seed: usize) -> ProximaRecord {
        let values = (0..8)
            .map(|dimension| (seed * 17 + dimension * 3) as f32 * 0.01)
            .collect::<Vec<_>>();
        ProximaRecord {
            oid: oid.to_string(),
            created_at_ns: seed as i64 + 1,
            updated_at_ns: seed as i64 + 1,
            embeddings: vec![EmbeddingCell {
                model_id: "model_0".to_string(),
                modality: "dense".to_string(),
                dim: values.len() as u32,
                values: EmbeddingValues::Fp32(values),
                ..EmbeddingCell::default()
            }],
            ..ProximaRecord::default()
        }
    }

    struct InMemoryRangeSource {
        bytes: Vec<u8>,
        range_reads: AtomicU64,
        whole_reads: AtomicU64,
    }

    #[async_trait]
    impl CompactionRangeSource for InMemoryRangeSource {
        async fn size(&self, _path: &str) -> Result<u64> {
            Ok(self.bytes.len() as u64)
        }

        async fn read_range(&self, _path: &str, offset: u64, length: u64) -> Result<Vec<u8>> {
            self.range_reads.fetch_add(1, AtomicOrdering::Relaxed);
            let start = usize::try_from(offset)
                .map_err(|_| SstError::Storage("test range offset exceeds usize".to_string()))?;
            let requested_end = offset.saturating_add(length);
            let end = usize::try_from(requested_end)
                .unwrap_or(usize::MAX)
                .min(self.bytes.len());
            Ok(self.bytes.get(start..end).unwrap_or_default().to_vec())
        }

        async fn read_all(&self, _path: &str) -> Result<Vec<u8>> {
            self.whole_reads.fetch_add(1, AtomicOrdering::Relaxed);
            Ok(self.bytes.clone())
        }
    }

    #[test]
    fn external_mvcc_matches_canonical_resolver_across_multiple_runs() {
        let now_ns = 1_000;
        let mut expired = record("deleted", 1, 1);
        expired.valid_to_ns = Some(now_ns - 1);
        let records = vec![
            record("a", 2, 20),
            record("gap", 3, 30),
            record("a", 1, 10),
            record("a", 2, 21),
            expired,
            record("", 1, 40),
            record("", 1, 41),
            record("b", 0, 50),
        ];
        let expected =
            MvccResolver::with_timestamp_ns(now_ns).resolve_sorted_batch(records.clone());
        let scratch = tempfile::tempdir().expect("scratch tempdir");

        let output = resolve_external_mvcc(
            scratch.path(),
            spill(records).into_iter().map(Ok),
            600,
            2,
            now_ns,
        )
        .expect("external MVCC");
        let actual: Vec<_> = output
            .read_records()
            .expect("read output")
            .into_iter()
            .map(|item| item.record)
            .collect();

        assert_eq!(actual, expected);
        assert!(output.stats().initial_run_count > 1);
        assert!(output.stats().merge_pass_count > 0);
        assert!(output.stats().max_open_runs <= 2);
    }

    #[test]
    fn output_is_byte_deterministic_across_run_sizes_and_fan_in() {
        let records = (0..40)
            .rev()
            .map(|i| record(&format!("oid-{i:03}"), 1, i))
            .collect::<Vec<_>>();
        let scratch_a = tempfile::tempdir().expect("scratch a");
        let scratch_b = tempfile::tempdir().expect("scratch b");

        let a = resolve_external_mvcc(
            scratch_a.path(),
            spill(records.clone()).into_iter().map(Ok),
            700,
            2,
            10_000,
        )
        .expect("external MVCC a");
        let b = resolve_external_mvcc(
            scratch_b.path(),
            spill(records).into_iter().map(Ok),
            2_000,
            8,
            10_000,
        )
        .expect("external MVCC b");

        assert_eq!(
            std::fs::read(a.output_path()).expect("read a"),
            std::fs::read(b.output_path()).expect("read b")
        );
    }

    #[test]
    fn checksum_corruption_fails_closed() {
        let scratch = tempfile::tempdir().expect("scratch tempdir");
        let output = resolve_external_mvcc(
            scratch.path(),
            spill(vec![record("a", 1, 1)]).into_iter().map(Ok),
            1_024,
            2,
            10_000,
        )
        .expect("external MVCC");
        let mut bytes = std::fs::read(output.output_path()).expect("read run");
        let last = bytes.len().checked_sub(1).expect("nonempty run");
        bytes[last] ^= 0xff;
        std::fs::write(output.output_path(), bytes).expect("corrupt run");

        let error = output.read_records().expect_err("corruption must fail");
        assert!(error.to_string().contains("checksum"));
    }

    #[test]
    fn output_drop_reclaims_task_scratch() {
        let scratch = tempfile::tempdir().expect("scratch tempdir");
        let task_path = {
            let output = resolve_external_mvcc(
                scratch.path(),
                spill(vec![record("a", 1, 1)]).into_iter().map(Ok),
                1_024,
                2,
                10_000,
            )
            .expect("external MVCC");
            output.task_path().to_path_buf()
        };
        assert!(!task_path.exists());
    }

    #[tokio::test]
    async fn coalesced_inputs_are_ranged_by_block_and_preserve_mvcc_vectors() {
        let segment_dir = tempfile::tempdir().expect("segment tempdir");
        let segment_path = segment_dir.path().join("input.pax");
        let records = (0..64)
            .rev()
            .map(|i| vector_record(&format!("oid-{i:03}"), i))
            .collect::<Vec<_>>();
        let mut writer = PaxSegmentWriter::new(
            &segment_path,
            BlockMode::Pax,
            BlockCompression::Lz4,
            "collection",
            0,
            1,
            Some(512),
        )
        .with_quant(VectorQuant::RaBitQ)
        .with_rerank_quant(VectorQuant::Sq8)
        .with_coalesced_rabitq(true);
        for record in &records {
            writer.add_record(record).expect("add record");
        }
        writer.finish().expect("finish segment");
        let bytes = std::fs::read(&segment_path).expect("read segment");
        let source = InMemoryRangeSource {
            bytes: bytes.clone(),
            range_reads: AtomicU64::new(0),
            whole_reads: AtomicU64::new(0),
        };
        let scratch = tempfile::tempdir().expect("scratch tempdir");

        let (output, input_stats) = resolve_external_mvcc_from_segments(
            &source,
            &["az://container/input.pax".to_string()],
            scratch.path(),
            2_048,
            3,
            10_000,
            &["model_0".to_string()],
            &[],
            None,
        )
        .await
        .expect("ranged external MVCC");
        let actual = output
            .read_records()
            .expect("read winners")
            .into_iter()
            .map(|record| record.record)
            .collect::<Vec<_>>();

        assert_eq!(actual.len(), records.len());
        assert!(actual.windows(2).all(|pair| pair[0].oid < pair[1].oid));
        assert!(actual.iter().all(|record| {
            record
                .embeddings
                .first()
                .is_some_and(|embedding| embedding.dim == 8)
        }));
        assert_eq!(input_stats.coalesced_files, 1);
        assert_eq!(input_stats.legacy_whole_file_fallbacks, 0);
        assert!(input_stats.block_range_reads > 1);
        assert_eq!(
            source.whole_reads.load(AtomicOrdering::Relaxed),
            0,
            "coalesced compaction must never whole-object decode"
        );
        assert!(source.range_reads.load(AtomicOrdering::Relaxed) > 4);
        assert!(input_stats.largest_range_bytes < bytes.len() as u64);
    }
}
