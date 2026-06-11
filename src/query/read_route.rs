//! Read-side route contract for split-aware DataFusion and future distributed execution.
//!
//! This is the SELECT/read sibling of `table_write_plan::RoutedExecutionPlan`.
//! It is deliberately a route/explain artifact only: execution paths can adopt
//! it incrementally without changing runtime behavior while the split planner,
//! DataFusion local scan, and Ballista-backed distributed route mature.

use crate::query::table_write_plan::ComputeBackend;
use proximadb_catalog::{CatalogAuthorityMode, CatalogWorkloadProfile};
use serde::Serialize;

/// Physical split strategy selected for a read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadSplitStrategy {
    /// No usable split metadata; one partition scans the requested collection.
    WholeCollection,
    /// Storage segment or manifest-level splits.
    Segment,
    /// Cursor/range partitioning over ordered keys.
    CursorRange,
    /// Columnar row-group or equivalent file-internal split.
    RowGroup,
    /// External object/file inventory supplied by a publication or federated asset.
    ObjectFile,
}

impl ReadSplitStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WholeCollection => "whole_collection",
            Self::Segment => "segment",
            Self::CursorRange => "cursor_range",
            Self::RowGroup => "row_group",
            Self::ObjectFile => "object_file",
        }
    }
}

/// Policy enforcement boundary for the selected read route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadPolicyBoundary {
    /// ProximaDB injects/enforces tenant/RLS predicates before execution.
    EngineEnforced,
    /// A connector/table provider enforces policy under a ProximaDB contract.
    ConnectorEnforced,
    /// External system enforces policy; valid only for explicit external-authority modes.
    ExternalPolicy,
    /// Route is unsafe and must be rejected.
    Unsupported,
}

impl ReadPolicyBoundary {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EngineEnforced => "engine-enforced",
            Self::ConnectorEnforced => "connector-enforced",
            Self::ExternalPolicy => "external-policy",
            Self::Unsupported => "unsupported",
        }
    }

    pub fn is_supported(&self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

/// Freshness contract requested or selected for a read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFreshnessSla {
    /// Read must observe canonical WAL/record state.
    Synchronous,
    /// Bounded async projection/snapshot lag.
    BoundedAsync { max_lag_ms: u64 },
    /// Stale projection/snapshot is acceptable and must be visible in EXPLAIN.
    StaleOk,
    /// Projection can be rebuilt before/while serving.
    RebuildOnDemand,
    /// Catalog carried a freshness value that the read router preserves but does not parse yet.
    CatalogValue(String),
}

impl ReadFreshnessSla {
    pub fn explain_label(&self) -> String {
        match self {
            Self::Synchronous => "synchronous".to_string(),
            Self::BoundedAsync { max_lag_ms } => format!("bounded_async:{max_lag_ms}ms"),
            Self::StaleOk => "stale_ok".to_string(),
            Self::RebuildOnDemand => "rebuild_on_demand".to_string(),
            Self::CatalogValue(value) => value.clone(),
        }
    }
}

/// Estimated split inventory chosen for this route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSplitSummary {
    pub strategy: ReadSplitStrategy,
    pub partition_count: usize,
    pub estimated_rows: Option<u64>,
    pub estimated_bytes: Option<u64>,
    /// Human-readable freshness of statistics, e.g. `fresh`, `stale`, `absent`.
    pub stats_freshness: String,
}

impl ReadSplitSummary {
    pub fn whole_collection() -> Self {
        Self {
            strategy: ReadSplitStrategy::WholeCollection,
            partition_count: 1,
            estimated_rows: None,
            estimated_bytes: None,
            stats_freshness: "absent".to_string(),
        }
    }
}

/// Optional distributed placement metadata supplied by deployment/control-plane config.
///
/// ProximaDB owns the route contract. Concrete AKS pools, scheduler endpoints,
/// autoscaling, and credentials are provisioned by the ops/control-plane layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DistributedReadPlacement {
    pub compute_pool: Option<String>,
    pub scheduler_endpoint: Option<String>,
    pub executor_group: Option<String>,
}

/// Candidate route considered by the read planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateReadRoute {
    pub backend: ComputeBackend,
    pub access_method: String,
    pub reason: String,
}

/// Rejected route or split strategy with a stable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedReadRoute {
    pub backend: ComputeBackend,
    pub reason_code: String,
    pub message: String,
}

/// Typed route result for a read plan before it is lowered to an executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedReadPlan {
    pub backend: ComputeBackend,
    pub workload_profile: CatalogWorkloadProfile,
    pub authority_mode: CatalogAuthorityMode,
    pub policy_boundary: ReadPolicyBoundary,
    pub freshness_sla: ReadFreshnessSla,
    pub split_summary: ReadSplitSummary,
    pub pushed_filters: Vec<String>,
    pub residual_filters: Vec<String>,
    pub rejected_pushdowns: Vec<String>,
    pub candidate_routes: Vec<CandidateReadRoute>,
    pub rejected_routes: Vec<RejectedReadRoute>,
    pub distributed_placement: Option<DistributedReadPlacement>,
}

