//! Phase D: MultiModelPlan operator dispatch — spec §7.
//!
//! Routes `Operator::CrossModelJoin` → [`MshjExecutor`] and
//! `Operator::HybridTraverse` → [`HybridTraverseExecutor`].
//!
//! All other operators are passed through as no-ops in this phase; they are
//! handled by the storage-engine pipeline in `compute::pipeline_executor`.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use proximadb_data_model::DataModel;
use proximadb_multimodel_plan::{MultiModelPlan, Operator};
use serde_json::Value;

use crate::operators::{
    hybrid_traverse::{AnnSeedProvider, GraphNeighbourProvider, HybridTraverseExecutor},
    mshj::MshjExecutor,
};

/// A row-oriented data source injected into the executor for Scan dispatch.
pub trait PlanDataSource: Send + Sync {
    fn scan(&self, model: DataModel, limit: Option<usize>) -> Result<Vec<Value>>;
}

/// Execution context for one plan run.
pub struct PlanExecutionContext {
    /// Source for Scan-dispatched rows (left/right sides of joins).
    pub data_source: Arc<dyn PlanDataSource>,
    /// ANN seed provider wired to HybridTraverse + VectorTopK.
    pub ann_provider: Option<Arc<dyn AnnSeedProvider>>,
    /// Graph neighbour provider wired to HybridTraverse.
    pub graph_provider: Option<Arc<dyn GraphNeighbourProvider>>,
    /// Query vector carried from a preceding VectorTopK op.
    current_query_vector: Vec<f32>,
    /// Rows produced by the last operator (pipeline accumulator).
    current_rows: Vec<Value>,
}

impl PlanExecutionContext {
    pub fn new(data_source: Arc<dyn PlanDataSource>) -> Self {
        Self {
            data_source,
            ann_provider: None,
            graph_provider: None,
            current_query_vector: Vec::new(),
            current_rows: Vec::new(),
        }
    }

    pub fn with_ann(mut self, ann: Arc<dyn AnnSeedProvider>) -> Self {
        self.ann_provider = Some(ann);
        self
    }

    pub fn with_graph(mut self, graph: Arc<dyn GraphNeighbourProvider>) -> Self {
        self.graph_provider = Some(graph);
        self
    }
}

/// Per-operator execution statistics.
#[derive(Debug, Clone, Default)]
pub struct OperatorStats {
    pub operator_index: usize,
    pub rows_in: usize,
    pub rows_out: usize,
    /// Microseconds spent in this operator.
    pub elapsed_us: u64,
}

/// Result of executing a full plan.
#[derive(Debug, Clone, Default)]
pub struct PlanExecutionResult {
    pub rows: Vec<Value>,
    pub operator_stats: Vec<OperatorStats>,
}

/// Phase D plan executor — dispatches operators to concrete executors.
pub struct PlanExecutor;

impl PlanExecutor {
    /// Execute `plan` given the injected `ctx` providers, returning all rows.
    pub fn execute(
        plan: &MultiModelPlan,
        ctx: &mut PlanExecutionContext,
    ) -> Result<PlanExecutionResult> {
        let mut stats_vec = Vec::with_capacity(plan.len());

        for (idx, operator) in plan.operators.iter().enumerate() {
            let t0 = std::time::Instant::now();
            let rows_in = ctx.current_rows.len();

            Self::dispatch(idx, operator, ctx)?;

            stats_vec.push(OperatorStats {
                operator_index: idx,
                rows_in,
                rows_out: ctx.current_rows.len(),
                elapsed_us: t0.elapsed().as_micros() as u64,
            });
        }

        Ok(PlanExecutionResult {
            rows: std::mem::take(&mut ctx.current_rows),
            operator_stats: stats_vec,
        })
    }

