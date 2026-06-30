//! Foundation search-result and filter-value types for ProximaDB.
//!
//! Extracted from `proximadb::core::search` as part of the root-crate
//! decomposition (Slice D / D1). These are the self-contained, foundation-tier
//! leaves of the search module — search-result records, the bounded result
//! queue, SQL/JSON value filtering, and JSON value (de)serialization — with no
//! upward dependencies on the storage/compute/index/query orchestration layers.
//!
//! The orchestration types that remain in `proximadb::core::search`
//! (`UnifiedSearchParams` and the search engines) depend *down* on this crate;
//! the root re-exports every module here so existing
//! `crate::core::search::{results, bounded_queue, sql_value_filter,
//! json_value_serde, json_comparison}::*` paths resolve unchanged.

pub mod bounded_queue;
pub mod json_comparison;
pub mod json_value_serde;
pub mod results;
pub mod sql_value_filter;
