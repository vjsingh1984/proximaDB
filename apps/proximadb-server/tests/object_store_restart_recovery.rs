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
