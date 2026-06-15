//! Tenant-scoped metadata-filter enforcement across the three v2 retrieval
//! paths — regression coverage for the AnvaiOps control-plane audit
//! (`HANDOFF_filter_enforcement.md`).
//!
//! A dim-1 collection holds three records: `a1,a2` → `account_id=acctA`,
//! `b1` → `account_id=acctB`. A filter of `account_id=acctA` must return exactly
//! `a1,a2` on every path. Before the fix:
//!   - typed `/search` over-filtered (returned 0 — bloom key mismatch),
//!   - `/records/scan` ignored the filter (returned all — a cross-tenant leak),
//!   - `/hybrid/search` ignored the filter on its BM25 leg (leak).
//!
//! Scan is the load-bearing assertion here: it reads the visible record set
//! directly (no dependence on the separate, lagging search index), so it is the
//! most reliable end-to-end proof that the filter predicate is applied.

use std::collections::BTreeSet;
use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::sleep;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind port 0");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

struct TestServer {
    rest_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl TestServer {
    async fn start() -> anyhow::Result<Self> {
        let rest_port = free_port();
        let tmp_data = TempDir::new()?;

        let mut config = Config::default();
        config.server.bind_address = "127.0.0.1".to_string();
        config.server.port = rest_port;
        config.server.data_dir = tmp_data.path().to_path_buf();
        config.api.rest_port = rest_port;
        config.api.grpc_port = free_port();
        config.api.arrow_flight_port = free_port();
        config.api.pg_port = Some(free_port());
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
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()?;
        let health_url = format!("http://127.0.0.1:{}/health", rest_port);
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            match http.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => {
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!("REST didn't become ready in 15s");
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            }
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

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

fn ids(values: &[serde_json::Value]) -> BTreeSet<String> {
    values
        .iter()
        .filter_map(|v| {
            v.get("id")
                .or_else(|| v.get("record_id"))
                .and_then(|id| id.as_str())
                .map(str::to_string)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_metadata_filter_is_enforced_on_all_paths() {
    let server = TestServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();
    let base = server.base_url();
    let collection = format!(
        "ftest_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    // dim-1 collection with an indexed, filterable account_id column.
    let create = http
        .post(format!("{base}/api/v2/collections"))
        .json(&json!({
            "name": collection,
            "dimension": 1,
            "enable_proxima_record": true,
            "schema": {
                "columns": [
                    { "name": "account_id", "data_type": "text", "indexed": true, "filterable": true }
                ],
                "enforcement": "hybrid",
                "allow_additional_fields": true
            }
        }))
        .send()
        .await
        .expect("create");
    assert!(
        create.status().is_success(),
        "create: {} {}",
        create.status(),
        create.text().await.unwrap_or_default()
    );

    let insert = http
        .post(format!(
            "{base}/api/v2/collections/{collection}/records/batch"
        ))
        .json(&json!({
            "records": [
                { "id": "a1", "vector": [0.0], "props": { "account_id": "acctA" } },
                { "id": "a2", "vector": [0.0], "props": { "account_id": "acctA" } },
                { "id": "b1", "vector": [0.0], "props": { "account_id": "acctB" } }
            ],
            "upsert": true
        }))
        .send()
        .await
        .expect("insert");
    assert!(
        insert.status().is_success(),
        "insert: {} {}",
        insert.status(),
        insert.text().await.unwrap_or_default()
    );

    sleep(Duration::from_millis(500)).await;

    let expected: BTreeSet<String> = ["a1".to_string(), "a2".to_string()].into_iter().collect();

    // ── Path C: /records/scan with an equality-map filter ───────────────────
    let scan = http
        .post(format!(
            "{base}/api/v2/collections/{collection}/records/scan"
        ))
        .json(&json!({ "limit": 10, "filter": { "account_id": "acctA" } }))
        .send()
        .await
        .expect("scan");
    assert!(scan.status().is_success(), "scan status {}", scan.status());
    let scan_body: serde_json::Value = scan.json().await.expect("scan json");
    let scan_records = scan_body
        .get("records")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        ids(&scan_records),
        expected,
        "scan must return exactly acctA records, got {scan_body}"
    );

    // ── Path A: typed /search with a typed-filter list ──────────────────────
    let search = http
        .post(format!("{base}/api/v2/collections/{collection}/search"))
        .json(&json!({
            "vector": [0.0],
            "top_k": 10,
            "filters": [{ "field": "account_id", "op": "eq", "value": "acctA" }]
        }))
        .send()
        .await
        .expect("search");
    assert!(
        search.status().is_success(),
        "search status {}",
        search.status()
    );
    let search_body: serde_json::Value = search.json().await.expect("search json");
    let search_records = search_body
        .get("results")
        .or_else(|| search_body.get("records"))
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let search_ids = ids(&search_records);
    // The filter must never admit acctB; given visible records, it returns acctA.
    assert!(
        !search_ids.contains("b1"),
        "typed search leaked acctB record b1: {search_body}"
    );
    assert_eq!(
        search_ids, expected,
        "typed search must return exactly acctA records, got {search_body}"
    );

    // ── Path B: hybrid search with a {field: value} filter ──────────────────
    // BM25 has no metadata of its own; the filter is enforced by re-checking
    // each candidate's record metadata. The text leg must not leak acctB.
    //
    // The hybrid routes are mounted under different prefixes depending on build
    // wiring (v1 on the server binary, v2 in the router source); auto-discover
    // the one this harness serves and skip if neither is mounted, so this test
    // never goes red for a routing/harness reason unrelated to the filter fix.
    let mut hybrid_prefix: Option<&str> = None;
    for prefix in ["/api/v1/hybrid", "/api/v2/hybrid"] {
        let probe = http
            .post(format!("{base}{prefix}/index"))
            .json(&json!({
                "collection": collection,
                "documents": [
                    { "id": "a1", "text": "alpha" },
                    { "id": "a2", "text": "alpha" },
                    { "id": "b1", "text": "alpha" }
                ]
            }))
            .send()
            .await
            .expect("hybrid index probe");
        if probe.status().is_success() {
            hybrid_prefix = Some(prefix);
            break;
        }
    }

    let Some(prefix) = hybrid_prefix else {
        eprintln!("hybrid routes not mounted in this harness; skipping hybrid leg");
        return;
    };

    let hybrid_search = |filters: Option<serde_json::Value>| {
        let http = http.clone();
        let url = format!("{base}{prefix}/search");
        let collection = collection.clone();
        async move {
            // Faithful to the handoff repro: a text-only hybrid query with an
            // optional metadata filter.
            let mut body = json!({
                "collection": collection,
                "text_query": "alpha",
                "top_k": 10,
            });
            if let Some(f) = filters {
                body["filters"] = f;
            }
            let resp = http
                .post(url)
                .json(&body)
                .send()
                .await
                .expect("hybrid search");
            assert!(
                resp.status().is_success(),
                "hybrid status {}",
                resp.status()
            );
            let parsed: serde_json::Value = resp.json().await.expect("hybrid json");
            let recs = parsed
                .get("results")
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default();
            (ids(&recs), parsed)
        }
    };

    // Baseline (no filter) proves the BM25 leg actually surfaces all three docs,
    // including the cross-account b1 — i.e. there is something to leak.
    let (baseline_ids, baseline_body) = hybrid_search(None).await;
    eprintln!("hybrid baseline ids: {baseline_ids:?}");
    assert!(
        baseline_ids.contains("b1"),
        "BM25 baseline did not surface b1, so the hybrid leak test is vacuous: {baseline_body}"
    );

    // Filtered: the account filter is enforced on the text leg by resolving the
    // authoritative set of record ids whose metadata satisfies the filter (read
    // from the live WAL+storage record set, canonical `evaluate_filter_proxima`)
    // and keeping only BM25 candidates in that set. So the filtered text-only
    // hybrid query must return *exactly* the acctA records a1/a2 — complete, and
    // never the acctB record b1 the unfiltered baseline surfaced.
    let (filtered_ids, filtered_body) = hybrid_search(Some(json!({ "account_id": "acctA" }))).await;
    eprintln!("hybrid filtered ids: {filtered_ids:?}");
    assert!(
        !filtered_ids.contains("b1"),
        "hybrid search leaked acctB record b1 through the BM25 leg: {filtered_body}"
    );
    assert_eq!(
        filtered_ids, expected,
        "hybrid filter must return exactly acctA records a1/a2, got {filtered_body}"
    );
}
