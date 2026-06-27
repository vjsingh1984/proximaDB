// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! ADR-025 OLAP read-merge (relational cold path) — pgwire e2e.
//!
//! Proves the central correctness property: after `ALTER TABLE … MATERIALIZE`
//! freezes a cold Parquet snapshot, subsequent `DELETE` / `UPDATE` / `INSERT`
//! over pgwire are reflected in OLAP `SELECT`s that route to the DataFusion
//! engine — and that the behavior is genuinely gated (default-OFF serves the
//! stale snapshot; opt-in reconciles it).
//!
//! Gated on `datafusion-integration`: the read-merge lives on the DataFusion
//! OLAP route, and the gate-OFF assertion (stale snapshot) is only meaningful
//! there — without the feature, parquet-backed tables route to the native
//! engine, which always reads live record state. Run with:
//!   cargo test --features datafusion-integration --test pgwire_olap_delta_merge_e2e -- --nocapture
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
        self.conn_str_for("proximadb")
    }
    /// Connection string scoped to a tenant (the startup `dbname` = tenant/catalog).
    fn conn_str_for(&self, dbname: &str) -> String {
        format!(
            "host=127.0.0.1 port={} user=postgres dbname={dbname} sslmode=disable",
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

fn explain_err(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        format!("[{}] {}", db.code().code(), db.message())
    } else {
        e.to_string()
    }
}

/// Run a 2-column query and parse the rows into `(i64, i64)` pairs (pgwire simple
/// query renders every cell as text).
async fn pairs(client: &tokio_postgres::Client, sql: &str) -> Vec<(i64, i64)> {
    client
        .simple_query(sql)
        .await
        .unwrap_or_else(|e| panic!("query `{sql}`: {}", explain_err(&e)))
        .into_iter()
        .filter_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => Some((
                r.get(0).unwrap_or_default().parse().unwrap_or(i64::MIN),
                r.get(1).unwrap_or_default().parse().unwrap_or(i64::MIN),
            )),
            _ => None,
        })
        .collect()
}

/// Run a single-cell aggregate query and parse the first column as `i64`.
async fn one_i64(client: &tokio_postgres::Client, sql: &str) -> i64 {
    client
        .simple_query(sql)
        .await
        .unwrap_or_else(|e| panic!("query `{sql}`: {}", explain_err(&e)))
        .into_iter()
        .find_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => {
                r.get(0).and_then(|s| s.parse::<i64>().ok())
            }
            _ => None,
        })
        .unwrap_or(i64::MIN)
}

/// Connect a client scoped to a tenant (startup `dbname` = tenant/catalog).
async fn connect(server: &PgServer, dbname: &str) -> tokio_postgres::Client {
    let (client, conn) =
        tokio_postgres::connect(&server.conn_str_for(dbname), tokio_postgres::NoTls)
            .await
            .expect("connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    client
}

/// A3 + A4: after MATERIALIZE, a `DELETE`/`UPDATE`/`INSERT` is invisible to the
/// DataFusion OLAP route with the merge OFF (stale snapshot), and fully reflected
/// with it ON — keyed by canonical oid, with `COUNT(*)` taking the merged
/// cardinality (not the Parquet footer count).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn olap_delta_merge_gate_off_stale_then_on_corrects() {
    let server = PgServer::start().await.expect("server start");
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    // Schema: single-column INT PK (oid = PK text) + a value column.
    client.simple_query("DROP TABLE IF EXISTS dvm").await.ok();
    client
        .simple_query("CREATE TABLE dvm (id INT PRIMARY KEY, v INT)")
        .await
        .unwrap_or_else(|e| panic!("create: {}", explain_err(&e)));
    for sql in [
        "INSERT INTO dvm (id, v) VALUES (1, 10)",
        "INSERT INTO dvm (id, v) VALUES (2, 20)",
        "INSERT INTO dvm (id, v) VALUES (3, 30)",
    ] {
        client
            .simple_query(sql)
            .await
            .unwrap_or_else(|e| panic!("insert `{sql}`: {}", explain_err(&e)));
    }

    // Freeze the cold Parquet snapshot at this LSN, then route SELECTs to OLAP.
    client
        .simple_query("ALTER TABLE dvm MATERIALIZE")
        .await
        .unwrap_or_else(|e| panic!("materialize: {}", explain_err(&e)));

    // Post-snapshot mutations: delete a row, update another, insert a new one.
    client
        .simple_query("DELETE FROM dvm WHERE id = 2")
        .await
        .unwrap_or_else(|e| panic!("delete: {}", explain_err(&e)));
    client
        .simple_query("UPDATE dvm SET v = 99 WHERE id = 3")
        .await
        .unwrap_or_else(|e| panic!("update: {}", explain_err(&e)));
    client
        .simple_query("INSERT INTO dvm (id, v) VALUES (4, 40)")
        .await
        .unwrap_or_else(|e| panic!("insert4: {}", explain_err(&e)));

    // GROUP BY / aggregate engages the relational engine → DataFusion over the
    // materialized Parquet base (never the legacy single-table path).
    let q_rows = "SELECT id, v FROM dvm GROUP BY id, v ORDER BY id";
    let q_count = "SELECT COUNT(*) AS c FROM dvm";

    // Gate OFF (default): the DataFusion route serves the STALE snapshot — this
    // proves the merge is genuinely gated and the legacy behavior is preserved.
    // SAFETY: toggled only between awaited queries — no request is in flight, so
    // no server thread reads this env var concurrently (Rust 2024 marks env
    // mutation unsafe for the general concurrent case). Tests run under nextest
    // process isolation (CLAUDE.md §11).
    unsafe { std::env::remove_var("PROXIMADB_OLAP_DELTA_MERGE") };
    assert_eq!(
        pairs(&client, q_rows).await,
        vec![(1, 10), (2, 20), (3, 30)],
        "gate-off must serve the stale materialized snapshot"
    );

    // Gate ON: the read-merge reconciles delete(2) / update(3→99) / insert(4).
    // SAFETY: see the note above — no in-flight request, nextest-isolated.
    unsafe { std::env::set_var("PROXIMADB_OLAP_DELTA_MERGE", "1") };
    assert_eq!(
        pairs(&client, q_rows).await,
        vec![(1, 10), (3, 99), (4, 40)],
        "gate-on must reflect post-snapshot delete/update/insert keyed by oid"
    );
    assert_eq!(
        one_i64(&client, q_count).await,
        3,
        "COUNT(*) must be the merged cardinality, not the Parquet footer count"
    );

    // SAFETY: toggled only between awaited queries — no request is in flight, so
    // no server thread reads this env var concurrently (Rust 2024 marks env
    // mutation unsafe for the general concurrent case). Tests run under nextest
    // process isolation (CLAUDE.md §11).
    unsafe { std::env::remove_var("PROXIMADB_OLAP_DELTA_MERGE") };
}

