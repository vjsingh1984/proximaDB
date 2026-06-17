//! Vector base-XOR codec profiling for PCX-005.
//!
//! Run with:
//! `cargo bench --bench bench_23_codec_vector_base_xor`

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use proximadb_codec::{
    AccessTemperature, AuthorityMode, ColumnModality, CompressionBenchmarkRecord,
    CompressionProfile, CompressionStatsProfile, LayoutHints, LossPolicy, PhysicalOrdering,
    StorageSpecialization, WorkloadProfile,
    functions::{
        vector_base_xor_decode_f32_vectors, vector_base_xor_encode_f32_vectors_with_profile,
        vector_base_xor_profile_f32_vectors,
    },
};
use std::time::Duration;

const ROWS: usize = 1024;
const DIMENSION: usize = 384;
const MIN_SELECTION_RATIO: f64 = 1.10;

#[derive(Debug)]
struct VectorDataset {
    name: &'static str,
    rows: Vec<Vec<f32>>,
}

impl VectorDataset {
    fn refs(&self) -> Vec<&[f32]> {
        self.rows.iter().map(Vec::as_slice).collect()
    }
}

fn bench_vector_base_xor(c: &mut Criterion) {
    let datasets = representative_datasets();

    eprintln!("vector base-XOR profile matrix:");
    for dataset in &datasets {
        let refs = dataset.refs();
        let profile = vector_base_xor_profile_f32_vectors(&refs).expect("profile should succeed");
        eprintln!(
            "  {} rows={} dim={} raw={} encoded={} ratio={:.3} bytes/value={:.3} zero_words={} literal_words={} fallback={}",
            dataset.name,
            profile.rows,
            profile.dimension,
            profile.raw_bytes,
            profile.encoded_bytes,
            profile.compression_ratio,
            profile.bytes_per_value,
            profile.zero_words,
            profile.literal_words,
            !profile.should_use(MIN_SELECTION_RATIO)
        );
        let record = CompressionBenchmarkRecord::new(
            "bench_23_codec_vector_base_xor",
            dataset.name,
            CompressionStatsProfile::from_measured_codec(
                format!("bench_23_codec_vector_base_xor/{}", dataset.name),
                vector_exact_profile(),
                vector_spatial_layout(profile.dimension),
                "VectorBaseXorEntropy",
                true,
                profile.raw_bytes as u64,
                profile.encoded_bytes as u64,
                (profile.rows * profile.dimension) as u64,
            ),
        );
        eprintln!(
            "PCX_PROFILE_JSON {}",
            serde_json::to_string(&record).expect("profile JSON should serialize")
        );
    }

    let mut group = c.benchmark_group("codec_vector_base_xor");
    group.measurement_time(Duration::from_secs(3));
    group.warm_up_time(Duration::from_secs(1));

    for dataset in &datasets {
        let refs = dataset.refs();
        let raw = raw_encode_vectors(&refs);
        let (encoded, profile) =
            vector_base_xor_encode_f32_vectors_with_profile(&refs).expect("encode should succeed");

        group.throughput(Throughput::Bytes(profile.raw_bytes as u64));
        group.bench_with_input(
            BenchmarkId::new("raw_decode", dataset.name),
            &raw,
            |b, raw| {
                b.iter(|| {
                    let decoded = raw_decode_vectors(black_box(raw), ROWS, DIMENSION)
                        .expect("raw decode should succeed");
                    black_box(decoded);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("base_xor_encode", dataset.name),
            &refs,
            |b, refs| {
                b.iter(|| {
                    let (encoded, profile) =
                        vector_base_xor_encode_f32_vectors_with_profile(black_box(refs))
                            .expect("base-XOR encode should succeed");
                    black_box((encoded, profile));
                });
            },
        );

        group.throughput(Throughput::Bytes(encoded.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("base_xor_decode", dataset.name),
            &encoded,
            |b, encoded| {
                b.iter(|| {
                    let decoded = vector_base_xor_decode_f32_vectors(black_box(encoded))
                        .expect("base-XOR decode should succeed");
                    black_box(decoded);
                });
            },
        );

        black_box(profile);
    }

    group.finish();
}

fn representative_datasets() -> Vec<VectorDataset> {
    vec![
        VectorDataset {
            name: "co_located_identical",
            rows: co_located_identical_rows(ROWS, DIMENSION),
        },
        VectorDataset {
            name: "co_located_sparse_drift",
            rows: co_located_sparse_drift_rows(ROWS, DIMENSION),
        },
        VectorDataset {
            name: "normalized_random_fallback",
            rows: normalized_random_rows(ROWS, DIMENSION),
        },
    ]
}

fn vector_exact_profile() -> CompressionProfile {
    CompressionProfile {
        authority_mode: AuthorityMode::CanonicalRecord,
        loss_policy: LossPolicy::ExactOnly,
        workload_profile: WorkloadProfile::AnnRerank,
        storage_specialization: StorageSpecialization::VectorExact,
        hotness: AccessTemperature::Warm,
        target_compression_ratio: Some(MIN_SELECTION_RATIO as f32),
        ..CompressionProfile::default()
    }
}

fn vector_spatial_layout(dimension: usize) -> LayoutHints {
    let mut hints = LayoutHints::vector_spatial();
    hints.modality = ColumnModality::VectorExact;
    hints.physical_ordering = PhysicalOrdering::VectorSpatial;
    if let Some(vector_layout) = hints.vector_layout.as_mut() {
        vector_layout.dimension = u16::try_from(dimension).ok();
    }
    hints
}

fn co_located_identical_rows(rows: usize, dimension: usize) -> Vec<Vec<f32>> {
    let base = normalized_base_vector(dimension);
    vec![base; rows]
}

fn co_located_sparse_drift_rows(rows: usize, dimension: usize) -> Vec<Vec<f32>> {
    let base = normalized_base_vector(dimension);
    let mut vectors = Vec::with_capacity(rows);
    vectors.push(base.clone());
    for row_idx in 1..rows {
        let mut row = base.clone();
        for lane in 0..6 {
            let dim_idx = (row_idx * 31 + lane * 53) % dimension;
            let bits = row[dim_idx].to_bits();
            row[dim_idx] = f32::from_bits(bits ^ ((lane as u32 + 1) << 7));
        }
        vectors.push(row);
    }
    vectors
}

fn normalized_random_rows(rows: usize, dimension: usize) -> Vec<Vec<f32>> {
    let mut state = 0x85eb_ca6bu32;
    let mut vectors = Vec::with_capacity(rows);
    for _ in 0..rows {
        let mut row = Vec::with_capacity(dimension);
        for _ in 0..dimension {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let value = ((state >> 8) as f32 / 0x00ff_ffff as f32) * 2.0 - 1.0;
            row.push(value);
        }
        normalize(&mut row);
        vectors.push(row);
    }
    vectors
}

fn normalized_base_vector(dimension: usize) -> Vec<f32> {
    let mut row: Vec<f32> = (0..dimension)
        .map(|idx| ((idx % 97) as f32 / 96.0) * 2.0 - 1.0)
        .collect();
    normalize(&mut row);
    row
}

fn normalize(row: &mut [f32]) {
    let norm = row.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in row {
            *value /= norm;
        }
    }
}

fn raw_encode_vectors(rows: &[&[f32]]) -> Vec<u8> {
    let total_values = rows.iter().map(|row| row.len()).sum::<usize>();
    let mut encoded = Vec::with_capacity(total_values * 4);
    for row in rows {
        for value in *row {
            encoded.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    }
    encoded
}

fn raw_decode_vectors(data: &[u8], rows: usize, dimension: usize) -> anyhow::Result<Vec<Vec<f32>>> {
    let expected_len = rows
        .checked_mul(dimension)
        .and_then(|values| values.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("raw vector decode byte count overflow"))?;
    if data.len() != expected_len {
        anyhow::bail!(
            "raw vector decode expected {expected_len} bytes, got {}",
            data.len()
        );
    }

    let mut decoded = Vec::with_capacity(rows);
    let mut pos = 0usize;
    for _ in 0..rows {
        let mut row = Vec::with_capacity(dimension);
        for _ in 0..dimension {
            let bits = u32::from_le_bytes(data[pos..pos + 4].try_into()?);
            row.push(f32::from_bits(bits));
            pos += 4;
        }
        decoded.push(row);
    }
    Ok(decoded)
}

criterion_group!(benches, bench_vector_base_xor);
criterion_main!(benches);
