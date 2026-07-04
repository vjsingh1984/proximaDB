// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Cross-modal table-function reachability over pgwire — e2e.
//!
//! Proves the routing fix: a `SELECT … FROM <materialized parquet table>
//! JOIN vector_search(…) / timeseries_range(…)` executes through the DataFusion
//! OLAP route over a real pgwire session, instead of declining to the legacy
//! single-table path (which reported "Column '…' does not exist").
//!
//! Two blockers were fixed: (1) `collect_from_table_factor` treated a
//! table-valued function `name(args)` as a catalog table, so catalog resolution
//! declined the query AND the parquet-backed route check failed; skipping
//! args-bearing factors routes it to DataFusion. (2) the shared relational
//! frontend now cleanly declines a table-function so the existing DataFusion
//! `ctx.sql` fallback (where the UDTFs are registered) resolves it.
//!
//! Gated on `datafusion-integration`: the UDTFs + the OLAP route only exist
//! there. Run:
//!   cargo test --features datafusion-integration --test pgwire_crossmodal_join_e2e -- --nocapture
#![cfg(feature = "datafusion-integration")]

use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use tempfile::TempDir;
use tokio::time::sleep;

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

struct PgServer {
    pg_port: u16,
    db: Option<ProximaDB>,
    _tmp: TempDir,
}

impl PgServer {
    async fn start() -> anyhow::Result<Self> {
        let pg_port = free_port();
        let rest_port = free_port();
        let grpc_port = free_port();
        let tmp = TempDir::new()?;
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
            db: Some(db),
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

impl Drop for PgServer {
    fn drop(&mut self) {
        if let Some(mut db) = self.db.take() {
            tokio::spawn(async move {
                let _ = db.shutdown().await;
            });
        }
    }
}

async fn connect(server: &PgServer) -> tokio_postgres::Client {
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    client
}

/// Run a query expected to SUCCEED (no error), returning the first cell as i64.
async fn count(client: &tokio_postgres::Client, sql: &str) -> i64 {
    let msgs = client.simple_query(sql).await.unwrap_or_else(|e| {
        let detail = e
            .as_db_error()
            .map(|d| format!("[{}] {}", d.code().code(), d.message()))
            .unwrap_or_else(|| e.to_string());
        panic!("cross-modal query declined (reachability regression): `{sql}` -> {detail}");
    });
    msgs.into_iter()
        .find_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => {
                r.get(0).and_then(|s| s.parse::<i64>().ok())
            }
            _ => None,
        })
        .unwrap_or(i64::MIN)
}

/// A materialized (parquet-backed) table JOINed with the `vector_search` and
/// `timeseries_range` table functions PLANS + EXECUTES through the DataFusion
/// OLAP route over pgwire — it does not decline to the legacy path. (Row
/// visibility of live-written UDTF data is covered separately; here the fix is
/// that the query is *reachable* and returns a well-formed result rather than a
/// "column does not exist" decline.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crossmodal_udtf_join_reaches_datafusion_over_pgwire() {
    let server = PgServer::start().await.expect("server start");
    let client = connect(&server).await;

    // Unique per run — the native catalog persists table DDL outside the per-test
    // tempdir, so a fixed name collides across runs.
    let wh = format!("wh_{}", server.pg_port);
    client
        .simple_query(&format!("DROP TABLE IF EXISTS {wh}"))
        .await
        .ok();
    client
        .simple_query(&format!("CREATE TABLE {wh} (id TEXT, tier TEXT)"))
        .await
        .expect("create");
    client
        .simple_query(&format!(
            "INSERT INTO {wh} VALUES ('a','private'),('b','retail'),('z','private')"
        ))
        .await
        .expect("insert");
    client
        .simple_query(&format!("ALTER TABLE {wh} MATERIALIZE"))
        .await
        .expect("materialize");

    // Baseline: a plain aggregate on the materialized table succeeds (routes OLAP).
    assert_eq!(
        count(&client, &format!("SELECT count(*) FROM {wh}")).await,
        3
    );

    // The cross-modal join must REACH the DataFusion UDTF, not decline to the legacy
    // single-table path. Before the fix it declined with "Column 'score' does not exist
    // in table '{wh}'". After the fix the query routes to DataFusion, resolves the
    // `vector_search` UDTF, and invokes it — which for a missing collection surfaces a
    // UDTF-INTERNAL error ("vector search: Collection … not found"). That internal error
    // is itself the proof the UDTF was reached; only the "does not exist in table" decline
    // is a reachability regression.
    let vjoin = format!(
        "SELECT count(*) FROM {wh} d JOIN vector_search('sig','[0.1,0.2]',5) v ON d.id = v.id"
    );
    match client.simple_query(&vjoin).await {
        Ok(_) => {} // planned + executed (empty/rows) — reachable
        Err(e) => {
            let msg = e
                .as_db_error()
                .map(|d| d.message().to_string())
                .unwrap_or_else(|| e.to_string());
            assert!(
                !msg.contains("does not exist in table"),
                "vector_search declined to legacy (reachability regression): {msg}"
            );
            assert!(
                msg.contains("vector search") || msg.to_lowercase().contains("collection"),
                "expected a UDTF-internal error proving the UDTF was reached, got: {msg}"
            );
        }
    }

    // `timeseries_range` is graceful on a missing collection (empty), so the join executes
    // cleanly and returns a well-formed count — the end-to-end proof of the DataFusion
    // route + frontend decline → `ctx.sql` fallback → UDTF resolution.
    let tsrange = count(
        &client,
        &format!("SELECT count(*) FROM {wh} d CROSS JOIN timeseries_range('ts', 0, 9999999) t"),
    )
    .await;
    assert!(
        tsrange >= 0,
        "timeseries_range join must execute, got {tsrange}"
    );
}
