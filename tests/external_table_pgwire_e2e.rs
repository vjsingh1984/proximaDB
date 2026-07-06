// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! External parquet table over pgwire — TD-OLAP-4 (external-table hardening).
//!
//! Proves the end-to-end path for registering an existing Parquet object as a
//! read-only table WITHOUT ingestion:
//!   CREATE TABLE t (...) WITH (format='parquet', external_location='…', authority='external')
//! is persisted in the native catalog as an external-authoritative Parquet
//! layout, and `SELECT` over it routes to the DataFusion OLAP engine and reads
//! the object directly.
//!
//! Also pins the Path-Isolation guardrail: an `external_location` outside the
//! operator allowlist (`PROXIMADB_EXTERNAL_TABLE_ROOTS`) is rejected fail-closed.
//!
//! Gated on `datafusion-integration` (the external read routes to DataFusion).
//!   cargo test --features datafusion-integration --test external_table_pgwire_e2e -- --nocapture
#![cfg(feature = "datafusion-integration")]

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use arrow_array::{Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use proximadb::core::Config;
use proximadb::database::ProximaDB;
use tempfile::TempDir;
use tokio::time::sleep;
use tokio_postgres::NoTls;

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

struct PgServer {
    pg_port: u16,
    _db: ProximaDB,
    _tmp: TempDir,
}

impl PgServer {
    async fn start(tmp: TempDir) -> anyhow::Result<Self> {
        let pg_port = free_port();
        let rest_port = free_port();
        let grpc_port = free_port();
        let mut config = Config::default();
        config.server.bind_address = "127.0.0.1".to_string();
        config.server.port = rest_port;
        config.server.data_dir = tmp.path().to_path_buf();
        config.api.rest_port = rest_port;
        config.api.grpc_port = grpc_port;
        config.api.unified_mode = false;
        config.api.pg_port = Some(pg_port);
        config.storage.storage_locations = vec![proximadb::core::config::StorageLocation {
            url: format!("file://{}", tmp.path().display()),
            ..Default::default()
        }];
        config.storage.wal_config.write_buffer_directory =
            format!("file://{}/wal", tmp.path().display());
        let mut db = ProximaDB::new(config).await?;
        db.start().await?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
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
        sleep(Duration::from_millis(200)).await;
        Ok(Self {
            pg_port,
            _db: db,
            _tmp: tmp,
        })
    }

    fn conn_str(&self) -> String {
        format!(
            "host=127.0.0.1 port={} user=postgres dbname=proximadb sslmode=disable",
            self.pg_port
        )
    }
}

/// Write a tiny two-column Parquet object and return its `file://` URI.
fn write_parquet(dir: &std::path::Path) -> String {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("name", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int32Array::from(vec![1, 2, 3])),
            Arc::new(StringArray::from(vec!["a", "b", "c"])),
        ],
    )
    .expect("batch");
    let path = dir.join("ext.parquet");
    let file = std::fs::File::create(&path).expect("create parquet");
    let mut w = ArrowWriter::try_new(file, schema, None).expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
    format!("file://{}", path.display())
}

async fn connect(server: &PgServer) -> tokio_postgres::Client {
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), NoTls)
        .await
        .expect("connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_parquet_table_registers_and_reads_via_datafusion() {
    let tmp = TempDir::new().expect("tmp");
    // Allowlist the server data dir (which holds the external object).
    let root = format!("file://{}", tmp.path().display());
    unsafe { std::env::set_var("PROXIMADB_EXTERNAL_TABLE_ROOTS", &root) };

    let ext_dir = tmp.path().join("external");
    std::fs::create_dir_all(&ext_dir).expect("mkdir");
    let location = write_parquet(&ext_dir);

    let server = PgServer::start(tmp).await.expect("server");
    let client = connect(&server).await;

    // Register the existing Parquet object as a read-only external table.
    client
        .simple_query(&format!(
            "CREATE TABLE ext_hits (id INT, name VARCHAR) \
             WITH (format='parquet', external_location='{location}', authority='external')"
        ))
        .await
        .expect("create external table");

    // SELECT routes to the DataFusion OLAP engine and reads the object directly.
    let rows = client
        .simple_query("SELECT id, name FROM ext_hits ORDER BY id")
        .await
        .expect("select");
    let data: Vec<_> = rows
        .iter()
        .filter_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => Some((
                r.get("id").unwrap().to_string(),
                r.get("name").unwrap().to_string(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        data,
        vec![
            ("1".to_string(), "a".to_string()),
            ("2".to_string(), "b".to_string()),
            ("3".to_string(), "c".to_string()),
        ],
        "external parquet rows must be read back via the DataFusion route"
    );

    unsafe { std::env::remove_var("PROXIMADB_EXTERNAL_TABLE_ROOTS") };
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_location_outside_allowlist_is_rejected() {
    let tmp = TempDir::new().expect("tmp");
    // Allowlist ONLY the server dir; the table below points outside it.
    unsafe {
        std::env::set_var(
            "PROXIMADB_EXTERNAL_TABLE_ROOTS",
            format!("file://{}", tmp.path().display()),
        )
    };
    let server = PgServer::start(tmp).await.expect("server");
    let client = connect(&server).await;

    let err = client
        .simple_query(
            "CREATE TABLE bad (id INT) \
             WITH (format='parquet', external_location='file:///etc/', authority='external')",
        )
        .await
        .err()
        .expect("CREATE must be rejected when the location is outside the allowlist");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("allowlist") || msg.contains("external"),
        "rejection should cite the external-location allowlist, got: {msg}"
    );

    unsafe { std::env::remove_var("PROXIMADB_EXTERNAL_TABLE_ROOTS") };
}
