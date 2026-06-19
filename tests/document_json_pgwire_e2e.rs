//! Document (SQL/JSON) over pgwire — conformance harness (first cut).
//!
//! Documents are modeled as a relational table with a JSON column, queried over
//! the PostgreSQL wire protocol — the open, standard way to store + query
//! semi-structured documents in a SQL database. Same model as the TPC-H/TPC-DS
//! harnesses: CREATE → INSERT → MATERIALIZE → query one by one, routed by
//! ProximaDB. Each query that executes cleanly counts toward `DOC_RATCHET`.
//!
//!   RUST_LOG=proximadb=debug cargo test --test document_json_pgwire_e2e -- --nocapture

use std::net::TcpListener;
use std::time::Duration;

use proximadb::core::Config;
use proximadb::database::ProximaDB;
use tempfile::TempDir;
use tokio::time::sleep;

/// Document/JSON queries expected to execute cleanly over pgwire (the ratchet).
const DOC_RATCHET: usize = 11;

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
    "docs",
    "CREATE TABLE docs (doc_id INT PRIMARY KEY, kind VARCHAR, doc JSON)",
)];

const DATA: &[&str] = &[
    "INSERT INTO docs (doc_id, kind, doc) VALUES (1, 'product', '{\"title\":\"alpha\",\"price\":10,\"in_stock\":true,\"tags\":[\"x\",\"y\"],\"author\":{\"name\":\"Ann\",\"country\":\"CA\"}}')",
    "INSERT INTO docs (doc_id, kind, doc) VALUES (2, 'product', '{\"title\":\"beta\",\"price\":25,\"in_stock\":false,\"tags\":[\"y\",\"z\"],\"author\":{\"name\":\"Bob\",\"country\":\"US\"}}')",
    "INSERT INTO docs (doc_id, kind, doc) VALUES (3, 'article', '{\"title\":\"gamma\",\"price\":5,\"in_stock\":true,\"tags\":[\"x\"],\"author\":{\"name\":\"Ann\",\"country\":\"CA\"}}')",
];

/// Document/JSON queries probing increasing capability. Numbered for reporting.
fn doc_queries() -> Vec<(&'static str, String)> {
    vec![
        // Baseline: store + retrieve the whole JSON document.
        ("d01_select_all", "select doc_id, kind, doc from docs order by doc_id".to_string()),
        // Filter on a normal relational column alongside the JSON blob.
        ("d02_filter_col", "select doc_id, doc from docs where kind = 'product' order by doc_id".to_string()),
        ("d03_count_by_kind", "select kind, count(*) as n from docs group by kind order by kind".to_string()),
        // JSON scalar field extraction (-> as json, ->> as text).
        ("d04_arrow_text", "select doc_id, doc->>'title' as title from docs order by doc_id".to_string()),
        ("d05_arrow_json", "select doc_id, doc->'price' as price from docs order by doc_id".to_string()),
        // Filter on an extracted JSON scalar.
        ("d06_filter_json_scalar", "select doc_id from docs where (doc->>'price')::int > 8 order by doc_id".to_string()),
        ("d07_filter_json_text", "select doc_id from docs where doc->>'title' = 'alpha'".to_string()),
        // Nested path.
        ("d08_nested_path", "select doc_id, doc->'author'->>'name' as author from docs order by doc_id".to_string()),
        // Group by an extracted JSON field.
        ("d09_group_by_json", "select doc->'author'->>'country' as country, count(*) as n from docs group by doc->'author'->>'country' order by country".to_string()),
        // json_extract_path_text function form (portable alternative to ->>).
        ("d10_json_extract_fn", "select doc_id, json_extract_path_text(doc, 'title') as title from docs order by doc_id".to_string()),
        // Boolean field extraction.
        ("d11_bool_field", "select doc_id from docs where (doc->>'in_stock')::boolean = true order by doc_id".to_string()),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn document_json_pgwire_conformance() {
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

    for sql in DATA {
        client
            .simple_query(sql)
            .await
            .unwrap_or_else(|e| panic!("INSERT failed: {}\n  sql: {sql}", explain_err(&e)));
    }
    eprintln!("✓ data: {} documents loaded", DATA.len());

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

    let queries = doc_queries();
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
        "\n=== Document JSON pgwire conformance: {}/{} passed (ratchet {}) ===",
        passed.len(),
        queries.len(),
        DOC_RATCHET
    );
    if !failed.is_empty() {
        eprintln!("failing:");
        for (id, err) in &failed {
            eprintln!("  {id}: {err}");
        }
    }

    assert!(
        passed.len() >= DOC_RATCHET,
        "Document JSON conformance regressed: {} passed < ratchet {}",
        passed.len(),
        DOC_RATCHET
    );

    // Value-correctness: GROUP BY a nested JSON field. The seeded authors are
    // Ann/CA, Bob/US, Ann/CA → grouping by author.country yields CA=2, US=1.
    // Proves JSON extraction + aggregation compute correct values on the OLAP
    // route, not merely that the SQL runs.
    let rows: Vec<_> = client
        .simple_query(
            "select doc->'author'->>'country' as country, count(*) as n from docs \
             group by doc->'author'->>'country' order by country",
        )
        .await
        .expect("group-by-json re-run")
        .into_iter()
        .filter_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => Some(r),
            _ => None,
        })
        .collect();
    let got: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            (
                r.get(0).unwrap_or("").to_string(),
                r.get(1).unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("CA".to_string(), "2".to_string()),
            ("US".to_string(), "1".to_string())
        ],
        "GROUP BY nested JSON field should yield CA=2, US=1"
    );
    eprintln!("✓ value-correctness: GROUP BY author.country = CA:2, US:1");
}
