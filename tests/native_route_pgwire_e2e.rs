// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Native-over-parquet PRIMARY routing — end-to-end correctness ratchet
//! (TD-OLAP-4 "favor native by operation").
//!
//! Exercises the production native-parquet route end-to-end: a real server, the
//! PostgreSQL wire protocol, and the router — never bypassing pgwire or the engine
//! selector. The native vectorized + route gates are ON
//! (`PROXIMADB_NATIVE_VECTORIZED=1`, `PROXIMADB_NATIVE_ROUTE=1`; both default-off in
//! production), so an UNFILTERED footer-elidable / scalar-aggregate SELECT over a
//! `MATERIALIZE`d (Parquet-backed) table routes to
//! `relational_pipeline::try_native_over_parquet` and is served by the native
//! vectorized engine as the PRIMARY backend — with DataFusion the correctness floor.
//!
//! The central correctness property: the native-served result MUST equal
//! DataFusion's for the routed shapes. We prove this WITHIN one process (so no
//! `OnceLock` gate flip is needed) by pairing each aggregate with a tautological
//! filter: the UNFILTERED form routes to native (no `Scan.predicate`, elidable /
//! narrow scan), while the SAME aggregate under `WHERE id >= <min>` (which every row
//! satisfies) carries a `Scan.predicate` — native has no predicate pushdown, so it
//! declines and DataFusion serves it. Equal results ⇒ native == DataFusion. Each is
//! additionally asserted against the hand-computed ANSI-correct value.
//!
//! Run with the real server-side cause behind any opaque pgwire `db error`:
//!   RUST_LOG=proximadb=debug cargo nextest run --test native_route_pgwire_e2e -- --nocapture