/// Tenant isolation of the read-merge (and the empty-delta fast path). Two
/// tenants share a table name; tenant A mutates after MATERIALIZE, tenant B does
/// not. A's OLAP read reflects A's delete/update; B's is UNAFFECTED — B's feed is
/// tenant-scoped (empty), so the fast path serves B's pristine snapshot. A cross-
/// tenant feed leak would surface A's delete/update in B's result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn olap_delta_merge_is_tenant_isolated() {
    // SAFETY: set before the server starts; never mutated while a request is in
    // flight. nextest process isolation (CLAUDE.md §11).
    unsafe { std::env::set_var("PROXIMADB_OLAP_DELTA_MERGE", "1") };
    let server = PgServer::start().await.expect("server start");
    let tenant_a = format!("tenant_a_{}", server.pg_port);
    let tenant_b = format!("tenant_b_{}", server.pg_port);
    let a = connect(&server, &tenant_a).await;
    let b = connect(&server, &tenant_b).await;

    // Both tenants: identical table name + data, each materialized to its own
    // tenant-isolated Parquet snapshot.
    for c in [&a, &b] {
        c.simple_query("CREATE TABLE shared (id INT PRIMARY KEY, v INT)")
            .await
            .unwrap_or_else(|e| panic!("create: {}", explain_err(&e)));
        for sql in [
            "INSERT INTO shared (id, v) VALUES (1, 10)",
            "INSERT INTO shared (id, v) VALUES (2, 20)",
            "INSERT INTO shared (id, v) VALUES (3, 30)",
        ] {
            c.simple_query(sql)
                .await
                .unwrap_or_else(|e| panic!("insert `{sql}`: {}", explain_err(&e)));
        }
        c.simple_query("ALTER TABLE shared MATERIALIZE")
            .await
            .unwrap_or_else(|e| panic!("materialize: {}", explain_err(&e)));
    }

    // Only tenant A mutates after its snapshot.
    a.simple_query("DELETE FROM shared WHERE id = 2")
        .await
        .unwrap_or_else(|e| panic!("A delete: {}", explain_err(&e)));
    a.simple_query("UPDATE shared SET v = 99 WHERE id = 3")
        .await
        .unwrap_or_else(|e| panic!("A update: {}", explain_err(&e)));

    let q = "SELECT id, v FROM shared GROUP BY id, v ORDER BY id";
    assert_eq!(
        pairs(&a, q).await,
        vec![(1, 10), (3, 99)],
        "tenant A must see its own post-snapshot delete(2)/update(3→99)"
    );
    assert_eq!(
        pairs(&b, q).await,
        vec![(1, 10), (2, 20), (3, 30)],
        "tenant B must NOT see tenant A's changes (feed is tenant-isolated)"
    );

    unsafe { std::env::remove_var("PROXIMADB_OLAP_DELTA_MERGE") };
}
