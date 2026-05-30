//! S? gate (Phase 8 F1, T1.9): the write-volume DriftWatcher closes the loop
//! end-to-end — writes after a recluster auto-trigger another recluster, with
//! no operator action.
//!
//! Boots a full DB with the drift knob driven tiny via env
//! (`PROXIMADB_DRIFT_THRESHOLD_WRITES=1`, `PROXIMADB_DRIFT_INTERVAL_SECS=1`) so the
//! background watcher fires fast, then:
//!   1. inserts records and runs ONE recluster (seeds discovery history + the
//!      LSN baseline — the watcher only sweeps collections that have history),
//!   2. inserts MORE records (advancing the manifest LSN past the baseline),
//!   3. asserts a SECOND recluster job appears and completes *on its own* — the
//!      flywheel: writes -> drift -> on_signal -> recluster -> republish.
//!
//! Own server harness per the tests/ convention (one ProximaDB boot per process
//! — the global WAL manifest is a process singleton).

use std::net::TcpListener;
use std::time::{Duration, Instant};

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

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

struct DiscoveryServer {
    rest_port: u16,
    db: Option<ProximaDB>,
    _tmp_data: TempDir,
}

impl DiscoveryServer {
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

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .no_proxy()
            .build()?;
        let health_url = format!("http://127.0.0.1:{}/health", rest_port);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match http_client.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => {
                    if Instant::now() > deadline {
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

impl Drop for DiscoveryServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

/// Count of `complete` recluster jobs for a collection.
async fn complete_recluster_count(http: &reqwest::Client, base: &str, name: &str) -> usize {
    let resp = http
        .get(format!("{base}/api/v2/collections/{name}/discovery-jobs"))
        .send()
        .await
        .expect("list discovery jobs");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["jobs"]
        .as_array()
        .map(|jobs| {
            jobs.iter()
                .filter(|j| {
                    j["kind"].as_str() == Some("recluster")
                        && j["status"].as_str() == Some("complete")
                })
                .count()
        })
        .unwrap_or(0)
}

async fn schedule_recluster(http: &reqwest::Client, base: &str, name: &str) -> String {
    let resp = http
        .post(format!("{base}/api/v2/collections/{name}/discovery-jobs"))
        .json(&json!({ "kind": "recluster" }))
        .send()
        .await
        .expect("create recluster job");
    assert!(resp.status().is_success(), "create recluster: {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    body["job"]["job_id"].as_str().expect("job_id").to_string()
}

async fn insert_varied(http: &reqwest::Client, base: &str, name: &str, range: std::ops::Range<usize>, dim: usize) {
    let mk = |i: usize| -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        v[i % dim] = 1.0;
        v[(i / dim + 1) % dim] += 0.5;
        v
    };
    let recs: Vec<serde_json::Value> = range
        .map(|i| json!({ "id": format!("rec-{i}"), "vector": mk(i) }))
        .collect();
    let resp = http
        .post(format!("{base}/api/v2/collections/{name}/records/batch"))
        .json(&json!({ "records": recs }))
        .send()
        .await
        .expect("v2 insert");
    assert!(
        resp.status().is_success(),
        "v2 insert: {} {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drift_watcher_auto_reclusters_after_writes() {
    // Drive the watcher fast: a single write batch past the last recluster,
    // swept every 1s. Must be set before the server boots (SharedServices reads
    // it at construction).
    unsafe {
        std::env::set_var("PROXIMADB_DRIFT_THRESHOLD_WRITES", "1");
        std::env::set_var("PROXIMADB_DRIFT_INTERVAL_SECS", "1");
    }

    let server = DiscoveryServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();
    let base = server.base_url();
    let name = format!("disc_drift_{}", nanos());
    let dim: usize = 8;

    // CREATE.
    let create = http
        .post(format!("{base}/api/v2/collections"))
        .json(&json!({
            "name": name,
            "dimension": dim,
            "engine": "sst",
            "distance_metric": "cosine",
            "enable_proxima_record": false,
        }))
        .send()
        .await
        .expect("v2 create");
    assert!(
        create.status().is_success(),
        "v2 create: {} {}",
        create.status(),
        create.text().await.unwrap_or_default()
    );

    // 1. Insert 20 records and run ONE recluster to seed history + baseline.
    //    (Until a collection has discovery history, the watcher does not sweep
    //    it — so an explicit first recluster bootstraps the loop.)
    insert_varied(&http, &base, &name, 0..20, dim).await;
    sleep(Duration::from_millis(500)).await;
    let seed_job = schedule_recluster(&http, &base, &name).await;

    // Wait for the seed recluster to complete (baseline established).
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let g = http
            .get(format!("{base}/api/v2/collections/{name}/discovery-jobs/{seed_job}"))
            .send()
            .await
            .expect("get seed job");
        let body: serde_json::Value = g.json().await.unwrap();
        match body["job"]["status"].as_str() {
            Some("complete") => break,
            Some("failed") => panic!("seed recluster failed: {body}"),
            _ => {}
        }
        if Instant::now() > deadline {
            panic!("seed recluster did not complete in 30s");
        }
        sleep(Duration::from_millis(400)).await;
    }
    // Let the loop settle, then capture the baseline recluster count. Recluster
    // is non-mutating and idle = no writes, so this must be stable (a rising
    // count here would mean the watcher loops on its own republish — a bug).
    sleep(Duration::from_secs(3)).await;
    let before = complete_recluster_count(&http, &base, &name).await;
    assert!(before >= 1, "seed recluster should have completed");

    // 2. Insert MORE records — advances the manifest LSN past the baseline.
    insert_varied(&http, &base, &name, 20..40, dim).await;
    sleep(Duration::from_millis(500)).await;

    // 3. The watcher must auto-enqueue and complete ANOTHER recluster with no
    //    further operator action — the flywheel closes on live write volume.
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        if complete_recluster_count(&http, &base, &name).await > before {
            break;
        }
        if Instant::now() > deadline {
            // Surface the full job list for diagnosis.
            let resp = http
                .get(format!("{base}/api/v2/collections/{name}/discovery-jobs"))
                .send()
                .await
                .unwrap();
            let body: serde_json::Value = resp.json().await.unwrap();
            panic!("drift watcher did not auto-trigger another recluster (>{before}) in 40s: {body}");
        }
        sleep(Duration::from_millis(500)).await;
    }

    // ── Bootstrap: a collection that NEVER had a manual discovery job must still
    //    be reclustered automatically once its vectors are indexed. This proves
    //    the watcher sweeps served-index collections (not just those with prior
    //    discovery history) — the autonomy gap that history-gating left open.
    let fresh = format!("disc_bootstrap_{}", nanos());
    let create = http
        .post(format!("{base}/api/v2/collections"))
        .json(&json!({
            "name": fresh,
            "dimension": dim,
            "engine": "sst",
            "distance_metric": "cosine",
            "enable_proxima_record": false,
        }))
        .send()
        .await
        .expect("v2 create fresh");
    assert!(
        create.status().is_success(),
        "v2 create fresh: {} {}",
        create.status(),
        create.text().await.unwrap_or_default()
    );
    // Index vectors only — no discovery job is ever scheduled for `fresh`.
    insert_varied(&http, &base, &fresh, 0..24, dim).await;

    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        if complete_recluster_count(&http, &base, &fresh).await >= 1 {
            break;
        }
        if Instant::now() > deadline {
            let resp = http
                .get(format!("{base}/api/v2/collections/{fresh}/discovery-jobs"))
                .send()
                .await
                .unwrap();
            let body: serde_json::Value = resp.json().await.unwrap();
            panic!(
                "drift watcher did not bootstrap-recluster a never-seeded collection in 40s: {body}"
            );
        }
        sleep(Duration::from_millis(500)).await;
    }
}
