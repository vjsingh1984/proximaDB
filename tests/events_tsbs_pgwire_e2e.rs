//! Events / time-series (TSBS-style) over pgwire — conformance harness.
//!
//! Models the TSBS "DevOps" workload: a `cpu` metrics table (timestamp + host/region
//! tags + gauges), queried over the PostgreSQL wire protocol with the standard TSBS
//! query shapes (single/double group-by, last-point, high-cpu threshold,
//! groupby-orderby-limit, time-bucketed aggregation). Same harness model as the
//! TPC-H/TPC-DS suites: CREATE → INSERT → MATERIALIZE → query one by one, routed by
//! ProximaDB to the DataFusion OLAP engine. Each query that executes cleanly counts
//! toward `TSBS_RATCHET`.
//!
//!   RUST_LOG=proximadb=debug cargo test --test events_tsbs_pgwire_e2e -- --nocapture

use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use tempfile::TempDir;
use tokio::time::sleep;

const TSBS_RATCHET: usize = 10;

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

fn explain_err(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        format!("[{}] {}", db.code().code(), db.message())
    } else {
        e.to_string()
    }
}

const SCHEMA: &[(&str, &str)] = &[(
    "cpu",
    "CREATE TABLE cpu (ts TIMESTAMP, hostname VARCHAR, region VARCHAR, datacenter VARCHAR, usage_user DOUBLE PRECISION, usage_system DOUBLE PRECISION, usage_idle DOUBLE PRECISION)",
)];

/// Two hosts, two regions, several timestamps across two hours.
fn data() -> Vec<String> {
    let rows = [
        (
            "2016-01-01 00:00:00",
            "host_0",
            "us-east",
            "us-east-1a",
            25.0,
            10.0,
            65.0,
        ),
        (
            "2016-01-01 00:10:00",
            "host_0",
            "us-east",
            "us-east-1a",
            35.0,
            12.0,
            53.0,
        ),
        (
            "2016-01-01 00:20:00",
            "host_0",
            "us-east",
            "us-east-1a",
            95.0,
            20.0,
            5.0,
        ),
        (
            "2016-01-01 01:00:00",
            "host_0",
            "us-east",
            "us-east-1a",
            40.0,
            15.0,
            45.0,
        ),
        (
            "2016-01-01 00:00:00",
            "host_1",
            "us-west",
            "us-west-2b",
            50.0,
            30.0,
            20.0,
        ),
        (
            "2016-01-01 00:10:00",
            "host_1",
            "us-west",
            "us-west-2b",
            92.0,
            35.0,
            3.0,
        ),
        (
            "2016-01-01 00:20:00",
            "host_1",
            "us-west",
            "us-west-2b",
            60.0,
            25.0,
            15.0,
        ),
        (
            "2016-01-01 01:00:00",
            "host_1",
            "us-west",
            "us-west-2b",
            70.0,
            28.0,
            12.0,
        ),
    ];
    let values: Vec<String> = rows
        .iter()
        .map(|(ts, h, r, dc, u, s, i)| {
            format!("(TIMESTAMP '{ts}', '{h}', '{r}', '{dc}', {u}, {s}, {i})")
        })
        .collect();
    vec![format!(
        "INSERT INTO cpu (ts, hostname, region, datacenter, usage_user, usage_system, usage_idle) VALUES {}",
        values.join(", ")
    )]
}

