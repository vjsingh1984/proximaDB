//! Shared distributed query planning contracts.

use proximadb_multimodel_query::QueryComponent;

/// A subquery targeted at specific shards.
#[derive(Debug, Clone)]
pub struct ShardedSubQuery {
    /// Target node for this subquery.
    pub target_node: String,
    /// Target node address.
    pub target_address: String,
    /// Shard IDs this subquery covers.
    pub shard_ids: Vec<String>,
    /// The query component(s) to execute.
    pub components: Vec<QueryComponent>,
    /// Collection name, when applicable.
    pub collection: Option<String>,
    /// Priority; lower values run first.
    pub priority: u32,
}
