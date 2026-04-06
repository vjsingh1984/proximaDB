// Query Facade Routing Benchmarks
// Measures overhead and performance of UnifiedQueryFacade routing

mod common;
use common::benchmark_utils::print_system_info;

use anyhow::Result;
use async_trait::async_trait;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use proximadb::query::facade::{
    QueryContext, QueryRequest, QueryResult, QueryResultData, QueryStrategy, QueryType,
    UnifiedQueryFacade,
};
use std::sync::Arc;
use std::time::Duration;

/// Mock strategy for benchmarking facade routing overhead
struct MockVectorStrategy;

#[async_trait]
impl QueryStrategy for MockVectorStrategy {
    fn name(&self) -> &str {
        "mock_vector"
    }

    fn can_handle(&self, request: &QueryRequest) -> bool {
        matches!(request.query_type, QueryType::VectorSearch)
    }

    async fn execute(&self, _request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        Ok(QueryResult {
            data: QueryResultData::Empty,
            metrics: None,
        })
    }

    fn priority(&self) -> i32 {
        100
    }
}

struct MockSqlStrategy;

#[async_trait]
impl QueryStrategy for MockSqlStrategy {
    fn name(&self) -> &str {
        "mock_sql"
    }

    fn can_handle(&self, request: &QueryRequest) -> bool {
        matches!(request.query_type, QueryType::Sql)
    }

    async fn execute(&self, _request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        Ok(QueryResult {
            data: QueryResultData::Empty,
            metrics: None,
        })
    }

    fn priority(&self) -> i32 {
        25
    }
}

struct MockGraphStrategy;

#[async_trait]
impl QueryStrategy for MockGraphStrategy {
    fn name(&self) -> &str {
        "mock_graph"
    }

    fn can_handle(&self, request: &QueryRequest) -> bool {
        matches!(request.query_type, QueryType::Graph)
    }

    async fn execute(&self, _request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        Ok(QueryResult {
            data: QueryResultData::Empty,
            metrics: None,
        })
    }

    fn priority(&self) -> i32 {
        75
    }
}

fn create_facade_with_strategies() -> UnifiedQueryFacade {
    let strategies: Vec<Arc<dyn QueryStrategy>> = vec![
        Arc::new(MockVectorStrategy),
        Arc::new(MockSqlStrategy),
        Arc::new(MockGraphStrategy),
    ];
    UnifiedQueryFacade::with_strategies(strategies)
}

fn bench_facade_routing(c: &mut Criterion) {
    print_system_info("facade_routing");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let facade = create_facade_with_strategies();

    let mut group = c.benchmark_group("facade_routing");
    group.measurement_time(Duration::from_secs(10));

    // Benchmark vector query routing
    group.bench_function("vector_query_routing", |b| {
        b.iter(|| {
            rt.block_on(async {
                let request = QueryRequest::vector_search(vec![0.1, 0.2, 0.3], 10);
                black_box(facade.execute(request).await)
            })
        })
    });

    // Benchmark SQL query routing
    group.bench_function("sql_query_routing", |b| {
        b.iter(|| {
            rt.block_on(async {
                let request = QueryRequest::sql("SELECT * FROM users WHERE id = 1");
                black_box(facade.execute(request).await)
            })
        })
    });

    // Benchmark graph query routing
    group.bench_function("graph_query_routing", |b| {
        b.iter(|| {
            rt.block_on(async {
                let request = QueryRequest::graph("MATCH (n:Person)-[:KNOWS]->(m) RETURN m");
                black_box(facade.execute(request).await)
            })
        })
    });

    group.finish();
}

fn bench_request_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("request_construction");
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("vector_request_construction", |b| {
        b.iter(|| black_box(QueryRequest::vector_search(vec![0.1, 0.2, 0.3], 10)))
    });

    group.bench_function("sql_request_construction", |b| {
        b.iter(|| {
            black_box(QueryRequest::sql(
                "SELECT * FROM products ORDER BY embedding <-> '[0.1,0.2]'::vector LIMIT 10",
            ))
        })
    });

    group.bench_function("graph_request_construction", |b| {
        b.iter(|| {
            black_box(QueryRequest::graph(
                "MATCH (n:Person)-[:KNOWS]->(m) RETURN m",
            ))
        })
    });

    group.finish();
}

criterion_group!(benches, bench_facade_routing, bench_request_construction,);

criterion_main!(benches);