/// TSBS DevOps-style query shapes.
fn tsbs_queries() -> Vec<(&'static str, String)> {
    vec![
        // single-groupby-1-1-1: max usage_user for one host over a time range.
        ("single_groupby", "select date_trunc('hour', ts) as hour, max(usage_user) as max_user from cpu where hostname = 'host_0' and ts >= TIMESTAMP '2016-01-01 00:00:00' and ts < TIMESTAMP '2016-01-01 02:00:00' group by date_trunc('hour', ts) order by hour".to_string()),
        // double-groupby: avg of every metric per host per hour.
        ("double_groupby", "select date_trunc('hour', ts) as hour, hostname, avg(usage_user) as au, avg(usage_system) as as_, avg(usage_idle) as ai from cpu group by date_trunc('hour', ts), hostname order by hour, hostname".to_string()),
        // cpu-max-all: max of all gauges per host.
        ("max_all", "select hostname, max(usage_user) as mu, max(usage_system) as ms, max(usage_idle) as mi from cpu group by hostname order by hostname".to_string()),
        // high-cpu: rows where usage_user > threshold.
        ("high_cpu", "select ts, hostname, usage_user from cpu where usage_user > 90.0 order by usage_user desc, hostname".to_string()),
        // high-cpu per region (filter + group).
        ("high_cpu_by_region", "select region, count(*) as n from cpu where usage_user > 90.0 group by region order by region".to_string()),
        // groupby-orderby-limit: top-N time buckets by max usage.
        ("groupby_orderby_limit", "select date_trunc('hour', ts) as hour, max(usage_user) as max_user from cpu group by date_trunc('hour', ts) order by max_user desc limit 5".to_string()),
        // last-point: latest reading per host (window).
        ("last_point", "select hostname, ts, usage_user from (select hostname, ts, usage_user, row_number() over (partition by hostname order by ts desc) as rn from cpu) t where rn = 1 order by hostname".to_string()),
        // EXTRACT over the time column (datetime expr).
        ("extract_hour", "select extract(hour from ts) as hr, count(*) as n from cpu group by extract(hour from ts) order by hr".to_string()),
        // group by tag + region with HAVING.
        ("region_having", "select region, avg(usage_user) as au from cpu group by region having avg(usage_user) > 40.0 order by region".to_string()),
        // moving aggregate (window frame over time per host).
        ("windowed_avg", "select hostname, ts, avg(usage_user) over (partition by hostname order by ts rows between 1 preceding and current row) as moving_avg from cpu order by hostname, ts".to_string()),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn events_tsbs_pgwire_conformance() {
    let server = PgServer::start().await.expect("server start");
    let (client, conn) = tokio_postgres::connect(&server.conn_str(), tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    for (name, ddl) in SCHEMA {
        let _ = client
            .simple_query(&format!("DROP TABLE IF EXISTS {name}"))
            .await;
        client
            .simple_query(ddl)
            .await
            .unwrap_or_else(|e| panic!("CREATE {name}: {}", explain_err(&e)));
    }
    eprintln!("✓ schema: {} tables created", SCHEMA.len());

    for sql in data() {
        client
            .simple_query(&sql)
            .await
            .unwrap_or_else(|e| panic!("INSERT failed: {}\n  sql: {sql}", explain_err(&e)));
    }
    eprintln!("✓ data: metrics loaded");

    let mut materialized = 0;
    for (name, _) in SCHEMA {
        match client
            .simple_query(&format!("ALTER TABLE {name} MATERIALIZE"))
            .await
        {
            Ok(_) => materialized += 1,
            Err(e) => eprintln!("  · MATERIALIZE {name}: {}", explain_err(&e)),
        }
    }
    eprintln!("✓ materialize: {materialized}/{} tables", SCHEMA.len());

    let queries = tsbs_queries();
    let mut passed = Vec::new();
    let mut failed = Vec::new();
    for (id, sql) in &queries {
        match client.simple_query(sql).await {
            Ok(_) => {
                eprintln!("  ✓ {id}");
                passed.push(*id);
            }
            Err(e) => {
                eprintln!("  ✗ {id}: {}", explain_err(&e));
                failed.push((*id, explain_err(&e)));
            }
        }
    }

    eprintln!(
        "\n=== Events TSBS pgwire conformance: {}/{} passed (ratchet {}) ===",
        passed.len(),
        queries.len(),
        TSBS_RATCHET
    );
    if !failed.is_empty() {
        eprintln!("failing:");
        for (id, err) in &failed {
            eprintln!("  {id}: {err}");
        }
    }

    assert!(
        passed.len() >= TSBS_RATCHET,
        "Events TSBS conformance regressed: {} passed < ratchet {}",
        passed.len(),
        TSBS_RATCHET
    );

    // Value-correctness: high-CPU rows (usage_user > 90) are host_0@00:20 (95.0)
    // and host_1@00:10 (92.0). Proves time-series filtering + ordering over the
    // materialized Parquet computes correct values, not just that the SQL runs.
    let rows: Vec<_> = client
        .simple_query(
            "select hostname, usage_user from cpu where usage_user > 90.0 \
             order by usage_user desc, hostname",
        )
        .await
        .expect("high-cpu re-run")
        .into_iter()
        .filter_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => Some(r),
            _ => None,
        })
        .collect();
    assert_eq!(rows.len(), 2, "two readings exceed 90% usage_user");
    assert_eq!(rows[0].get(0).unwrap_or(""), "host_0", "top high-cpu host");
    assert_eq!(
        rows[1].get(0).unwrap_or(""),
        "host_1",
        "second high-cpu host"
    );
    eprintln!("✓ value-correctness: high-cpu = host_0(95), host_1(92)");
}
