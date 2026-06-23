//! Boundary implementation of the storage↔index decoupling contract
//! ([`proximadb_index_traits`]) for [`AxisManager`].
//!
//! This is the "convert at the index boundary" half of the §A2 resolution from
//! `docs/12-design/STORAGE_INDEX_COMPUTE_DECOUPLING_CONTRACTS_2026_06_21.adoc`:
//! the slim, dependency-light DTO lives in the foundation `proximadb-index-traits`
//! crate; the conversions to/from the rich AXIS envelope
//! (`AxisHybridQuery`/`AxisManagerQueryResult`, which carry catalog/core/
//! observability types) live here, next to `AxisManager`.
//!
//! Implementing the role traits here — with no behavioural change — lets us prove
//! the DTO is lossless for storage's measured use *before* any engine field is
//! narrowed from `Arc<AxisManager>` to `Arc<dyn IndexQuery>` (the separate,
//! coordinated DIP-adoption step).

use async_trait::async_trait;

use proximadb_index_traits::{
    IndexFilterOperator, IndexHybridQuery, IndexIngest, IndexLifecycle, IndexMaintenance,
    IndexMetadataFilter, IndexMetrics, IndexQuery, IndexQueryResult, IndexScoredResult,
    IndexSearchEffort, IndexVectorQuery,
};
use proximadb_records::ProximaRecord;

use crate::core::search::SearchEffort;
use crate::index::axis::management::manager::{
    AxisHybridQuery, AxisManager, AxisManagerQueryResult, AxisMetadataFilter, FilterOperator,
    ScoredResult, VectorQuery,
};

// ---------------------------------------------------------------------------
// Input conversions: slim DTO -> rich AXIS envelope (DTO target types are local,
// so these are `From` impls; orphan rule satisfied).
// ---------------------------------------------------------------------------

impl From<IndexSearchEffort> for SearchEffort {
    fn from(e: IndexSearchEffort) -> Self {
        match e {
            IndexSearchEffort::Exact => SearchEffort::Exact,
            IndexSearchEffort::Approximate { hint } => SearchEffort::Approximate { hint },
        }
    }
}

impl From<IndexFilterOperator> for FilterOperator {
    fn from(op: IndexFilterOperator) -> Self {
        match op {
            IndexFilterOperator::Equals => FilterOperator::Equals,
            IndexFilterOperator::NotEquals => FilterOperator::NotEquals,
            IndexFilterOperator::GreaterThan => FilterOperator::GreaterThan,
            IndexFilterOperator::GreaterThanOrEqual => FilterOperator::GreaterThanOrEqual,
            IndexFilterOperator::LessThan => FilterOperator::LessThan,
            IndexFilterOperator::LessThanOrEqual => FilterOperator::LessThanOrEqual,
            IndexFilterOperator::In => FilterOperator::In,
            IndexFilterOperator::NotIn => FilterOperator::NotIn,
            IndexFilterOperator::Contains => FilterOperator::Contains,
            IndexFilterOperator::StartsWith => FilterOperator::StartsWith,
            IndexFilterOperator::EndsWith => FilterOperator::EndsWith,
            IndexFilterOperator::Like => FilterOperator::Like,
            IndexFilterOperator::Between => FilterOperator::Between,
            IndexFilterOperator::IsNull => FilterOperator::IsNull,
            IndexFilterOperator::IsNotNull => FilterOperator::IsNotNull,
        }
    }
}

impl From<IndexMetadataFilter> for AxisMetadataFilter {
    fn from(f: IndexMetadataFilter) -> Self {
        AxisMetadataFilter {
            field: f.field,
            operator: f.operator.into(),
            value: f.value,
        }
    }
}