impl RoutedReadPlan {
    /// Construct the conservative single-node/native fallback.
    pub fn native_whole_collection(workload_profile: CatalogWorkloadProfile) -> Self {
        Self {
            backend: ComputeBackend::Native,
            workload_profile,
            authority_mode: CatalogAuthorityMode::ProximaAuthoritative,
            policy_boundary: ReadPolicyBoundary::EngineEnforced,
            freshness_sla: ReadFreshnessSla::Synchronous,
            split_summary: ReadSplitSummary::whole_collection(),
            pushed_filters: Vec::new(),
            residual_filters: Vec::new(),
            rejected_pushdowns: Vec::new(),
            candidate_routes: vec![CandidateReadRoute {
                backend: ComputeBackend::Native,
                access_method: "canonical-record-scan".to_string(),
                reason: "safe native fallback".to_string(),
            }],
            rejected_routes: Vec::new(),
            distributed_placement: None,
        }
    }

    pub fn route_explanation(&self) -> ReadRouteExplanation {
        ReadRouteExplanation {
            selected_backend: backend_name(&self.backend),
            workload_profile: format!("{:?}", self.workload_profile),
            authority_mode: format!("{:?}", self.authority_mode),
            policy_boundary: self.policy_boundary.as_str().to_string(),
            freshness_sla: self.freshness_sla.explain_label(),
            split_strategy: self.split_summary.strategy.as_str().to_string(),
            partition_count: self.split_summary.partition_count,
            estimated_rows: self.split_summary.estimated_rows,
            estimated_bytes: self.split_summary.estimated_bytes,
            stats_freshness: self.split_summary.stats_freshness.clone(),
            pushed_filters: self.pushed_filters.clone(),
            residual_filters: self.residual_filters.clone(),
            rejected_pushdowns: self.rejected_pushdowns.clone(),
            candidate_routes: self
                .candidate_routes
                .iter()
                .map(candidate_explanation)
                .collect(),
            rejected_routes: self
                .rejected_routes
                .iter()
                .map(rejected_explanation)
                .collect(),
            distributed_placement: self
                .distributed_placement
                .as_ref()
                .map(distributed_placement_explanation),
            route_valid: self.policy_boundary.is_supported() && self.rejected_routes.is_empty(),
        }
    }
}

