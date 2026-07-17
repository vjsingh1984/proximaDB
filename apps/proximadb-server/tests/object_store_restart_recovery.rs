//! TD-OBJSTORE-2 process-boundary recovery proof.
//!
//! Runs the real server three times against one S3/ADLS object-store prefix,
//! with a brand-new local `server.data_dir` each time:
//!
//! 1. CREATE + INSERT, then SIGKILL (catalog + data WAL must already be durable).
//! 2. Recover catalog + replay WAL, verify search, then SIGINT (flush SST).
//! 3. Recover on another empty local disk and verify cold SST search.
//!
//! Run through `scripts/prove_object_store_durability.sh`; the test is ignored
//! by default because it requires cloud credentials and
//! `PROXIMADB_OBJECT_STORE_URL=s3://...` or `adls://...`.

use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::{Value, json};

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind port 0");
    listener.local_addr().expect("local addr").port()
}

fn write_config(path: &Path, data_dir: &Path, root: &str, ports: [u16; 4]) {
    let [rest, grpc, flight, pg] = ports;
    let config = format!(
        r#"
[server]
node_id = "td-objstore-2"
bind_address = "127.0.0.1"
port = {rest}
data_dir = "{}"

[server.tenant]
mode = "single_tenant"
default_tenant = "default"

[storage]
metadata_url = "{root}/metadata"
mmap_enabled = false

[[storage.storage_locations]]
url = "{root}/collections"
weight = 1
tags = ["td-objstore-2", "durable"]

[storage.optimization]
enable_mmap = false

[storage.wal_config]
write_buffer_directory = "{root}/wal"
enable_wal = true
sync_mode = "PerBatch"

[storage.sst_config]
data_directory = "{root}/collections"
mmap_enabled = false

[storage.viper_config]
data_directory = "{root}/collections"

[api]
rest_port = {rest}
grpc_port = {grpc}
arrow_flight_port = {flight}
pg_port = {pg}
unified_mode = false

[monitoring]
metrics_enabled = false
log_level = "info"
"#,
        data_dir.display()
    );
    std::fs::write(path, config).expect("write process config");
}

struct ServerProcess {
    child: Child,
    base_url: String,
}

impl ServerProcess {
    async fn start(root: &str, data_dir: &Path, config_path: &Path) -> anyhow::Result<Self> {
        let ports = [free_port(), free_port(), free_port(), free_port()];
        write_config(config_path, data_dir, root, ports);
        let child = Command::new(env!("CARGO_BIN_EXE_proximadb-server"))
            .arg("--config")
            .arg(config_path)
            .env("RUST_LOG", "info")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        let server = Self {
            child,
            base_url: format!("http://127.0.0.1:{}", ports[0]),
        };
        server.wait_ready().await?;
        Ok(server)
    }

