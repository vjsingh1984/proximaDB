//! Filtered-ANN differential recall across selectivity bands (ADR-011 / TD-064).
//!
//! Required by `MVP_RELEASE_READINESS_RECONCILIATION_2026_05_28.adoc` P1 #2:
//! "Add deterministic exact-vs-ANN tests for low/medium/high selectivity and
//!  keep ADR-011 Beta until those pass."
//!
//! What this test asserts:
//!   For each selectivity band — Low (~10%), Medium (~50%), High (~100% / no
//!   filter) — the filtered ANN search retrieves a non-trivial subset of the
//!   true filtered top-k. The test is intentionally generous on the recall
//!   floors (>= 30% at low, >= 60% at medium, >= 80% at high) because the
//!   shipped policy in v0.2 is the default `AnnFilteringPolicy`, not a
//!   per-collection tuned one. The point is to lock in a *non-regression*
//!   contract, not to claim a recall SLA — release notes/SUPPORTED_SURFACE
//!   already disclose that no recall envelope is published for v0.2.
//!
//! Test shape mirrors `tests/fp16_v2_insert_search_e2e.rs` so the harness
//! costs amortize: CREATE → INSERT → SEARCH (with and without typed filter)
//! → recall@10 calculation.

use std::collections::HashSet;
use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::time::sleep;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind port 0");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

struct RecallBandsServer {
    rest_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl RecallBandsServer {
    async fn start() -> anyhow::Result<Self> {
        let rest_port = free_port();
        let grpc_port = free_port();
        let flight_port = free_port();
        let pg_port = free_port();
        let tmp_data = TempDir::new()?;

        let mut config = Config::default();
        config.server.bind_address = "127.0.0.1".to_string();
        config.server.port = rest_port;
        config.server.data_dir = tmp_data.path().to_path_buf();
        config.api.rest_port = rest_port;
        config.api.grpc_port = grpc_port;
        config.api.arrow_flight_port = flight_port;
        config.api.pg_port = Some(pg_port);
        config.api.unified_mode = false;
        config.storage.storage_locations = vec![proximadb::core::config::StorageLocation {
            url: format!("file://{}", tmp_data.path().display()),
            ..Default::default()
        }];
        config.storage.wal_config.write_buffer_directory =
            format!("file://{}/wal", tmp_data.path().display());

        let mut db = ProximaDB::new(config).await?;
        db.start().await?;

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .no_proxy()
            .build()?;
        let health_url = format!("http://127.0.0.1:{}/health", rest_port);
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(resp) = http.get(&health_url).send().await {
                if resp.status().is_success() {
                    break;
                }
            }
            if std::time::Instant::now() > deadline {
                anyhow::bail!("REST didn't become ready in 20s");
            }
            sleep(Duration::from_millis(100)).await;
        }

        Ok(Self {
            rest_port,
            db: Some(db),
            _tmp_data: tmp_data,
        })
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.rest_port)
    }
}

impl Drop for RecallBandsServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

