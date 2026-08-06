// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Native hash join over pgwire — end-to-end correctness ratchet (ADR-054 Phase 3,
//! TD-OLAP-11).
//!
//! Exercises the NATIVE hash-join path end-to-end: a real server, the PostgreSQL
//! wire protocol, and the router — never bypassing pgwire or the engine selector.
//! The native vectorized + join gates are ON (`PROXIMADB_NATIVE_VECTORIZED=1`,
//! `PROXIMADB_NATIVE_JOIN=1`; both default-off in production), so a JOIN over
//! non-materialized relational tables routes through
//! `NativeVolcanoEngine::execute_physical` → `try_vectorized` → the native
//! `HashJoin{Build,Probe}Operator`.
//!
//! Shadow mode is deliberately OFF here: this test asserts the engine's ACTUAL
//! output against the hand-computed SQL-correct result, so a native-join bug
//! fails the test. (Enabling `PROXIMADB_NATIVE_JOIN_SHADOW` would mask a bug by
//! failing safe to the Volcano result — that mode is validated separately.)
//! Tables are NOT materialized on purpose: `MATERIALIZE` makes a table
//! Parquet-backed and routes it to DataFusion's OLAP join, which is a different
//! engine. The TD-OLAP-11 native join serves the non-Parquet relational path.
//!
//! Honest scope (what this ratchets vs. what it depends on): this is a JOIN
//! correctness ratchet over the real pgwire→router path with the native gates
//! ENABLED. Whether the native `HashJoin*Operator` or the Volcano fallback
//! actually serves a given query depends on the `PhysicalPlan::Scan`-leaf lowering
//! in `native_ops::lower_physical` (TD-OLAP-14, in flight): a freshly-created
//! relational table declines to the Volcano until scan lowering covers it. Either
//! way the RESULT must be correct (that is the ratchet), and this test starts
//! exercising the native operator automatically the moment scan lowering engages.
//! The native operator itself is proven directly by the unit tests in
//! `native_ops.rs`; this fills the end-to-end gap those unit tests can't cover.
//!
//! The dataset mirrors the Volcano baseline in `pgwire_relational_engine_e2e.rs`
//! (dept 3 `hr` = unmatched right; emp 13 `dan` dept_id 99 = unmatched left) so
//! every join kind has both matched and unmatched rows.
//!
//! Run with the real server-side cause behind any opaque pgwire `db error`:
//!   RUST_LOG=proximadb=debug cargo nextest run --test native_join_pgwire_e2e -- --nocapture

use std::collections::BTreeSet;
use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use tempfile::TempDir;
use tokio::time::sleep;
use tokio_postgres::{SimpleQueryMessage, SimpleQueryRow};

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
        // Enable the native vectorized + hash-join gates BEFORE the server (and its
        // `OnceLock`-cached gate reads) come up. nextest runs each test in its own
        // process, so this env mutation cannot leak into other tests.
        // SAFETY: single-threaded test setup, before any server thread starts.
        unsafe {
            std::env::set_var("PROXIMADB_NATIVE_VECTORIZED", "1");
            std::env::set_var("PROXIMADB_NATIVE_JOIN", "1");
        }

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

/// Render a tokio_postgres error including the real server-side DbError cause.
fn explain_err(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        format!("[{}] {}", db.code().code(), db.message())
    } else {
        e.to_string()
    }
}

/// One cell as text; a SQL NULL renders as the literal `NULL` (so NULL-extended
/// join rows are compared explicitly rather than silently dropped).
fn cell(row: &SimpleQueryRow, col: &str) -> String {
    row.get(col)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "NULL".to_string())
}

/// The `(a, b)` column pairs across all result rows, as an order-insensitive set
/// (join output order is not guaranteed).
fn pairs(messages: &[SimpleQueryMessage], a: &str, b: &str) -> BTreeSet<(String, String)> {
    messages
        .iter()
        .filter_map(|m| match m {
            SimpleQueryMessage::Row(r) => Some((cell(r, a), cell(r, b))),
            _ => None,
        })
        .collect()
}

