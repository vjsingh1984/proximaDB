//! Deterministic, bounded local-scratch primitives for canonical compaction.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use proximadb_records::ProximaRecord;
use serde::{Deserialize, Serialize};

use crate::core::search::mvcc_resolution::{effective_version, is_append_only_oid};
use crate::storage::engines::sst::error::{Result, SstError};

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

    let task_directory = tempfile::Builder::new()
        .prefix("proximadb-compaction-spill-")
        .tempdir_in(scratch_root)?;
    let mut stats = ExternalMvccStats::default();
    let mut runs = Vec::new();
    let mut buffer = Vec::new();
    let mut buffered_bytes = 0u64;

    for record in records {
        let record = record?;
        let record_bytes = estimated_buffer_bytes(&record)?;
        if !buffer.is_empty() && buffered_bytes.saturating_add(record_bytes) > max_run_buffer_bytes
        {
            let path = task_directory
                .path()
                .join(format!("oid-pass-0000-run-{:06}.pxrun", runs.len()));
            stats.scratch_bytes_written = stats
                .scratch_bytes_written
                .saturating_add(write_sorted_run(&path, &mut buffer)?);
            runs.push(path);
            buffered_bytes = 0;
        }
        buffered_bytes = buffered_bytes.saturating_add(record_bytes);
        stats.peak_run_buffer_bytes = stats.peak_run_buffer_bytes.max(buffered_bytes);
        stats.input_records = stats.input_records.saturating_add(1);
        buffer.push(record);
    }

    if !buffer.is_empty() {
        let path = task_directory
            .path()
            .join(format!("oid-pass-0000-run-{:06}.pxrun", runs.len()));
        stats.scratch_bytes_written = stats
            .scratch_bytes_written
            .saturating_add(write_sorted_run(&path, &mut buffer)?);
        runs.push(path);
    }
    stats.initial_run_count = runs.len();

    let mut pass = 0usize;
    while runs.len() > max_merge_fan_in {
        pass = pass.saturating_add(1);
        let mut next_runs = Vec::with_capacity(runs.len().div_ceil(max_merge_fan_in));
        for (group_index, group) in runs.chunks(max_merge_fan_in).enumerate() {
            let output = task_directory
                .path()
                .join(format!("oid-pass-{pass:04}-run-{group_index:06}.pxrun"));
            stats.max_open_runs = stats.max_open_runs.max(group.len());
            stats.scratch_bytes_written = stats
                .scratch_bytes_written
                .saturating_add(merge_sorted_runs(group, &output)?);
            next_runs.push(output);
        }
        for old_run in &runs {
            std::fs::remove_file(old_run)?;
        }
        runs = next_runs;
    }
    stats.merge_pass_count = pass;

    let output_path = task_directory.path().join("mvcc-winners.pxrun");
    stats.max_open_runs = stats.max_open_runs.max(runs.len());
    let (bytes_written, output_records) =
        write_mvcc_winners(&runs, &output_path, current_timestamp_ns)?;
    stats.scratch_bytes_written = stats.scratch_bytes_written.saturating_add(bytes_written);
    stats.output_records = output_records;
    for old_run in &runs {
        std::fs::remove_file(old_run)?;
    }

    Ok(ExternalMvccOutput {
        task_directory,
        output_path,
        stats,
    })
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
    use proximadb_records::ProximaRecord;

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
}
