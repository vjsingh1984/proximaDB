// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-EXEC-CANCEL-1 — pgwire CancelRequest actually cancels a running query.
//!
//! Before this fix the server rejected CancelRequest connections outright and
//! the Volcano executor never observed `ExecutionControls::cancellation_flag`,
//! so a grinding query (e.g. an un-normalized cross product, TD-REL-LOWER-2)
//! kept burning CPU and queued every subsequent query on the session. This
//! eval drives the REAL wire path: `BackendKeyData` → out-of-band
//! `CancelRequest` (tokio_postgres `cancel_token`) → registry → cooperative
//! flag → `CancelCheckExec` strided check+yield → SQLSTATE `57014` — and
//! asserts the session answers promptly afterwards.
//!
//!   RUST_LOG=proximadb=debug cargo test --test pgwire_cancel_e2e -- --nocapture

use std::net::TcpListener;
use std::time::{Duration, Instant};

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

/// Minimal in-process pgwire server (mirrors `route_cost_override_pgwire_eval`).
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

/// MULTI-thread runtime (unlike the sibling evals' current-thread), 16 MiB
/// worker stacks (the TD-ROUTE-2 pgwire-lowering pin). Multi-thread matters:
/// the CancelRequest arrives on a second connection that must be serviced
/// WHILE the grinder runs — production servers are multi-thread, and this test
/// models that. The executor's cooperative `yield_now` (the fix under test)
/// then lets the cancel handler run even under a synchronous grind.
#[test]
fn cancel_request_stops_a_grinding_native_query() {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .thread_stack_size(16 * 1024 * 1024)
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(eval_body());
}

async fn eval_body() {
    let server = PgServer::start().await.expect("server start");
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), tokio_postgres::NoTls)
        .await
        .expect("connect");
    let conn_task = tokio::spawn(async move {
        let _ = conn.await;
    });

    // Three 2k-row tables. The 3-way cross product is 8×10⁹ predicate
    // evaluations — minutes of Volcano grind (a 2-way 25M-pair product
    // completes UNDER the cancel window in the dev profile, so 2-way is not a
    // reliable grinder) — behind a never-true filter that can never be
    // rewritten into an equi-join (future-proof against TD-REL-LOWER-2 making
    // equality cross-joins fast).
    for ddl in [
        "DROP TABLE IF EXISTS ta",
        "DROP TABLE IF EXISTS tb",
        "DROP TABLE IF EXISTS tc",
        "CREATE TABLE ta (a_id INT PRIMARY KEY, a_val INT)",
        "CREATE TABLE tb (b_id INT PRIMARY KEY, b_val INT)",
        "CREATE TABLE tc (c_id INT PRIMARY KEY, c_val INT)",
    ] {
        client.simple_query(ddl).await.expect("ddl");
    }
    for (table, prefix) in [("ta", "a"), ("tb", "b"), ("tc", "c")] {
        for batch in 0..4 {
            let rows: Vec<String> = (0..500)
                .map(|i| {
                    let id = batch * 500 + i;
                    format!("({id}, 0)")
                })
                .collect();
            client
                .simple_query(&format!(
                    "INSERT INTO {table} ({prefix}_id, {prefix}_val) VALUES {}",
                    rows.join(",")
                ))
                .await
                .expect("insert");
        }
    }

    // The grinder: engages the relational route (aggregate + comma-join), and
    // the predicate is never true, so Volcano must enumerate all 8×10⁹
    // combinations to answer — unless cancelled.
    let grinder = "SELECT count(*) FROM ta JOIN tb ON a_val = b_val JOIN tc ON b_val = c_val";

    let cancel_token = client.cancel_token();
    let started = Instant::now();
    let result = {
        let query = client.simple_query(grinder);
        tokio::pin!(query);
        // Let the query get well into execution, then fire the out-of-band cancel.
        tokio::select! {
            r = &mut query => r, // finished before we cancelled — see the assert below
            _ = sleep(Duration::from_millis(1500)) => {
                cancel_token
                    .cancel_query(tokio_postgres::NoTls)
                    .await
                    .expect("cancel connection");
                let cancelled_at = Instant::now();
                let r = tokio::time::timeout(Duration::from_secs(15), &mut query)
                    .await
                    .expect("query must return promptly after CancelRequest — the cooperative flag/executor check is not firing");
                let latency = cancelled_at.elapsed();
                eprintln!("✓ query returned {latency:?} after CancelRequest");
                assert!(
                    latency < Duration::from_secs(10),
                    "cancellation latency too high: {latency:?}"
                );
                r
            }
        }
    };

    // The grinder must have been cancelled, not completed: a completion here
    // means the fixture is too small to grind (raise the row counts).
    let err = result.expect_err(
        "grinder completed before the cancel fired — fixture no longer grinds; \
         enlarge it so the cancel path is actually exercised",
    );
    let db_err = err.as_db_error().expect("expected a server-side error");
    assert_eq!(
        db_err.code().code(),
        "57014",
        "expected SQLSTATE 57014 (query_canceled), got [{}] {}",
        db_err.code().code(),
        db_err.message()
    );
    eprintln!(
        "✓ grinder cancelled after {:?} total: [{}] {}",
        started.elapsed(),
        db_err.code().code(),
        db_err.message()
    );

    // The session must be immediately usable — the exact failure mode the TD
    // measured was every follow-on query queueing behind the un-cancelled
    // grind.
    let follow_up = tokio::time::timeout(
        Duration::from_secs(5),
        client.simple_query("SELECT count(*) FROM ta"),
    )
    .await
    .expect("session unresponsive after cancel — the grind is still running")
    .expect("follow-up query failed");
    let rows = follow_up
        .iter()
        .filter(|m| matches!(m, tokio_postgres::SimpleQueryMessage::Row(_)))
        .count();
    assert_eq!(rows, 1);
    eprintln!("✓ session answers promptly after the cancel");
    drop(client);
    let _ = conn_task.await;
}
