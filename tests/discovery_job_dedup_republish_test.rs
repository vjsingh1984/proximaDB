//! S5 gate (Phase 8 F1): end-to-end Continuous Discovery dedup over REST v2.
//!
//! Boots the full DB (`ProximaDB::new` wires `SharedServices`, including the
//! background `DiscoveryJobExecutor` and the snapshot-publish coordinator),
//! creates a collection, inserts records with exact-duplicate vectors,
//! schedules a `dedup` discovery job via the v2 endpoint, waits for the
//! background executor to complete it, and asserts:
//!   * the job reaches `complete` (full pipeline: pin -> pass -> atomic
//!     republish), and
//!   * duplicates were removed (`removed_count >= 2`).
//!
//! Exercises S0 (snapshot coordinator) + S1 (registry) + S2/S2b (service +
//! executor wired into SharedServices) + S3 (dedup pass) + S4 (REST surface).

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_dedup_job_removes_duplicates_and_completes() {
    let server = DiscoveryServer::start().await.expect("server start");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .build()
        .unwrap();
    let base = server.base_url();
    let name = format!("disc_dedup_{}", nanos());
    let dim: usize = 8;

    // CREATE (fp32 default, canonical searchable path).
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

    // INSERT 5 records; rec-3 duplicates rec-0 and rec-4 duplicates rec-1.
    let unit = |i: usize| -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        v[i % dim] = 1.0;
        v
    };
    let records = json!({
        "records": [
            { "id": "rec-0", "vector": unit(0) },
            { "id": "rec-1", "vector": unit(1) },
            { "id": "rec-2", "vector": unit(2) },
            { "id": "rec-3", "vector": unit(0) },
            { "id": "rec-4", "vector": unit(1) },
        ]
    });
    let insert = http
        .post(format!("{base}/api/v2/collections/{name}/records/batch"))
        .json(&records)
        .send()
        .await
        .expect("v2 insert");
    assert!(
        insert.status().is_success(),
        "v2 insert: {} {}",
        insert.status(),
        insert.text().await.unwrap_or_default()
    );

    sleep(Duration::from_millis(500)).await;

    // SCHEDULE a dedup discovery job via the v2 endpoint.
    let job_resp = http
        .post(format!("{base}/api/v2/collections/{name}/discovery-jobs"))
        .json(&json!({ "kind": "dedup" }))
        .send()
        .await
        .expect("create discovery job");
    assert!(
        job_resp.status().is_success(),
        "create discovery job: {} {}",
        job_resp.status(),
        job_resp.text().await.unwrap_or_default()
    );
    let job_body: serde_json::Value = job_resp.json().await.unwrap();
    let job_id = job_body["job"]["job_id"]
        .as_str()
        .expect("job_id in response")
        .to_string();

    // POLL until the background executor finishes (it polls every ~2s).
    let deadline = Instant::now() + Duration::from_secs(25);
    let final_job = loop {
        let g = http
            .get(format!(
                "{base}/api/v2/collections/{name}/discovery-jobs/{job_id}"
            ))
            .send()
            .await
            .expect("get discovery job");
        assert!(g.status().is_success(), "get discovery job: {}", g.status());
        let body: serde_json::Value = g.json().await.unwrap();
        let status = body["job"]["status"].as_str().unwrap_or("").to_string();
        if status == "complete" || status == "failed" {
            break body;
        }
        if Instant::now() > deadline {
            panic!("discovery job {job_id} did not finish in 25s: {body}");
        }
        sleep(Duration::from_millis(500)).await;
    };

    let job = &final_job["job"];

    // Primary gate: the full CS/CD pipeline ran end-to-end through REST —
    // schedule (S4) -> background executor claim (S2/S2b) -> pin snapshot (S0)
    // -> dedup pass (S3) -> atomic republish (S0 coordinator) -> Complete.
    assert_eq!(
        job["status"].as_str(),
        Some("complete"),
        "discovery job should complete (pin -> dedup -> atomic republish): {final_job}"
    );
    assert!(
        job["snapshot_to_lsn"].is_u64(),
        "completed job should record the pinned snapshot range: {final_job}"
    );

    // Dedup efficacy: the storage-inclusive scan
    // (`list_all_records_with_tenant_context`) enumerates WAL + flushed storage,
    // so the records are visible regardless of flush timing. The 2 duplicates
    // (rec-3 == rec-0, rec-4 == rec-1) must be removed.
    let input = job["input_record_count"].as_u64().unwrap_or(0);
    let removed = job["removed_count"].as_u64().unwrap_or(0);
    assert!(
        removed >= 2,
        "dedup must remove the 2 duplicate records (rec-3, rec-4); \
         got removed_count={removed}, input_record_count={input}: {final_job}"
    );
}