fn expected(items: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    items
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_hash_join_over_pgwire_matches_expected() {
    let server = PgServer::start().await.expect("server start");
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), tokio_postgres::NoTls)
        .await
        .expect("tokio-postgres connect");
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("pgwire connection error: {e}");
        }
    });

    // Unique table names so parallel/repeat runs never collide in the catalog.
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dept = format!("dept_{suffix}");
    let emp = format!("emp_{suffix}");

    client
        .simple_query(&format!(
            "CREATE TABLE {dept} (id BIGINT PRIMARY KEY, dname VARCHAR)"
        ))
        .await
        .unwrap_or_else(|e| panic!("CREATE dept: {}", explain_err(&e)));
    client
        .simple_query(&format!(
            "CREATE TABLE {emp} (id BIGINT PRIMARY KEY, dept_id BIGINT, ename VARCHAR)"
        ))
        .await
        .unwrap_or_else(|e| panic!("CREATE emp: {}", explain_err(&e)));

    // dept 3 (hr) has no employee → unmatched RIGHT row for RIGHT/FULL.
    for (id, dname) in [(1, "eng"), (2, "sales"), (3, "hr")] {
        client
            .simple_query(&format!(
                "INSERT INTO {dept} (id, dname) VALUES ({id}, '{dname}')"
            ))
            .await
            .unwrap_or_else(|e| panic!("INSERT dept: {}", explain_err(&e)));
    }
    // emp 13 (dan) has dept_id 99 (no such dept) → unmatched LEFT row for LEFT/FULL.
    for (id, dept_id, ename) in [
        (10, 1, "ann"),
        (11, 1, "bob"),
        (12, 2, "cas"),
        (13, 99, "dan"),
    ] {
        client
            .simple_query(&format!(
                "INSERT INTO {emp} (id, dept_id, ename) VALUES ({id}, {dept_id}, '{ename}')"
            ))
            .await
            .unwrap_or_else(|e| panic!("INSERT emp: {}", explain_err(&e)));
    }

    sleep(Duration::from_millis(500)).await;

    // INNER: only matched (dept_id = dept.id) pairs.
    let rows = client
        .simple_query(&format!(
            "SELECT ename, dname FROM {emp} JOIN {dept} ON {emp}.dept_id = {dept}.id"
        ))
        .await
        .unwrap_or_else(|e| panic!("INNER JOIN: {}", explain_err(&e)));
    assert_eq!(
        pairs(&rows, "ename", "dname"),
        expected(&[("ann", "eng"), ("bob", "eng"), ("cas", "sales")]),
        "native INNER JOIN emp×dept"
    );

    // LEFT: all emp; the unmatched-left `dan` gets a NULL dname.
    let rows = client
        .simple_query(&format!(
            "SELECT ename, dname FROM {emp} LEFT JOIN {dept} ON {emp}.dept_id = {dept}.id"
        ))
        .await
        .unwrap_or_else(|e| panic!("LEFT JOIN: {}", explain_err(&e)));
    assert_eq!(
        pairs(&rows, "ename", "dname"),
        expected(&[
            ("ann", "eng"),
            ("bob", "eng"),
            ("cas", "sales"),
            ("dan", "NULL"),
        ]),
        "native LEFT JOIN keeps unmatched-left row with NULL-padded build column"
    );

    // RIGHT: all dept; the unmatched-right `hr` gets a NULL ename (build-drain path).
    let rows = client
        .simple_query(&format!(
            "SELECT ename, dname FROM {emp} RIGHT JOIN {dept} ON {emp}.dept_id = {dept}.id"
        ))
        .await
        .unwrap_or_else(|e| panic!("RIGHT JOIN: {}", explain_err(&e)));
    assert_eq!(
        pairs(&rows, "ename", "dname"),
        expected(&[
            ("ann", "eng"),
            ("bob", "eng"),
            ("cas", "sales"),
            ("NULL", "hr"),
        ]),
        "native RIGHT JOIN drains unmatched-right (build) row with NULL-padded probe column"
    );

    // FULL: matched pairs + unmatched-left (`dan`) + unmatched-right (`hr`).
    let rows = client
        .simple_query(&format!(
            "SELECT ename, dname FROM {emp} FULL JOIN {dept} ON {emp}.dept_id = {dept}.id"
        ))
        .await
        .unwrap_or_else(|e| panic!("FULL JOIN: {}", explain_err(&e)));
    assert_eq!(
        pairs(&rows, "ename", "dname"),
        expected(&[
            ("ann", "eng"),
            ("bob", "eng"),
            ("cas", "sales"),
            ("dan", "NULL"),
            ("NULL", "hr"),
        ]),
        "native FULL JOIN emits matched pairs + both unmatched sides"
    );
}