    async fn wait_ready(&self) -> anyhow::Result<()> {
        let http = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()?;
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if let Ok(response) = http.get(format!("{}/health", self.base_url)).send().await
                && response.status().is_success()
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!("server at {} did not become healthy", self.base_url);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    fn crash(mut self) -> anyhow::Result<()> {
        self.child.kill()?;
        self.child.wait()?;
        Ok(())
    }

    fn graceful(mut self) -> anyhow::Result<()> {
        let status = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()?;
        if !status.success() {
            anyhow::bail!("failed to send SIGINT to server pid {}", self.child.id());
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait()? {
                if !status.success() {
                    anyhow::bail!("server exited unsuccessfully during graceful stop: {status}");
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                anyhow::bail!("server did not complete graceful shutdown in 30s");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

async fn create_and_insert(base: &str, collection: &str, engine: &str) -> anyhow::Result<()> {
    let http = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = http
        .post(format!("{base}/api/v2/collections"))
        .json(&json!({
            "name": collection,
            "dimension": 8,
            "engine": engine,
            "distance_metric": "cosine",
            "canonical_embedding_precision": "fp32",
            "enable_proxima_record": false
        }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(status.is_success(), "create failed: {status} {body}");

    let response = http
        .post(format!(
            "{base}/api/v2/collections/{collection}/records/batch"
        ))
        .json(&json!({
            "records": [
                { "id": "durable-0", "vector": [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7] },
                { "id": "durable-1", "vector": [0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1, 0.0] }
            ]
        }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(status.is_success(), "insert failed: {status} {body}");
    Ok(())
}

async fn assert_recovered(base: &str, collection: &str, phase: &str) -> anyhow::Result<()> {
    let http = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = http
        .get(format!("{base}/api/v2/collections/{collection}"))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(
        status.is_success(),
        "{phase}: catalog did not recover collection: {status} {body}"
    );

    let response = http
        .get(format!(
            "{base}/api/v2/collections/{collection}/records/durable-0"
        ))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(
        status.is_success(),
        "{phase}: recovered point read failed: {status} {body}"
    );
    let record: Value = serde_json::from_str(&body)?;
    anyhow::ensure!(
        record.get("id").and_then(Value::as_str) == Some("durable-0"),
        "{phase}: point read returned the wrong record: {body}"
    );

    let response = http
        .post(format!("{base}/api/v2/collections/{collection}/search"))
        .json(&json!({
            "vector": [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7],
            "top_k": 2
        }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(
        status.is_success(),
        "{phase}: search failed: {status} {body}"
    );
    let json: Value = serde_json::from_str(&body)?;
    let ids = json
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    anyhow::ensure!(
        ids.contains(&"durable-0"),
        "{phase}: durable-0 absent after recovery: {body}"
    );
    Ok(())
}

/// TD-OBJSTORE-1 batch 3: seed a dim-1 "zero-vector" KV collection.
///
/// anvaiops uses `vector: [0.0]` rows as a durable KV store (API keys, tenant
/// registry, billing). The engine skips embedding for zero-vector rows and
/// serves them by id. `rec-zero` is the KV record under test; `rec-ctrl` is a
/// non-zero dim-1 control that isolates the zero-vector-ness as the variable.
async fn create_and_insert_kv(base: &str, collection: &str, engine: &str) -> anyhow::Result<()> {
    let http = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = http
        .post(format!("{base}/api/v2/collections"))
        .json(&json!({
            "name": collection,
            "dimension": 1,
            "engine": engine,
            "distance_metric": "cosine",
            "canonical_embedding_precision": "fp32",
            "enable_proxima_record": false
        }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(status.is_success(), "kv create failed: {status} {body}");

    let response = http
        .post(format!(
            "{base}/api/v2/collections/{collection}/records/batch"
        ))
        .json(&json!({
            "records": [
                { "id": "rec-ctrl", "vector": [0.5], "props": { "kind": "control" } },
                { "id": "rec-zero", "vector": [0.0], "props": { "kind": "api_key", "secret": "sk-anvaiops" } }
            ]
        }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(status.is_success(), "kv insert failed: {status} {body}");
    // Both records must exist immediately after seed (matches the TD: the KV
    // record is queryable at seed time; it only 404s after restart).
    for id in ["rec-ctrl", "rec-zero"] {
        let r = http
            .get(format!(
                "{base}/api/v2/collections/{collection}/records/{id}"
            ))
            .send()
            .await?;
        anyhow::ensure!(
            r.status().is_success(),
            "kv seed: {id} not readable at seed time: {}",
            r.status()
        );
    }
    Ok(())
}

/// Assert the dim-1 zero-vector KV record survived restart. The TD symptom is a
/// 404 on `GET .../records/rec-zero` (and `record_count=0`) after the VM restart.
async fn assert_kv_recovered(base: &str, collection: &str, phase: &str) -> anyhow::Result<()> {
    let http = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()?;
    // Collection must recover from the catalog.
    let response = http
        .get(format!("{base}/api/v2/collections/{collection}"))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(
        status.is_success(),
        "{phase}: catalog did not recover kv collection: {status} {body}"
    );

    // Each record must be readable by id after restart (the actual TD symptom).
    for id in ["rec-ctrl", "rec-zero"] {
        let response = http
            .get(format!(
                "{base}/api/v2/collections/{collection}/records/{id}"
            ))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        anyhow::ensure!(
            status.is_success(),
            "{phase}: kv record {id} gone after restart ({status}): {body}"
        );
        let record: Value = serde_json::from_str(&body)?;
        anyhow::ensure!(
            record.get("id").and_then(Value::as_str) == Some(id),
            "{phase}: kv point read returned the wrong record: {body}"
        );
    }
    Ok(())
}

/// TD-OBJSTORE-1 batch 3 regression: a dim-1 zero-vector KV record written to an
/// object-store backend, then lost after a crash+restart. Reproduces on
/// `adls://`/`s3://`/`gs://` (whole-object-overwrite WAL); passes trivially on
/// `file://`. Runs against Azurite/MinIO/fake-gcs via
/// `scripts/run_cloud_emulator_tests.sh` or a real cloud
/// `PROXIMADB_OBJECT_STORE_URL` (TD-OBJSTORE-5 tiers).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires PROXIMADB_OBJECT_STORE_URL (adls://Azurite, s3://MinIO, gs://fake-gcs, or real cloud)"]
async fn zero_vector_kv_record_survives_restart() -> anyhow::Result<()> {
    let base = std::env::var("PROXIMADB_OBJECT_STORE_URL")?;
    anyhow::ensure!(
        base.starts_with("s3://")
            || base.starts_with("adls://")
            || base.starts_with("az://")
            || base.starts_with("gs://"),
        "test URL must use s3://, adls://, az:// or gs:// (got {base})"
    );
    let root = format!(
        "{}/td-objstore-1-kv/{}",
        base.trim_end_matches('/'),
        uuid::Uuid::new_v4().simple()
    );
    let tmp = tempfile::tempdir()?;
    let collection = format!("kv_{}", uuid::Uuid::new_v4().simple());

    let vm1 = tmp.path().join("vm1");
    let vm2 = tmp.path().join("vm2");
    for dir in [&vm1, &vm2] {
        std::fs::create_dir_all(dir)?;
    }

    // Phase 1: seed the KV records, then SIGKILL before any flush (WAL-only).
    let first = ServerProcess::start(&root, &vm1, &tmp.path().join("vm1.toml")).await?;
    create_and_insert_kv(&first.base_url, &collection, "sst").await?;
    first.crash()?;

    // Phase 2: restart on a fresh local disk → WAL replay from object store.
    let wal_replay = ServerProcess::start(&root, &vm2, &tmp.path().join("vm2.toml")).await?;
    assert_kv_recovered(&wal_replay.base_url, &collection, "KV WAL replay").await?;
    wal_replay.crash()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires PROXIMADB_OBJECT_STORE_URL plus S3/ADLS credentials"]
async fn catalog_wal_and_sst_survive_fresh_local_disks() -> anyhow::Result<()> {
    let base = std::env::var("PROXIMADB_OBJECT_STORE_URL")?;
    anyhow::ensure!(
        base.starts_with("s3://") || base.starts_with("adls://"),
        "test URL must use s3:// or adls:// (got {base})"
    );
    let root = format!(
        "{}/td-objstore-2/{}",
        base.trim_end_matches('/'),
        uuid::Uuid::new_v4().simple()
    );
    let tmp = tempfile::tempdir()?;
    let sst_collection = format!("td_objstore_sst_{}", uuid::Uuid::new_v4().simple());
    let helix_collection = format!("td_objstore_helix_{}", uuid::Uuid::new_v4().simple());

    let vm1 = tmp.path().join("vm1");
    let vm2 = tmp.path().join("vm2");
    let vm3 = tmp.path().join("vm3");
    for dir in [&vm1, &vm2, &vm3] {
        std::fs::create_dir_all(dir)?;
    }

    let first = ServerProcess::start(&root, &vm1, &tmp.path().join("vm1.toml")).await?;
    create_and_insert(&first.base_url, &sst_collection, "sst").await?;
    create_and_insert(&first.base_url, &helix_collection, "helix").await?;
    first.crash()?;

    let wal_replay = ServerProcess::start(&root, &vm2, &tmp.path().join("vm2.toml")).await?;
    assert_recovered(&wal_replay.base_url, &sst_collection, "SST WAL replay").await?;
    assert_recovered(&wal_replay.base_url, &helix_collection, "HELIX WAL replay").await?;
    wal_replay.graceful()?; // materializes SST and reclaims flushed WAL

    let cold_sst = ServerProcess::start(&root, &vm3, &tmp.path().join("vm3.toml")).await?;
    assert_recovered(&cold_sst.base_url, &sst_collection, "cold SST").await?;
    assert_recovered(&cold_sst.base_url, &helix_collection, "cold HELIX").await?;
    cold_sst.crash()?;
    Ok(())
}

/// Deterministic splitmix64 (no `rand` dev-dep) — reproducible seed vectors.
fn splitmix64(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn unit_vec(seed: &mut u64, dim: usize) -> Vec<f32> {
    let raw: Vec<f64> = (0..dim)
        .map(|_| (splitmix64(seed) as f64 / u64::MAX as f64) * 2.0 - 1.0)
        .collect();
    let norm = raw.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-12);
    raw.iter().map(|x| (x / norm) as f32).collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// TD-OBJSTORE-5 S3 (ADR-063 D8 QA tier): cold-read recall ratchet on the object
/// store. Seeds a deterministic corpus, flushes (graceful stop), restarts on a
/// FRESH local disk, and asserts recall@5 vs local brute force. The storage
/// backend must be recall-NEUTRAL: this proves the quantized cascade's ranged
/// reads + footer parse are byte-correct over the real cloud wire API — any
/// recall delta vs the file:// baseline is a read-path bug, not a tuning knob.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires PROXIMADB_OBJECT_STORE_URL (adls://Azurite, s3://MinIO, gs://fake-gcs, or real cloud)"]
async fn cold_recall_ratchet_survives_restart() -> anyhow::Result<()> {
    const DIM: usize = 8;
    const N: usize = 64;
    const K: usize = 5;
    const PROBES: usize = 8;
    const MIN_RECALL: f32 = 0.9;

    let base = std::env::var("PROXIMADB_OBJECT_STORE_URL")?;
    anyhow::ensure!(
        base.starts_with("s3://")
            || base.starts_with("adls://")
            || base.starts_with("az://")
            || base.starts_with("gs://"),
        "test URL must use s3://, adls://, az:// or gs:// (got {base})"
    );
    let root = format!(
        "{}/td-objstore-5-recall/{}",
        base.trim_end_matches('/'),
        uuid::Uuid::new_v4().simple()
    );
    let tmp = tempfile::tempdir()?;
    let collection = format!("recall_{}", uuid::Uuid::new_v4().simple());
    let vm1 = tmp.path().join("vm1");
    let vm2 = tmp.path().join("vm2");
    for dir in [&vm1, &vm2] {
        std::fs::create_dir_all(dir)?;
    }

    let mut seed = 0x5EED_5EED_5EED_5EEDu64;
    let corpus: Vec<Vec<f32>> = (0..N).map(|_| unit_vec(&mut seed, DIM)).collect();

    // Phase 1: seed the corpus, then GRACEFUL stop (SIGINT flushes the segment) —
    // this ratchet targets the COLD read path, not WAL replay.
    let first = ServerProcess::start(&root, &vm1, &tmp.path().join("vm1.toml")).await?;
    {
        let http = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()?;
        let response = http
            .post(format!("{}/api/v2/collections", first.base_url))
            .json(&json!({
                "name": collection,
                "dimension": DIM,
                "engine": "sst",
                "distance_metric": "cosine",
                "canonical_embedding_precision": "fp32",
                "enable_proxima_record": false
            }))
            .send()
            .await?;
        anyhow::ensure!(response.status().is_success(), "recall create failed");
        let records: Vec<Value> = corpus
            .iter()
            .enumerate()
            .map(|(i, v)| json!({ "id": format!("vec-{i}"), "vector": v }))
            .collect();
        let response = http
            .post(format!(
                "{}/api/v2/collections/{collection}/records/batch",
                first.base_url
            ))
            .json(&json!({ "records": records }))
            .send()
            .await?;
        anyhow::ensure!(response.status().is_success(), "recall insert failed");
    }
    first.graceful()?;

    // Phase 2: fresh local disk → cold read from the object store only.
    let cold = ServerProcess::start(&root, &vm2, &tmp.path().join("vm2.toml")).await?;
    let http = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut total_hits = 0usize;
    for p in 0..PROBES {
        // Probe = perturbed copy of every 8th corpus vector (deterministic).
        let base_vec = &corpus[p * (N / PROBES)];
        let mut pseed = 0xABCD_EF01_2345_6789u64 ^ (p as u64);
        let noise = unit_vec(&mut pseed, DIM);
        let probe: Vec<f32> = base_vec
            .iter()
            .zip(&noise)
            .map(|(a, n)| a + 0.05 * n)
            .collect();

        // Local brute-force top-K by cosine.
        let mut scored: Vec<(usize, f32)> = corpus
            .iter()
            .enumerate()
            .map(|(i, v)| (i, cosine(&probe, v)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let truth: Vec<String> = scored[..K]
            .iter()
            .map(|(i, _)| format!("vec-{i}"))
            .collect();

        let response = http
            .post(format!(
                "{}/api/v2/collections/{collection}/search",
                cold.base_url
            ))
            .json(&json!({ "vector": probe, "top_k": K }))
            .send()
            .await?;
        anyhow::ensure!(response.status().is_success(), "recall search failed");
        let body: Value = serde_json::from_str(&response.text().await?)?;
        let got: Vec<&str> = body
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| row.get("id").and_then(Value::as_str))
            .collect();
        total_hits += truth.iter().filter(|t| got.contains(&t.as_str())).count();
    }
    let recall = total_hits as f32 / (PROBES * K) as f32;
    cold.crash()?;
    anyhow::ensure!(
        recall >= MIN_RECALL,
        "cold recall@{K} on the object store = {recall:.3} < {MIN_RECALL} — \
         backend must be recall-neutral; a delta vs file:// is a read-path bug"
    );
    Ok(())
}