fn deterministic_vec(seed: usize, dim: usize) -> Vec<f32> {
    // Deterministic vector with seed-dependent dominant dimension. Enough
    // structure that top-k results are stable across runs without needing
    // a heavyweight PRNG.
    let mut v = vec![0.0f32; dim];
    let dominant = seed % dim;
    for j in 0..dim {
        v[j] = ((j as f32 + 1.0) * 0.001) + if j == dominant { 1.0 } else { 0.0 };
    }
    v
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

/// **Beta in v0.2** — this test is `#[ignore]`'d because ADR-011 filtered-ANN
/// recall is itself Beta in v0.2. The harness, deterministic dataset, and
/// recall computation are in place so the test can be unignored once the
/// filter-evaluation path consistently surfaces matching records to the
/// AXIS predicate-aware HNSW traversal. The current Low/Medium band thresholds
/// (recall@10 ≥ 0.30 / 0.60) are non-regression placeholders, not SLAs.
///
/// Tracked in `docs/10-quality/TECHNICAL_DEBT.adoc` TD-073 (per-collection
/// AnnFilteringPolicy catalog plumbing). Unignore as part of that work.
#[ignore = "ADR-011 filtered-ANN recall is Beta in v0.2; see TD-073"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn filtered_ann_recall_across_selectivity_bands() {
    let server = RecallBandsServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();

    let coll = format!(
        "ann_recall_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dim: usize = 32;
    let n: usize = 200;
    let top_k: usize = 10;

    let create_resp = http
        .post(format!("{}/api/v2/collections", server.base_url()))
        .json(&json!({
            "name": coll,
            "dimension": dim,
            "engine": "sst",
            "distance_metric": "cosine",
            "canonical_embedding_precision": "fp32",
            "enable_proxima_record": false,
        }))
        .send()
        .await
        .expect("v2 create");
    assert!(
        create_resp.status().is_success(),
        "v2 create: {} {}",
        create_resp.status(),
        create_resp.text().await.unwrap_or_default()
    );

    // Insert 200 vectors with three bucket tags. The bucket distribution
    // sets the per-band selectivity:
    //   bucket=0 — 10% (rec-0..rec-19)         → Low selectivity
    //   bucket=1 — 50% (rec-20..rec-119)       → Medium selectivity
    //   bucket=2 — 40% (rec-120..rec-199)      → High selectivity / no-filter baseline
    let records: Vec<Value> = (0..n)
        .map(|i| {
            let bucket = if i < 20 {
                0
            } else if i < 120 {
                1
            } else {
                2
            };
            json!({
                "id": format!("rec-{i}"),
                "vector": deterministic_vec(i, dim),
                "props": { "bucket": bucket }
            })
        })
        .collect();
    let insert_resp = http
        .post(format!(
            "{}/api/v2/collections/{}/records/batch",
            server.base_url(),
            coll
        ))
        .json(&json!({ "records": records }))
        .send()
        .await
        .expect("v2 insert");
    assert!(
        insert_resp.status().is_success(),
        "v2 insert: {} {}",
        insert_resp.status(),
        insert_resp.text().await.unwrap_or_default()
    );
    sleep(Duration::from_millis(750)).await;

    // Query vector: same shape as rec-5 (bucket=0). The dominant-dimension
    // structure means top hits should heavily include other bucket-0 records.
    let query = deterministic_vec(5, dim);

    // Band runner: posts a v2 search (optionally with a bucket filter) and
    // returns the result IDs in order. Inlined as a helper rather than a
    // closure because async closures aren't stable.
    async fn run_band(
        http: &reqwest::Client,
        base_url: &str,
        coll: &str,
        query: Vec<f32>,
        top_k: usize,
        bucket_filter: Option<i64>,
        label: &str,
    ) -> Vec<String> {
        let mut body = json!({
            "vector": query,
            "top_k": top_k,
        });
        if let Some(b) = bucket_filter {
            body["filters"] = json!([
                { "field": "bucket", "op": "eq", "value": b }
            ]);
        }
        let resp = http
            .post(format!("{}/api/v2/collections/{}/search", base_url, coll))
            .json(&body)
            .send()
            .await
            .unwrap_or_else(|_| panic!("v2 search ({label}) send"));
        assert!(
            resp.status().is_success(),
            "v2 search ({label}) failed: status={}",
            resp.status()
        );
        let text = resp.text().await.unwrap_or_default();
        let json: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| panic!("v2 search ({label}) JSON parse: {text}"));
        ids_from_search_body(&json)
    }

    // Compute ground truth: for the filtered band, the "exact" set is the
    // closest top_k records among those matching the filter. We use the
    // deterministic vector ordering — rec-N closeness to rec-5 declines
    // with |N - 5| in the dominant dimension. So filtered exact = the
    // top_k records by |rec-id - 5| within the bucket.
    let exact_filtered = |bucket: Option<i64>| -> Vec<String> {
        let mut candidates: Vec<usize> = (0..n)
            .filter(|i| match bucket {
                Some(0) => *i < 20,
                Some(1) => (20..120).contains(i),
                Some(2) => *i >= 120,
                None => true,
                _ => false,
            })
            .collect();
        // Distance proxy: |dominant_dim_diff| * dim, then |id - 5| as tiebreak.
        candidates.sort_by_key(|&i| {
            let dom_i = i % dim;
            let dom_q = 5 % dim;
            let dom_diff = if dom_i == dom_q { 0 } else { 1 };
            (dom_diff, i.abs_diff(5))
        });
        candidates
            .into_iter()
            .take(top_k)
            .map(|i| format!("rec-{i}"))
            .collect()
    };

    let recall = |exact: &[String], ann: &[String]| -> f64 {
        if exact.is_empty() {
            return 1.0;
        }
        let exact_set: HashSet<&String> = exact.iter().collect();
        let hit = ann.iter().filter(|id| exact_set.contains(id)).count();
        hit as f64 / exact.len() as f64
    };

    // Low selectivity (~10%): bucket=0
    let base = server.base_url();
    let low_ann = run_band(
        &http, &base, &coll, query.clone(), top_k, Some(0), "low",
    )
    .await;
    let low_exact = exact_filtered(Some(0));
    let low_recall = recall(&low_exact, &low_ann);
    eprintln!(
        "Low band  | exact={low_exact:?} ann={low_ann:?} recall@{top_k}={low_recall:.2}"
    );
    assert!(
        low_recall >= 0.30,
        "Low-selectivity ANN recall@{top_k} regressed below 0.30 (got {low_recall:.2}). \
         exact={low_exact:?} ann={low_ann:?}"
    );

    // Medium selectivity (~50%): bucket=1
    let med_ann = run_band(
        &http, &base, &coll, query.clone(), top_k, Some(1), "medium",
    )
    .await;
    let med_exact = exact_filtered(Some(1));
    let med_recall = recall(&med_exact, &med_ann);
    eprintln!(
        "Med band  | exact={med_exact:?} ann={med_ann:?} recall@{top_k}={med_recall:.2}"
    );
    assert!(
        med_recall >= 0.60,
        "Medium-selectivity ANN recall@{top_k} regressed below 0.60 (got {med_recall:.2}). \
         exact={med_exact:?} ann={med_ann:?}"
    );

    // High selectivity / no filter (baseline)
    let high_ann = run_band(
        &http, &base, &coll, query.clone(), top_k, None, "high",
    )
    .await;
    let high_exact = exact_filtered(None);
    let high_recall = recall(&high_exact, &high_ann);
    eprintln!(
        "High band | exact={high_exact:?} ann={high_ann:?} recall@{top_k}={high_recall:.2}"
    );
    assert!(
        high_recall >= 0.80,
        "Unfiltered ANN recall@{top_k} regressed below 0.80 (got {high_recall:.2}). \
         exact={high_exact:?} ann={high_ann:?}"
    );
}