use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use proximadb::query::route_cost_model::GLOBAL_ROUTE_COST_MODEL;
use tempfile::TempDir;
use tokio::time::sleep;
use tokio_postgres::{Client, SimpleQueryMessage};

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
        // Turn on the native vectorized path AND the "favor native by operation"
        // route BEFORE the server (and its `OnceLock`-cached gate reads) come up.
        // nextest runs each test in its own process, so this env mutation cannot
        // leak into other tests.
        // SAFETY: single-threaded test setup, before any server thread starts.
        unsafe {
            std::env::set_var("PROXIMADB_NATIVE_VECTORIZED", "1");
            std::env::set_var("PROXIMADB_NATIVE_ROUTE", "1");
            // Decode row-groups in parallel too — additive, never a correctness dep.
            std::env::set_var("PROXIMADB_NATIVE_MORSEL", "1");
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

/// Run a single-column scalar-aggregate SELECT and return the one cell as text.
async fn scalar(client: &Client, sql: &str) -> String {
    let messages = client
        .simple_query(sql)
        .await
        .unwrap_or_else(|e| panic!("query `{sql}`: {}", explain_err(&e)));
    for m in &messages {
        if let SimpleQueryMessage::Row(r) = m {
            return r
                .get(0)
                .map(|s| s.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }
    }
    panic!("query `{sql}` returned no row");
}

/// Parse a numeric scalar cell to f64 (aggregate results may render as `30` or
/// `30.0` depending on the engine's output type; comparing as f64 is
/// representation-insensitive).
fn num(s: &str) -> f64 {
    s.parse::<f64>()
        .unwrap_or_else(|_| panic!("expected numeric scalar, got `{s}`"))
}

/// The native-over-parquet PRIMARY route must return exactly DataFusion's answer for
/// COUNT(*)/MIN/MAX (footer-elidable) and SUM/AVG (scalar-aggregate). Proven by the
/// unfiltered (native) vs tautological-filtered (DataFusion) pairing described in the
/// module docs, plus hand-computed ANSI-correct expected values.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_route_aggregates_equal_datafusion_over_pgwire() {
    let server = PgServer::start().await.expect("server start");
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), tokio_postgres::NoTls)
        .await
        .expect("tokio-postgres connect");
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("pgwire connection error: {e}");
        }
    });

    // Unique table name so parallel/repeat runs never collide in the catalog.
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let t = format!("nr_{suffix}");

    client
        .simple_query(&format!(
            "CREATE TABLE {t} (id INT PRIMARY KEY, v INT, w INT)"
        ))
        .await
        .unwrap_or_else(|e| panic!("CREATE: {}", explain_err(&e)));

    // Deterministic, NULL-free data with a known min id of 1 so `WHERE id >= 1` is a
    // tautology (matches every row) — the filtered aggregate DataFusion computes
    // equals the unfiltered aggregate native computes.
    //   COUNT(*)=5  MIN(v)=10  MAX(v)=50  SUM(v)=150  AVG(v)=30
    for (id, v, w) in [
        (1, 10, 100),
        (2, 20, 200),
        (3, 30, 300),
        (4, 40, 400),
        (5, 50, 500),
    ] {
        client
            .simple_query(&format!(
                "INSERT INTO {t} (id, v, w) VALUES ({id}, {v}, {w})"
            ))
            .await
            .unwrap_or_else(|e| panic!("INSERT: {}", explain_err(&e)));
    }

    // MATERIALIZE → Parquet-backed → OLAP route reaches DataFusion (and, for the
    // eligible unfiltered shapes, native-over-parquet as PRIMARY).
    client
        .simple_query(&format!("ALTER TABLE {t} MATERIALIZE"))
        .await
        .unwrap_or_else(|e| panic!("MATERIALIZE: {}", explain_err(&e)));
    sleep(Duration::from_millis(500)).await;

    // (aggregate, unfiltered→native, tautological-filtered→DataFusion, expected).
    // The filter `id >= 1` matches all rows, so the DataFusion answer equals the
    // unfiltered native answer AND the hand-computed value.
    let cases = [
        ("COUNT(*)", 5.0_f64),
        ("MIN(v)", 10.0),
        ("MAX(v)", 50.0),
        ("SUM(v)", 150.0),
        ("AVG(v)", 30.0),
    ];
    for (agg, expected) in cases {
        let native = scalar(&client, &format!("SELECT {agg} FROM {t}")).await;
        let datafusion = scalar(&client, &format!("SELECT {agg} FROM {t} WHERE id >= 1")).await;
        assert_eq!(
            num(&native),
            expected,
            "native-routed {agg} must equal the ANSI-correct value"
        );
        assert_eq!(
            num(&native),
            num(&datafusion),
            "native-routed {agg} ({native}) must equal DataFusion's ({datafusion})"
        );
    }

    // MIN/MAX over a second column via footer elision — same equality property.
    for (agg, expected) in [("MIN(w)", 100.0_f64), ("MAX(w)", 500.0)] {
        let native = scalar(&client, &format!("SELECT {agg} FROM {t}")).await;
        let datafusion = scalar(&client, &format!("SELECT {agg} FROM {t} WHERE id >= 1")).await;
        assert_eq!(num(&native), expected, "native-routed {agg}");
        assert_eq!(
            num(&native),
            num(&datafusion),
            "native-routed {agg} ({native}) must equal DataFusion's ({datafusion})"
        );
    }

    // Non-vacuity guard: prove the unfiltered aggregates were ACTUALLY served by the
    // native engine (not silently declined to DataFusion, which would make the
    // equality checks above pass trivially with DataFusion == DataFusion). The pgwire
    // query boundary wraps each query in an `io_trace::instrument` scope that feeds
    // the route observer (installed at startup) the stamped route; the native
    // over-parquet path stamps `("vectorized", "NativeVectorized")`. So a learned
    // cost-model cell with the `NativeVectorized` backend label exists iff native
    // served at least one query as PRIMARY.
    let served_native = GLOBAL_ROUTE_COST_MODEL
        .learned_cell_keys()
        .iter()
        .any(|(_class, backend)| backend == "NativeVectorized");
    assert!(
        served_native,
        "native-over-parquet route never engaged — the equality checks would be \
         vacuous (DataFusion == DataFusion). Learned cost-model cells: {:?}",
        GLOBAL_ROUTE_COST_MODEL.learned_cell_keys()
    );
}
