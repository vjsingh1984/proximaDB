//! Vector ANN recall@k over the native REST v2 surface (ANN-Benchmarks style).
//!
//! Spins up an in-process ProximaDB, creates a vector collection, inserts a
//! deterministic dataset, then for a set of query vectors compares the engine's
//! k-NN results against BRUTE-FORCE cosine ground truth computed in-test. The
//! metric is mean recall@k — the standard ANN-Benchmarks quality measure.
//!
//! This is the vector modality's qa-gate conformance harness; it ratchets on
//! mean recall (no-regression). Modeled on tests/filtered_ann_recall_bands.rs
//! but UNFILTERED (the filter-eval path is Beta in v0.2) so it exercises the
//! core ANN quality path only.
//!
//!   cargo test --test vector_ann_recall_e2e -- --nocapture

use std::collections::HashSet;
use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::time::sleep;

/// Mean recall@k floor (ratchet). Measured mean is 1.000 at this scale (small N
/// is exact/brute-force-served); 0.90 leaves headroom for cross-platform float
/// drift on borderline neighbors while still catching real index regressions.
const RECALL_RATCHET: f64 = 0.90;

const DIM: usize = 32;
const N: usize = 300;
const TOP_K: usize = 10;
const N_QUERIES: usize = 20;

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

struct VecServer {
    rest_port: u16,
    db: Option<ProximaDB>,
    _tmp: TempDir,
}

impl VecServer {
    async fn start() -> anyhow::Result<Self> {
        let rest_port = free_port();
        let grpc_port = free_port();
        let flight_port = free_port();
        let pg_port = free_port();
        let tmp = TempDir::new()?;
        let mut config = Config::default();
        config.server.bind_address = "127.0.0.1".to_string();
        config.server.port = rest_port;
        config.server.data_dir = tmp.path().to_path_buf();
        config.api.rest_port = rest_port;
        config.api.grpc_port = grpc_port;
        config.api.arrow_flight_port = flight_port;
        config.api.pg_port = Some(pg_port);
        config.api.unified_mode = false;
        config.storage.storage_locations = vec![proximadb::core::config::StorageLocation {
            url: format!("file://{}", tmp.path().display()),
            ..Default::default()
        }];
        config.storage.wal_config.write_buffer_directory =
            format!("file://{}/wal", tmp.path().display());
        let mut db = ProximaDB::new(config).await?;
        db.start().await?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()?;
        let health = format!("http://127.0.0.1:{rest_port}/health");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            match http.get(&health).send().await {
                Ok(r) if r.status().is_success() => break,
                _ if std::time::Instant::now() > deadline => anyhow::bail!("REST not ready"),
                _ => sleep(Duration::from_millis(100)).await,
            }
        }
        Ok(Self {
            rest_port,
            db: Some(db),
            _tmp: tmp,
        })
    }
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.rest_port)
    }
}

impl Drop for VecServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

/// Deterministic pseudo-random vector (splitmix64-seeded) — distinct directions
/// so brute-force top-k is a real ranking, stable across runs without a PRNG dep.
fn gen_vec(seed: u64, dim: usize) -> Vec<f32> {
    let mut s = seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1);
    (0..dim)
        .map(|_| {
            s ^= s >> 30;
            s = s.wrapping_mul(0xBF58476D1CE4E5B9);
            s ^= s >> 27;
            s = s.wrapping_mul(0x94D049BB133111EB);
            s ^= s >> 31;
            // map to [-1, 1)
            ((s >> 11) as f32 / (1u64 << 53) as f32) * 2.0 - 1.0
        })
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

fn brute_force_topk(corpus: &[(String, Vec<f32>)], query: &[f32], k: usize) -> Vec<String> {
    let mut scored: Vec<(&String, f32)> = corpus
        .iter()
        .map(|(id, v)| (id, cosine(v, query)))
        .collect();
    // Descending cosine (higher = closer).
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(k)
        .map(|(id, _)| id.clone())
        .collect()
}

fn ids_from_search_body(body: &Value) -> Vec<String> {
    body.get("results")
        .or_else(|| body.get("matches"))
        .or_else(|| body.get("hits"))
        .or_else(|| body.get("records"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    m.get("id")
                        .or_else(|| m.get("record_id"))
                        .and_then(|s| s.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vector_ann_recall_at_k() {
    let server = VecServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();
    let base = server.base();

    let coll = format!(
        "ann_recall_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    // 1. Create vector collection (cosine / fp32 / SST).
    let create = http
        .post(format!("{base}/api/v2/collections"))
        .json(&json!({
            "name": coll,
            "dimension": DIM,
            "engine": "sst",
            "distance_metric": "cosine",
            "canonical_embedding_precision": "fp32",
            "enable_proxima_record": false,
        }))
        .send()
        .await
        .expect("create send");
    assert!(
        create.status().is_success(),
        "create: {} {}",
        create.status(),
        create.text().await.unwrap_or_default()
    );

    // 2. Build the corpus and insert it.
    let corpus: Vec<(String, Vec<f32>)> = (0..N)
        .map(|i| (format!("rec-{i}"), gen_vec(i as u64, DIM)))
        .collect();
    let records: Vec<Value> = corpus
        .iter()
        .map(|(id, v)| json!({ "id": id, "vector": v }))
        .collect();
    let insert = http
        .post(format!("{base}/api/v2/collections/{coll}/records/batch"))
        .json(&json!({ "records": records }))
        .send()
        .await
        .expect("insert send");
    assert!(
        insert.status().is_success(),
        "insert: {} {}",
        insert.status(),
        insert.text().await.unwrap_or_default()
    );
    sleep(Duration::from_millis(750)).await;

    // 3. For each query (an existing vector + small perturbation), compare the
    //    engine's k-NN to brute-force cosine ground truth.
    let mut recalls = Vec::new();
    let mut empty_results = 0;
    for q in 0..N_QUERIES {
        let base_idx = (q * (N / N_QUERIES)) % N;
        let mut query = corpus[base_idx].1.clone();
        // Small perturbation so the query isn't byte-identical to a stored vector.
        let noise = gen_vec((q as u64).wrapping_add(1_000_000), DIM);
        for j in 0..DIM {
            query[j] += noise[j] * 0.01;
        }

        let resp = http
            .post(format!("{base}/api/v2/collections/{coll}/search"))
            .json(&json!({ "vector": query, "top_k": TOP_K }))
            .send()
            .await
            .expect("search send");
        assert!(
            resp.status().is_success(),
            "search: {} {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
        let body: Value = resp.json().await.expect("search json");
        let ann = ids_from_search_body(&body);
        if ann.is_empty() {
            empty_results += 1;
        }
        let exact = brute_force_topk(&corpus, &query, TOP_K);
        let exact_set: HashSet<&String> = exact.iter().collect();
        let hits = ann.iter().filter(|id| exact_set.contains(id)).count();
        recalls.push(hits as f64 / TOP_K as f64);
    }

    let mean = recalls.iter().sum::<f64>() / recalls.len() as f64;
    eprintln!(
        "=== Vector ANN recall@{TOP_K}: mean={mean:.3} over {N_QUERIES} queries (N={N}, dim={DIM}); ratchet {RECALL_RATCHET} ==="
    );

    assert_eq!(
        empty_results, 0,
        "{empty_results}/{N_QUERIES} searches returned ZERO results — insert→search index wiring is broken"
    );
    assert!(
        mean >= RECALL_RATCHET,
        "Vector ANN recall@{TOP_K} regressed: mean {mean:.3} < ratchet {RECALL_RATCHET}"
    );
}