impl From<IndexVectorQuery> for VectorQuery {
    fn from(q: IndexVectorQuery) -> Self {
        match q {
            IndexVectorQuery::Dense {
                vector,
                similarity_threshold,
            } => VectorQuery::Dense {
                vector,
                similarity_threshold,
            },
            IndexVectorQuery::Sparse {
                vector,
                similarity_threshold,
            } => VectorQuery::Sparse {
                vector,
                similarity_threshold,
            },
        }
    }
}

impl From<IndexHybridQuery> for AxisHybridQuery {
    fn from(q: IndexHybridQuery) -> Self {
        AxisHybridQuery {
            collection_id: q.collection_id,
            vector_query: q.vector_query.map(Into::into),
            metadata_filters: q.metadata_filters.into_iter().map(Into::into).collect(),
            id_filters: q.id_filters,
            top_k: q.top_k,
            include_expired: q.include_expired,
            search_effort: q.search_effort.map(Into::into),
            // Storage never sets these on the query path (verified at the call
            // sites); the AXIS query path defaults them. Keeping them here makes
            // the "DTO is lossless for storage" property explicit.
            ..AxisHybridQuery::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Output conversions: rich AXIS result -> slim DTO. The DTO result types are
// foreign (defined in proximadb-index-traits), so these are free functions, not
// `From` impls (orphan rule).
// ---------------------------------------------------------------------------

fn to_index_scored_result(s: ScoredResult) -> IndexScoredResult {
    IndexScoredResult {
        vector_id: s.vector_id,
        similarity: s.similarity,
        expires_at: s.expires_at,
    }
}

fn to_index_query_result(r: AxisManagerQueryResult) -> IndexQueryResult {
    IndexQueryResult {
        results: r.results.into_iter().map(to_index_scored_result).collect(),
    }
}

// ---------------------------------------------------------------------------
// Role-trait implementations. Each delegates to the corresponding inherent
// `AxisManager` method (called via `AxisManager::method(self, ..)` so inherent
// resolution is unambiguous against the same-named trait method). No behavioural
// change — only the call shape.
// ---------------------------------------------------------------------------

#[async_trait]
impl IndexQuery for AxisManager {
    async fn query(&self, query: IndexHybridQuery) -> anyhow::Result<IndexQueryResult> {
        let result = AxisManager::query(self, query.into()).await?;
        Ok(to_index_query_result(result))
    }
}

#[async_trait]
impl IndexIngest for AxisManager {
    async fn handle_flushed_vectors(
        &self,
        collection_id: &str,
        flushed_vectors: Vec<ProximaRecord>,
        files_created: Vec<String>,
    ) -> anyhow::Result<()> {
        AxisManager::handle_flushed_vectors(self, collection_id, flushed_vectors, files_created)
            .await
    }
}

#[async_trait]
impl IndexMetrics for AxisManager {
    async fn registered_vector_count(&self, collection_id: &str) -> usize {
        AxisManager::registered_vector_count(self, collection_id).await
    }
}

#[async_trait]
impl IndexMaintenance for AxisManager {
    async fn rebuild_index(&self, collection_id: &str, index_name: &str) -> anyhow::Result<()> {
        AxisManager::rebuild_index(self, collection_id, index_name).await
    }

    async fn analyze_and_optimize(&self, collection_id: &str) -> anyhow::Result<()> {
        AxisManager::analyze_and_optimize(self, collection_id).await
    }
}

#[async_trait]
impl IndexLifecycle for AxisManager {
    async fn drop_collection(&self, collection_id: &str) -> anyhow::Result<()> {
        AxisManager::drop_collection(self, collection_id).await
    }

    async fn suspend_collection(&self, collection_id: &str) -> anyhow::Result<()> {
        AxisManager::suspend_collection(self, collection_id).await
    }

    async fn resume_collection(&self, collection_id: &str) -> anyhow::Result<bool> {
        AxisManager::resume_collection(self, collection_id).await
    }

    async fn is_suspended(&self, collection_id: &str) -> bool {
        AxisManager::is_suspended(self, collection_id).await
    }
}
