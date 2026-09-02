//! Asymptotic ratchet: IVF ingest must not slow down as clusters grow.
//!
//! Companion to `graph_ingest_asymptotic_integration_test.rs`, guarding the
//! same defect family on the vector-index ingest path: per-item work
//! proportional to resident size. The historical shape here was the posting
//! list read-modify-write in `UnifiedIvfIndex::add_vector` — every insert
//! deep-cloned the whole `TieredPostingList` (including the inline
//! `Vec<Vec<f32>>` holding up to 1000 vectors) and re-inserted it, so
//! building a cluster of C vectors cost O(C²·dim) float copies. Every
//! fixed-size test passed; only the batch-time SHAPE exposes it.
//!
//! This test measures the shape, not a wall-clock number: it ingests equal
//! batches and compares the LAST batches against the FIRST. Amortized-
//! constant per-add cost measures ~1 regardless of machine speed — both
//! halves run in the same process moments apart, so the comparison
//! self-normalizes and survives noisy CI runners. The defect it blocks
//! grows with cluster size (≈10× at these parameters in a dev build).
//!
//! If this test starts failing, some per-add path has re-acquired a term
//! proportional to cluster size. Do not widen the threshold; find the term.

use std::time::Instant;

use proximadb::index::axis::{AxisIvfConfig, AxisIvfIndex};

const COLLECTION: &str = "ivf_asymptotic_ratchet";
const DIM: usize = 64;
const N_CLUSTERS: usize = 8;
const BATCH: usize = 200;
const BATCHES: usize = 20; // 4000 adds → ~500/cluster, under the 1000-vectors
// inline cutoff where the cloned field is largest.
/// Late/early median ratio allowed. Amortized-constant ingest measures ~1;
/// the posting-list clone this blocks measures ~10 at these sizes. Generous
/// for CI noise and amortized map growth without letting a size-proportional
/// per-add term through.
const MAX_RATIO: f64 = 4.0;

/// Deterministic xorshift64* PRNG — no `rand` dependency, reproducible
/// everywhere.
struct Lcg(u64);

impl Lcg {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn vector(&mut self) -> Vec<f32> {
        (0..DIM)
            .map(|_| (self.next_u64() % 10_000) as f32 / 100.0)
            .collect()
    }
}

async fn batch_times() -> Vec<f64> {
    let mut config = AxisIvfConfig::default();
    config.dimension = DIM;
    config.n_clusters = N_CLUSTERS;
    config.n_probe = 4;
    config.use_binary = false; // keep the measured path purely fp32/inline
    config.use_pq = false;
    config.train_on_insert = false; // retraining mid-test would dominate timing
    config.min_train_size = usize::MAX;

    let mut index =
        AxisIvfIndex::new(COLLECTION.to_string(), config).expect("construct UnifiedIvfIndex");

    // Train on a spread of vectors so all N_CLUSTERS centroids form.
    let mut rng = Lcg(0x9E3779B97F4A7C15);
    let training: Vec<Vec<f32>> = (0..256).map(|_| rng.vector()).collect();
    index.train(training).await.expect("train index");

    let mut times = Vec::with_capacity(BATCHES);
    let mut counter: u64 = 0;
    for _ in 0..BATCHES {
        // Pre-generate the batch so generation cost is outside the timer.
        let batch: Vec<(String, Vec<f32>)> = (0..BATCH)
            .map(|_| {
                counter += 1;
                (format!("v{counter}"), rng.vector())
            })
            .collect();

        let start = Instant::now();
        for (id, vector) in batch {
            index
                .add_vector(id, vector, None)
                .await
                .expect("add_vector");
        }
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times
}

fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted[sorted.len() / 2]
}

#[tokio::test]
async fn ivf_ingest_does_not_slow_down_as_clusters_grow() {
    let times = batch_times().await;

    // The first add proves the index was trained (add_vector rejects
    // untrained indexes), so a non-empty timing vector means all adds ran.
    assert_eq!(times.len(), BATCHES);
    assert!(times.iter().all(|t| *t > 0.0));

    let early = median(&times[..3]);
    let late = median(&times[BATCHES - 3..]);
    // 1 ms floor absorbs scheduler jitter when batches are sub-millisecond.
    let ratio = late / early.max(1.0);

    eprintln!(
        "ivf ingest batch times (ms): first3 median {early:.3}, last3 median {late:.3}, ratio {ratio:.2}"
    );

    assert!(
        ratio <= MAX_RATIO,
        "IVF ingest slowed down as clusters grew: last/first batch-time ratio \
         {ratio:.2} > {MAX_RATIO}. Some per-add path re-acquired work \
         proportional to cluster size (see the posting-list read-modify-write \
         history in add_vector). Batch times (ms): {times:.3?}"
    );
}