/// Stable JSON/debug payload for unified EXPLAIN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadRouteExplanation {
    pub selected_backend: String,
    pub workload_profile: String,
    pub authority_mode: String,
    pub policy_boundary: String,
    pub freshness_sla: String,
    pub split_strategy: String,
    pub partition_count: usize,
    pub estimated_rows: Option<u64>,
    pub estimated_bytes: Option<u64>,
    pub stats_freshness: String,
    pub pushed_filters: Vec<String>,
    pub residual_filters: Vec<String>,
    pub rejected_pushdowns: Vec<String>,
    pub candidate_routes: Vec<ReadCandidateRouteExplanation>,
    pub rejected_routes: Vec<ReadRejectedRouteExplanation>,
    pub distributed_placement: Option<DistributedReadPlacementExplanation>,
    pub route_valid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadCandidateRouteExplanation {
    pub backend: String,
    pub access_method: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadRejectedRouteExplanation {
    pub backend: String,
    pub reason_code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DistributedReadPlacementExplanation {
    pub compute_pool: Option<String>,
    pub scheduler_endpoint: Option<String>,
    pub executor_group: Option<String>,
}

fn candidate_explanation(candidate: &CandidateReadRoute) -> ReadCandidateRouteExplanation {
    ReadCandidateRouteExplanation {
        backend: backend_name(&candidate.backend),
        access_method: candidate.access_method.clone(),
        reason: candidate.reason.clone(),
    }
}

fn rejected_explanation(rejected: &RejectedReadRoute) -> ReadRejectedRouteExplanation {
    ReadRejectedRouteExplanation {
        backend: backend_name(&rejected.backend),
        reason_code: rejected.reason_code.clone(),
        message: rejected.message.clone(),
    }
}

fn distributed_placement_explanation(
    placement: &DistributedReadPlacement,
) -> DistributedReadPlacementExplanation {
    DistributedReadPlacementExplanation {
        compute_pool: placement.compute_pool.clone(),
        scheduler_endpoint: placement.scheduler_endpoint.clone(),
        executor_group: placement.executor_group.clone(),
    }
}

pub fn backend_name(backend: &ComputeBackend) -> String {
    match backend {
        ComputeBackend::Native => "Native".to_string(),
        ComputeBackend::DataFusionLocal => "DataFusionLocal".to_string(),
        ComputeBackend::DataFusionDistributed => "DataFusionDistributed".to_string(),
        ComputeBackend::PolarsLocal => "PolarsLocal".to_string(),
        ComputeBackend::DuckDbCompat => "DuckDbCompat".to_string(),
        ComputeBackend::ExternalDelegated(name) => format!("ExternalDelegated({name})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_whole_collection_explains_safe_fallback() {
        let routed = RoutedReadPlan::native_whole_collection(CatalogWorkloadProfile::Oltp);
        let explain = routed.route_explanation();

        assert_eq!(explain.selected_backend, "Native");
        assert_eq!(explain.policy_boundary, "engine-enforced");
        assert_eq!(explain.freshness_sla, "synchronous");
        assert_eq!(explain.split_strategy, "whole_collection");
        assert_eq!(explain.partition_count, 1);
        assert!(explain.route_valid);
    }

    #[test]
    fn datafusion_distributed_route_carries_ballista_placement_metadata() {
        let routed = RoutedReadPlan {
            backend: ComputeBackend::DataFusionDistributed,
            workload_profile: CatalogWorkloadProfile::Olap,
            authority_mode: CatalogAuthorityMode::ProjectionPublication,
            policy_boundary: ReadPolicyBoundary::ConnectorEnforced,
            freshness_sla: ReadFreshnessSla::BoundedAsync { max_lag_ms: 5_000 },
            split_summary: ReadSplitSummary {
                strategy: ReadSplitStrategy::ObjectFile,
                partition_count: 32,
                estimated_rows: Some(10_000_000),
                estimated_bytes: Some(8_000_000_000),
                stats_freshness: "fresh".to_string(),
            },
            pushed_filters: vec!["tenant_id = 't1'".to_string()],
            residual_filters: vec!["udf_score(payload) > 0.8".to_string()],
            rejected_pushdowns: vec!["udf_score is not storage-pushdown safe".to_string()],
            candidate_routes: vec![CandidateReadRoute {
                backend: ComputeBackend::DataFusionDistributed,
                access_method: "ballista-object-splits".to_string(),
                reason: "large OLAP scan with object-file split inventory".to_string(),
            }],
            rejected_routes: Vec::new(),
            distributed_placement: Some(DistributedReadPlacement {
                compute_pool: Some("olap-general".to_string()),
                scheduler_endpoint: Some("df://ballista-scheduler:50050".to_string()),
                executor_group: Some("ballista-executors".to_string()),
            }),
        };

        let explain = routed.route_explanation();

        assert_eq!(explain.selected_backend, "DataFusionDistributed");
        assert_eq!(explain.policy_boundary, "connector-enforced");
        assert_eq!(explain.freshness_sla, "bounded_async:5000ms");
        assert_eq!(explain.partition_count, 32);
        assert_eq!(explain.pushed_filters, vec!["tenant_id = 't1'"]);
        assert_eq!(explain.residual_filters, vec!["udf_score(payload) > 0.8"]);
        assert_eq!(
            explain
                .distributed_placement
                .expect("distributed placement")
                .compute_pool
                .as_deref(),
            Some("olap-general")
        );
        assert!(explain.route_valid);
    }

    #[test]
    fn unsupported_policy_boundary_marks_route_invalid() {
        let mut routed = RoutedReadPlan::native_whole_collection(CatalogWorkloadProfile::Olap);
        routed.backend = ComputeBackend::DataFusionDistributed;
        routed.policy_boundary = ReadPolicyBoundary::Unsupported;
        routed.rejected_routes.push(RejectedReadRoute {
            backend: ComputeBackend::DataFusionDistributed,
            reason_code: "unsupported_policy_boundary".to_string(),
            message: "distributed route cannot enforce tenant/RLS predicates".to_string(),
        });

        let explain = routed.route_explanation();

        assert_eq!(explain.selected_backend, "DataFusionDistributed");
        assert_eq!(explain.policy_boundary, "unsupported");
        assert!(!explain.route_valid);
        assert_eq!(
            explain.rejected_routes[0].reason_code,
            "unsupported_policy_boundary"
        );
    }

    #[test]
    fn explanation_serializes_with_stable_keys() {
        let routed = RoutedReadPlan::native_whole_collection(CatalogWorkloadProfile::Oltp);
        let value = serde_json::to_value(routed.route_explanation()).expect("serialize");

        assert_eq!(value["selected_backend"], "Native");
        assert_eq!(value["split_strategy"], "whole_collection");
        assert_eq!(value["route_valid"], true);
        assert!(value["candidate_routes"].is_array());
    }
}