    fn dispatch(idx: usize, operator: &Operator, ctx: &mut PlanExecutionContext) -> Result<()> {
        match operator {
            // ── Phase D: Cross-modal Multi-Stage Hash Join ─────────────────────
            Operator::CrossModelJoin {
                left_modality,
                right_modality,
                condition,
            } => {
                let left = ctx.data_source.scan(*left_modality, None)?;
                let right = ctx.data_source.scan(*right_modality, None)?;
                let (joined, _stats) = MshjExecutor::join(&left, &right, condition)?;
                ctx.current_rows = joined.into_iter().map(|r| r.record).collect();
            }

            // ── Phase D: Hybrid graph + vector traversal ───────────────────────
            Operator::HybridTraverse { edge_pattern } => {
                let ann = ctx
                    .ann_provider
                    .as_ref()
                    .ok_or_else(|| anyhow!("HybridTraverse requires an AnnSeedProvider"))?
                    .clone();
                let graph = ctx
                    .graph_provider
                    .as_ref()
                    .ok_or_else(|| anyhow!("HybridTraverse requires a GraphNeighbourProvider"))?
                    .clone();

                let beam_width = 10.max(ctx.current_rows.len());
                let top_k = beam_width;
                let executor = HybridTraverseExecutor::new(beam_width, top_k);
                let (nodes, _tstat) = executor.traverse(
                    &ctx.current_query_vector,
                    edge_pattern,
                    ann.as_ref(),
                    graph.as_ref(),
                )?;
                ctx.current_rows = nodes
                    .into_iter()
                    .map(|n| {
                        serde_json::json!({
                            "id": n.id,
                            "vector_score": n.vector_score,
                            "hop_depth": n.hop_depth,
                            "payload": n.payload,
                        })
                    })
                    .collect();
            }

            // ── VectorTopK: seed the query vector for downstream HybridTraverse ─
            Operator::VectorTopK {
                query_vector, k, ..
            } => {
                ctx.current_query_vector = query_vector.clone();
                if let Some(ann) = &ctx.ann_provider {
                    let seeds = ann.find_seeds(query_vector, *k)?;
                    ctx.current_rows = seeds
                        .into_iter()
                        .map(|(id, score)| serde_json::json!({ "id": id, "score": score }))
                        .collect();
                }
                // If no ANN provider, leave rows unchanged (plan validation catches this).
            }

            // ── Passthrough operators (storage-engine pipeline handles these) ───
            Operator::Scan { source, .. } => {
                // The data model name drives the scan; full engine dispatch is in
                // `compute::pipeline_executor`. Here we emit a placeholder scan.
                let model = Self::data_model_from_source(source);
                ctx.current_rows = ctx.data_source.scan(model, None)?;
            }

            Operator::Limit { n, offset } => {
                let rows = std::mem::take(&mut ctx.current_rows);
                ctx.current_rows = rows.into_iter().skip(*offset).take(*n).collect();
            }

            Operator::TopK { k, .. } => {
                ctx.current_rows.truncate(*k);
            }

            // All other operators are no-ops in Phase D (handled by PipelineExecutor)
            _ => {
                tracing::trace!(
                    "PlanExecutor: operator {} ({:?}) is a passthrough in Phase D",
                    idx,
                    std::mem::discriminant(operator)
                );
            }
        }
        Ok(())
    }

