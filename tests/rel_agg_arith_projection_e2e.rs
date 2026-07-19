// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-REL-LOWER-4 — aggregates nested inside a projection expression
//! (`sum(x)/sum(y)`, `100.0*sum(case…)/sum(y)`) must lower on the native
//! route, not error `aggregate function in non-aggregate position`.
//!
//! These are the "aggregate ratio / market-share" shapes: TPC-H Q8
//! (`sum(case when nation=… then vol else 0 end)/sum(vol)`, grouped) and
//! Q14 (`100.0*sum(case when p_type like 'PROMO%' …)/sum(…)`, global).
//! The frontend previously handled only a *bare* top-level aggregate per
//! projection item; an aggregate nested under arithmetic/CASE was declined
//! ("Phase 3"). This drives the split-projection hoist: each nested
//! aggregate is extracted to an Aggregate slot and the surrounding
//! arithmetic becomes a post-aggregate Project expression.

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
async fn rows(client: &tokio_postgres::Client, sql: &str) -> Vec<Vec<String>> {
    let msgs = client.simple_query(sql).await.unwrap_or_else(|e| {
        panic!(
            "{sql}\n  {}",
            e.as_db_error()
                .map(|d| format!("[{}] {}", d.code().code(), d.message()))
                .unwrap_or_else(|| e.to_string())
        )
    });
    msgs.iter()
        .filter_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => Some(
                (0..r.len())
                    .map(|i| r.get(i).unwrap_or("").to_string())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn native_aggregate_arithmetic_projection() {
    std::thread::Builder::new()
        .name("rel-agg-arith-e2e-16m".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("rt")
                .block_on(body())
        })
        .expect("spawn")
        .join()
        .expect("panic");
}

async fn body() {
    let server = PgServer::start().await.expect("server");
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    for ddl in [
        "DROP TABLE IF EXISTS sales",
        "CREATE TABLE sales (id INT PRIMARY KEY, yr INT, nation VARCHAR, vol DOUBLE PRECISION)",
        // 1995: BR=100, total=400 → share 0.25 ; 1996: BR=50, total=100 → 0.5
        "INSERT INTO sales VALUES (1,1995,'BR',100),(2,1995,'US',300),(3,1996,'BR',50),(4,1996,'US',50)",
    ] {
        client.simple_query(ddl).await.expect("ddl");
    }

    // Q8 shape: grouped market-share ratio — an aggregate CASE over another
    // aggregate, in a division, per group, ordered by the group key.
    let got = rows(
        &client,
        "SELECT yr, sum(case when nation = 'BR' then vol else 0 end) / sum(vol) AS share \
         FROM sales GROUP BY yr ORDER BY yr",
    )
    .await;
    assert_eq!(got.len(), 2, "expected one row per year");
    assert_eq!(got[0][0], "1995");
    assert_eq!(got[1][0], "1996");
    let share95: f64 = got[0][1].parse().expect("f64");
    let share96: f64 = got[1][1].parse().expect("f64");
    assert!((share95 - 0.25).abs() < 1e-9, "1995 share = {share95}");
    assert!((share96 - 0.50).abs() < 1e-9, "1996 share = {share96}");

    // Q14 shape: global (no GROUP BY) scaled ratio with a literal factor.
    // 100.0 * (BR total 150) / (grand total 500) = 30.0
    let got2 = rows(
        &client,
        "SELECT 100.0 * sum(case when nation = 'BR' then vol else 0 end) / sum(vol) AS pct \
         FROM sales",
    )
    .await;
    assert_eq!(got2.len(), 1, "global aggregate is a single row");
    let pct: f64 = got2[0][0].parse().expect("f64");
    assert!((pct - 30.0).abs() < 1e-9, "pct = {pct}");

    // Plain difference of two aggregates (no CASE) must also lower.
    let got3 = rows(
        &client,
        "SELECT sum(vol) - sum(case when nation = 'US' then vol else 0 end) AS non_us FROM sales",
    )
    .await;
    let non_us: f64 = got3[0][0].parse().expect("f64");
    assert!((non_us - 150.0).abs() < 1e-9, "non_us = {non_us}");

    eprintln!("✓ TD-REL-LOWER-4 aggregate-arithmetic projections lower on the native route");
}
