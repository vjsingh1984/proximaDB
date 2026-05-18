/// REST handlers for read-only collection analytics (Entanglement Index)
pub mod analytics;
/// REST handlers for Agentic Query Language (RUBICON)
pub mod aql;
#[cfg(feature = "enterprise-catalogs")]
/// REST handlers for enterprise catalog operations (Polaris, Delta Lake)
pub mod catalog;
/// REST handlers for document storage operations
pub mod document;
/// REST handlers for SKS entity operations
pub mod entities;
/// REST handlers for graph database operations
pub mod graph;
/// Core REST handlers for collections, vectors, search, and health
pub mod handlers;
/// REST handlers for hybrid (vector + BM25) search
pub mod hybrid;
/// Iceberg REST Catalog server (v1 spec) — Spark/Trino/DuckDB/PyIceberg compatible
pub mod iceberg_rest_catalog;
/// REST handlers for unified multi-model query execution
pub mod multimodal_query;
/// REST handlers for Natural Language query translation (AV-SQL)
pub mod nl;
/// REST handlers for observability queries (logs, metrics, traces)
pub mod observability;