    fn data_model_from_source(source: &str) -> DataModel {
        match source.to_lowercase().as_str() {
            "vector" | "vectors" => DataModel::Vector,
            "document" | "documents" => DataModel::Document,
            "graph" | "graphs" => DataModel::Graph,
            "timeseries" | "ts" => DataModel::TimeSeries,
            "log" | "logs" | "observability" => DataModel::Observability,
            "event" | "events" => DataModel::Event,
            _ => DataModel::Vector,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests (TDD first)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_data_model::DataModel as PlanDataModel;
    use proximadb_multimodel_plan::{
        EdgePattern, JoinCondition, PlanContext, TraversalDirection, VectorMetric,
    };

    // ── Mock data source ─────────────────────────────────────────────────────

    struct FixedDataSource {
        vector_rows: Vec<Value>,
        document_rows: Vec<Value>,
    }

    impl PlanDataSource for FixedDataSource {
        fn scan(&self, model: DataModel, _limit: Option<usize>) -> Result<Vec<Value>> {
            Ok(match model {
                proximadb_data_model::DataModel::Vector => self.vector_rows.clone(),
                proximadb_data_model::DataModel::Document => self.document_rows.clone(),
                _ => Vec::new(),
            })
        }
    }

    // ── Mock ANN provider ────────────────────────────────────────────────────

    struct FixedAnn {
        seeds: Vec<(String, f32)>,
    }

    impl AnnSeedProvider for FixedAnn {
        fn find_seeds(&self, _query: &[f32], _k: usize) -> Result<Vec<(String, f32)>> {
            Ok(self.seeds.clone())
        }
    }

    // ── Mock graph provider ──────────────────────────────────────────────────

    struct EmptyGraph;

    impl GraphNeighbourProvider for EmptyGraph {
        fn neighbours(
            &self,
            _node_id: &str,
        ) -> Result<Vec<(String, Option<String>, TraversalDirection)>> {
            Ok(Vec::new())
        }
    }

    fn make_context(vector_rows: Vec<Value>, document_rows: Vec<Value>) -> PlanExecutionContext {
        let src = Arc::new(FixedDataSource {
            vector_rows,
            document_rows,
        });
        PlanExecutionContext::new(src)
    }

    fn plan_ctx() -> PlanContext {
        PlanContext::default()
    }

    // ── Test: empty plan returns empty result ────────────────────────────────

    #[test]
    fn test_empty_plan_returns_empty_result() {
        let plan = MultiModelPlan::new(vec![], plan_ctx());
        let mut ctx = make_context(vec![], vec![]);
        let result = PlanExecutor::execute(&plan, &mut ctx).unwrap();
        assert!(result.rows.is_empty());
        assert!(result.operator_stats.is_empty());
    }

    // ── Test: CrossModelJoin dispatches to MshjExecutor ─────────────────────

    #[test]
    fn test_cross_model_join_dispatches_to_mshj() {
        let vector_rows = vec![
            serde_json::json!({ "id": "v1", "embedding_ref": "e1" }),
            serde_json::json!({ "id": "v2", "embedding_ref": "e2" }),
        ];
        let doc_rows = vec![
            serde_json::json!({ "doc_id": "e1", "title": "Alpha" }),
            serde_json::json!({ "doc_id": "e3", "title": "Gamma" }), // no match
        ];

        let plan = MultiModelPlan::new(
            vec![Operator::CrossModelJoin {
                left_modality: PlanDataModel::Vector,
                right_modality: PlanDataModel::Document,
                condition: JoinCondition::On("embedding_ref".to_string(), "doc_id".to_string()),
            }],
            plan_ctx(),
        );

        let mut ctx = make_context(vector_rows, doc_rows);
        let result = PlanExecutor::execute(&plan, &mut ctx).unwrap();

        // Only v1/e1 match; v2 has no doc, e3 has no vector
        assert_eq!(result.rows.len(), 1, "expected exactly one joined row");
        let row = &result.rows[0];
        assert_eq!(row["id"], "v1");
        assert_eq!(row["title"], "Alpha");

        // Operator stats recorded
        assert_eq!(result.operator_stats.len(), 1);
        assert_eq!(result.operator_stats[0].rows_out, 1);
    }

    // ── Test: HybridTraverse dispatches to HybridTraverseExecutor ───────────

    #[test]
    fn test_hybrid_traverse_dispatches_to_executor() {
        let ann = Arc::new(FixedAnn {
            seeds: vec![("node_a".to_string(), 0.95), ("node_b".to_string(), 0.80)],
        });
        let graph = Arc::new(EmptyGraph);

        let edge_pattern = EdgePattern {
            edge_type: None,
            min_hops: 0,
            max_hops: Some(2),
            direction: TraversalDirection::Both,
        };

        let plan = MultiModelPlan::new(
            vec![
                Operator::VectorTopK {
                    query_vector: vec![1.0, 0.0, 0.0],
                    k: 5,
                    metric: VectorMetric::Cosine,
                    predicate: None,
                },
                Operator::HybridTraverse { edge_pattern },
            ],
            plan_ctx(),
        );

        let mut ctx = make_context(vec![], vec![]).with_ann(ann).with_graph(graph);
        let result = PlanExecutor::execute(&plan, &mut ctx).unwrap();

        // Seeds node_a and node_b returned; EmptyGraph adds no further nodes
        assert_eq!(result.rows.len(), 2);
        let ids: Vec<&str> = result
            .rows
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"node_a"));
        assert!(ids.contains(&"node_b"));

        // Two operators, both recorded
        assert_eq!(result.operator_stats.len(), 2);
    }

    // ── Test: HybridTraverse without provider returns an error ───────────────

    #[test]
    fn test_hybrid_traverse_without_provider_errors() {
        let edge_pattern = EdgePattern {
            edge_type: None,
            min_hops: 0,
            max_hops: Some(1),
            direction: TraversalDirection::Both,
        };

        let plan = MultiModelPlan::new(vec![Operator::HybridTraverse { edge_pattern }], plan_ctx());

        let mut ctx = make_context(vec![], vec![]); // no ANN provider
        let err = PlanExecutor::execute(&plan, &mut ctx).unwrap_err();
        assert!(err.to_string().contains("AnnSeedProvider"));
    }

    // ── Test: Limit operator slices rows ────────────────────────────────────

    #[test]
    fn test_limit_operator_slices_rows() {
        let vector_rows: Vec<Value> = (0..10)
            .map(|i| serde_json::json!({ "id": format!("v{i}") }))
            .collect();

        let plan = MultiModelPlan::new(
            vec![
                Operator::Scan {
                    source: "vector".to_string(),
                    data_model: PlanDataModel::Vector,
                    filter: None,
                    columns: None,
                },
                Operator::Limit { n: 3, offset: 2 },
            ],
            plan_ctx(),
        );

        let mut ctx = make_context(vector_rows, vec![]);
        let result = PlanExecutor::execute(&plan, &mut ctx).unwrap();
        assert_eq!(result.rows.len(), 3);
        assert_eq!(result.rows[0]["id"], "v2"); // offset 2
    }
}
