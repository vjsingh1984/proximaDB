//! # Cross-Model Query Optimizer
//!
//! Optimizes federated queries across multiple data models.
//!
//! ## Optimization Strategies
//!
//! - **Predicate pushdown**: Push filters to storage engines
//! - **Join reordering**: Optimize cross-model join order
//! - **Model-aware cost estimation**: Different cost models per data model
//! - **Parallel execution planning**: Identify parallelizable operations

use std::collections::HashMap;
use anyhow::{Result, anyhow};

use super::parser::{FederatedQuery, QueryType, SqlExtension, TargetModelType};
use crate::storage::multimodel::ModelType;

/// Physical plan node types
#[derive(Debug, Clone)]
pub enum PlanNodeType {
    /// Scan a table/collection
    Scan {
        target: String,
        model_type: ModelType,
        predicates: Vec<Predicate>,
    },
    /// Vector similarity search
    VectorSearch {
        collection: String,
        top_k: usize,
        query_vector_source: VectorSource,
    },
    /// Graph traversal
    GraphTraversal {
        cypher: String,
        start_nodes: Option<Vec<String>>,
    },
    /// Document query
    DocumentQuery {
        collection: String,
        filter: Option<String>,
    },
    /// Observability query (logs/metrics)
    ObservabilityQuery {
        namespace: String,
        query_type: ObservabilityQueryType,
        time_range: Option<TimeRange>,
    },
    /// Hash join
    HashJoin {
        left: Box<PlanNode>,
        right: Box<PlanNode>,
        join_keys: Vec<(String, String)>,
        join_type: JoinType,
    },
    /// Nested loop join (for LATERAL)
    NestedLoopJoin {
        outer: Box<PlanNode>,
        inner: Box<PlanNode>,
        correlation: Vec<String>,
    },
    /// Index join
    IndexJoin {
        left: Box<PlanNode>,
        right: Box<PlanNode>,
        index_lookup: String,
    },
    /// Filter operation
    Filter {
        input: Box<PlanNode>,
        predicate: Predicate,
    },
    /// Project columns
    Project {
        input: Box<PlanNode>,
        columns: Vec<String>,
    },
    /// Sort/Order By
    Sort {
        input: Box<PlanNode>,
        order_by: Vec<OrderByClause>,
    },
    /// Limit
    Limit {
        input: Box<PlanNode>,
        limit: usize,
        offset: usize,
    },
    /// Aggregate
    Aggregate {
        input: Box<PlanNode>,
        group_by: Vec<String>,
        aggregates: Vec<AggregateExpr>,
    },
    /// Union
    Union {
        inputs: Vec<PlanNode>,
        all: bool,
    },
}

/// Source of a query vector
#[derive(Debug, Clone)]
pub enum VectorSource {
    /// Literal vector value
    Literal(Vec<f32>),
    /// Column reference from another table
    ColumnRef { table: String, column: String },
    /// Subquery result
    Subquery(Box<PlanNode>),
}

/// Observability query type
#[derive(Debug, Clone)]
pub enum ObservabilityQueryType {
    Logs,
    Metrics,
    Traces,
}

/// Time range for observability queries
#[derive(Debug, Clone)]
pub struct TimeRange {
    pub start_ns: i64,
    pub end_ns: i64,
}

/// Join type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
    Lateral,
}

/// Predicate for filtering
#[derive(Debug, Clone)]
pub struct Predicate {
    pub column: String,
    pub op: PredicateOp,
    pub value: PredicateValue,
}

/// Predicate operators
#[derive(Debug, Clone)]
pub enum PredicateOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Like,
    In,
    IsNull,
    IsNotNull,
    Between,
}

/// Predicate values
#[derive(Debug, Clone)]
pub enum PredicateValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    List(Vec<PredicateValue>),
    Null,
}

/// Order by clause
#[derive(Debug, Clone)]
pub struct OrderByClause {
    pub column: String,
    pub ascending: bool,
    pub nulls_first: bool,
}

/// Aggregate expression
#[derive(Debug, Clone)]
pub struct AggregateExpr {
    pub function: AggregateFunction,
    pub column: Option<String>,
    pub alias: String,
}

/// Aggregate functions
#[derive(Debug, Clone)]
pub enum AggregateFunction {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    CountDistinct,
}

/// Query execution plan
#[derive(Debug, Clone)]
pub struct PlanNode {
    /// Node ID for tracking
    pub id: usize,
    /// Node type with parameters
    pub node_type: PlanNodeType,
    /// Estimated cost
    pub estimated_cost: f64,
    /// Estimated row count
    pub estimated_rows: u64,
    /// Output columns
    pub output_columns: Vec<String>,
}

/// Complete query plan
#[derive(Debug)]
pub struct QueryPlan {
    /// Root of the plan tree
    pub root: PlanNode,
    /// Total estimated cost
    pub total_cost: f64,
    /// Plan metadata
    pub metadata: PlanMetadata,
}

/// Plan metadata
#[derive(Debug, Default)]
pub struct PlanMetadata {
    /// Models involved in the plan
    pub involved_models: Vec<ModelType>,
    /// Whether this is a cross-model join
    pub is_cross_model: bool,
    /// Parallelizable stages
    pub parallel_stages: Vec<Vec<usize>>,
    /// Optimization hints applied
    pub hints_applied: Vec<String>,
}

/// Cost model for different data models
#[derive(Debug, Clone)]
pub struct CostModel {
    /// Base cost per row scan
    pub row_scan_cost: f64,
    /// Index lookup cost
    pub index_lookup_cost: f64,
    /// Network transfer cost per byte
    pub network_cost_per_byte: f64,
    /// CPU cost per operation
    pub cpu_cost_per_op: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        Self {
            row_scan_cost: 1.0,
            index_lookup_cost: 0.1,
            network_cost_per_byte: 0.001,
            cpu_cost_per_op: 0.01,
        }
    }
}

/// Cross-model query optimizer
pub struct CrossModelOptimizer {
    /// Cost models per data model
    cost_models: HashMap<ModelType, CostModel>,
    /// Next plan node ID
    next_node_id: std::sync::atomic::AtomicUsize,
}

impl CrossModelOptimizer {
    /// Create a new optimizer
    pub fn new() -> Self {
        let mut cost_models = HashMap::new();

        // Vector search is fast with index
        cost_models.insert(ModelType::Vector, CostModel {
            row_scan_cost: 0.5,
            index_lookup_cost: 0.05,
            network_cost_per_byte: 0.001,
            cpu_cost_per_op: 0.02, // More CPU for distance calculations
        });

        // Graph traversal varies by depth
        cost_models.insert(ModelType::Graph, CostModel {
            row_scan_cost: 0.3, // CSR is efficient
            index_lookup_cost: 0.01, // Very fast node lookup
            network_cost_per_byte: 0.001,
            cpu_cost_per_op: 0.005,
        });

        // Document queries depend on indexes
        cost_models.insert(ModelType::Document, CostModel {
            row_scan_cost: 1.0,
            index_lookup_cost: 0.2,
            network_cost_per_byte: 0.002, // Larger documents
            cpu_cost_per_op: 0.01,
        });

        // RDBMS is baseline
        cost_models.insert(ModelType::Relational, CostModel::default());

        // Observability is optimized for time-range
        cost_models.insert(ModelType::Observability, CostModel {
            row_scan_cost: 0.2, // Partitioned by time
            index_lookup_cost: 0.1,
            network_cost_per_byte: 0.001,
            cpu_cost_per_op: 0.005,
        });

        Self {
            cost_models,
            next_node_id: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Optimize a federated query
    pub fn optimize(&self, query: &FederatedQuery) -> Result<QueryPlan> {
        // Build initial logical plan
        let logical_plan = self.build_logical_plan(query)?;

        // Apply optimizations
        let optimized = self.apply_optimizations(logical_plan)?;

        // Build physical plan
        let physical_plan = self.build_physical_plan(optimized)?;

        Ok(physical_plan)
    }

    /// Build initial logical plan from parsed query
    fn build_logical_plan(&self, query: &FederatedQuery) -> Result<PlanNode> {
        match query.query_type {
            QueryType::Sql => self.plan_sql_query(query),
            QueryType::VectorSearch => self.plan_vector_search(query),
            QueryType::GraphQuery => self.plan_graph_query(query),
            QueryType::DocumentQuery => self.plan_document_query(query),
            QueryType::LogQuery | QueryType::MetricQuery => self.plan_observability_query(query),
            QueryType::Federated => self.plan_federated_query(query),
        }
    }

    /// Plan a simple SQL query
    fn plan_sql_query(&self, query: &FederatedQuery) -> Result<PlanNode> {
        let target = query.targets.first()
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "unknown".to_string());

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::Scan {
                target: target.clone(),
                model_type: ModelType::Relational,
                predicates: vec![],
            },
            estimated_cost: 100.0,
            estimated_rows: 1000,
            output_columns: vec!["*".to_string()],
        })
    }

    /// Plan a vector search query
    fn plan_vector_search(&self, query: &FederatedQuery) -> Result<PlanNode> {
        let (collection, top_k) = query.extensions.iter()
            .find_map(|ext| match ext {
                SqlExtension::VectorSearch { collection, top_k } => {
                    Some((collection.clone(), *top_k))
                }
                SqlExtension::VectorDistance { .. } => {
                    // Extract from query targets
                    query.targets.first()
                        .map(|t| (t.name.clone(), 10))
                }
                _ => None,
            })
            .unwrap_or(("default".to_string(), 10));

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::VectorSearch {
                collection,
                top_k,
                query_vector_source: VectorSource::Literal(vec![]),
            },
            estimated_cost: 10.0,
            estimated_rows: top_k as u64,
            output_columns: vec!["id".to_string(), "score".to_string(), "vector".to_string()],
        })
    }

    /// Plan a graph query
    fn plan_graph_query(&self, query: &FederatedQuery) -> Result<PlanNode> {
        let cypher = query.extensions.iter()
            .find_map(|ext| match ext {
                SqlExtension::GraphQuery { cypher } => Some(cypher.clone()),
                _ => None,
            })
            .unwrap_or_default();

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::GraphTraversal {
                cypher,
                start_nodes: None,
            },
            estimated_cost: 50.0,
            estimated_rows: 100,
            output_columns: vec!["*".to_string()],
        })
    }

    /// Plan a document query
    fn plan_document_query(&self, query: &FederatedQuery) -> Result<PlanNode> {
        let (collection, filter) = query.extensions.iter()
            .find_map(|ext| match ext {
                SqlExtension::DocumentQuery { collection, filter } => {
                    Some((collection.clone(), filter.clone()))
                }
                _ => None,
            })
            .unwrap_or(("default".to_string(), None));

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::DocumentQuery {
                collection,
                filter,
            },
            estimated_cost: 30.0,
            estimated_rows: 500,
            output_columns: vec!["id".to_string(), "doc".to_string()],
        })
    }

    /// Plan an observability query
    fn plan_observability_query(&self, query: &FederatedQuery) -> Result<PlanNode> {
        let (namespace, query_type) = query.extensions.iter()
            .find_map(|ext| match ext {
                SqlExtension::Logs { namespace } => {
                    Some((namespace.clone(), ObservabilityQueryType::Logs))
                }
                SqlExtension::Metrics { namespace } => {
                    Some((namespace.clone(), ObservabilityQueryType::Metrics))
                }
                _ => None,
            })
            .unwrap_or(("default".to_string(), ObservabilityQueryType::Logs));

        Ok(PlanNode {
            id: self.next_id(),
            node_type: PlanNodeType::ObservabilityQuery {
                namespace,
                query_type,
                time_range: None, // TODO: Extract from WHERE clause
            },
            estimated_cost: 20.0,
            estimated_rows: 1000,
            output_columns: vec!["timestamp".to_string(), "data".to_string()],
        })
    }

    /// Plan a federated (cross-model) query
    fn plan_federated_query(&self, query: &FederatedQuery) -> Result<PlanNode> {
        // Build sub-plans for each extension
        let mut sub_plans: Vec<PlanNode> = Vec::new();

        for ext in &query.extensions {
            let sub_plan = match ext {
                SqlExtension::VectorSearch { collection, top_k } => {
                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::VectorSearch {
                            collection: collection.clone(),
                            top_k: *top_k,
                            query_vector_source: VectorSource::Literal(vec![]),
                        },
                        estimated_cost: 10.0,
                        estimated_rows: *top_k as u64,
                        output_columns: vec!["id".to_string(), "score".to_string()],
                    }
                }
                SqlExtension::GraphQuery { cypher } => {
                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::GraphTraversal {
                            cypher: cypher.clone(),
                            start_nodes: None,
                        },
                        estimated_cost: 50.0,
                        estimated_rows: 100,
                        output_columns: vec!["*".to_string()],
                    }
                }
                SqlExtension::DocumentQuery { collection, filter } => {
                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::DocumentQuery {
                            collection: collection.clone(),
                            filter: filter.clone(),
                        },
                        estimated_cost: 30.0,
                        estimated_rows: 500,
                        output_columns: vec!["id".to_string(), "doc".to_string()],
                    }
                }
                SqlExtension::Logs { namespace } => {
                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::ObservabilityQuery {
                            namespace: namespace.clone(),
                            query_type: ObservabilityQueryType::Logs,
                            time_range: None,
                        },
                        estimated_cost: 20.0,
                        estimated_rows: 1000,
                        output_columns: vec!["timestamp".to_string(), "log".to_string()],
                    }
                }
                SqlExtension::Metrics { namespace } => {
                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::ObservabilityQuery {
                            namespace: namespace.clone(),
                            query_type: ObservabilityQueryType::Metrics,
                            time_range: None,
                        },
                        estimated_cost: 20.0,
                        estimated_rows: 500,
                        output_columns: vec!["timestamp".to_string(), "value".to_string()],
                    }
                }
                SqlExtension::VectorDistance { left_column, .. } => {
                    let target = query.targets.first()
                        .map(|t| t.name.clone())
                        .unwrap_or("default".to_string());
                    PlanNode {
                        id: self.next_id(),
                        node_type: PlanNodeType::VectorSearch {
                            collection: target,
                            top_k: 10,
                            query_vector_source: VectorSource::Literal(vec![]),
                        },
                        estimated_cost: 10.0,
                        estimated_rows: 10,
                        output_columns: vec!["id".to_string(), left_column.clone()],
                    }
                }
            };
            sub_plans.push(sub_plan);
        }

        // Add scans for regular tables
        for target in &query.targets {
            if target.model_type == TargetModelType::Table || target.model_type == TargetModelType::Unknown {
                sub_plans.push(PlanNode {
                    id: self.next_id(),
                    node_type: PlanNodeType::Scan {
                        target: target.name.clone(),
                        model_type: ModelType::Relational,
                        predicates: vec![],
                    },
                    estimated_cost: 100.0,
                    estimated_rows: 1000,
                    output_columns: vec!["*".to_string()],
                });
            }
        }

        // If we have multiple sub-plans, join them
        if sub_plans.len() >= 2 {
            let mut result = sub_plans.remove(0);
            for right in sub_plans {
                result = PlanNode {
                    id: self.next_id(),
                    node_type: if query.sql.to_uppercase().contains("LATERAL") {
                        PlanNodeType::NestedLoopJoin {
                            outer: Box::new(result),
                            inner: Box::new(right),
                            correlation: vec![],
                        }
                    } else {
                        PlanNodeType::HashJoin {
                            left: Box::new(result),
                            right: Box::new(right),
                            join_keys: vec![("id".to_string(), "id".to_string())],
                            join_type: JoinType::Inner,
                        }
                    },
                    estimated_cost: 200.0,
                    estimated_rows: 100,
                    output_columns: vec!["*".to_string()],
                };
            }
            Ok(result)
        } else if sub_plans.len() == 1 {
            Ok(sub_plans.remove(0))
        } else {
            Err(anyhow!("No valid plan nodes generated"))
        }
    }

    /// Apply optimization rules
    fn apply_optimizations(&self, plan: PlanNode) -> Result<PlanNode> {
        // TODO: Implement optimization rules
        // - Predicate pushdown
        // - Join reordering based on cost
        // - Projection pushdown
        // - Parallel execution identification
        Ok(plan)
    }

    /// Build physical plan from logical plan
    fn build_physical_plan(&self, plan: PlanNode) -> Result<QueryPlan> {
        let total_cost = self.calculate_total_cost(&plan);
        let involved_models = self.collect_involved_models(&plan);

        Ok(QueryPlan {
            root: plan,
            total_cost,
            metadata: PlanMetadata {
                involved_models: involved_models.clone(),
                is_cross_model: involved_models.len() > 1,
                parallel_stages: vec![],
                hints_applied: vec![],
            },
        })
    }

    /// Calculate total cost of a plan
    fn calculate_total_cost(&self, plan: &PlanNode) -> f64 {
        let mut cost = plan.estimated_cost;

        match &plan.node_type {
            PlanNodeType::HashJoin { left, right, .. } => {
                cost += self.calculate_total_cost(left);
                cost += self.calculate_total_cost(right);
            }
            PlanNodeType::NestedLoopJoin { outer, inner, .. } => {
                cost += self.calculate_total_cost(outer);
                cost += self.calculate_total_cost(inner) * outer.estimated_rows as f64;
            }
            PlanNodeType::IndexJoin { left, right, .. } => {
                cost += self.calculate_total_cost(left);
                cost += self.calculate_total_cost(right);
            }
            PlanNodeType::Filter { input, .. } => {
                cost += self.calculate_total_cost(input);
            }
            PlanNodeType::Project { input, .. } => {
                cost += self.calculate_total_cost(input);
            }
            PlanNodeType::Sort { input, .. } => {
                cost += self.calculate_total_cost(input);
            }
            PlanNodeType::Limit { input, .. } => {
                cost += self.calculate_total_cost(input);
            }
            PlanNodeType::Aggregate { input, .. } => {
                cost += self.calculate_total_cost(input);
            }
            PlanNodeType::Union { inputs, .. } => {
                for input in inputs {
                    cost += self.calculate_total_cost(input);
                }
            }
            _ => {}
        }

        cost
    }

    /// Collect all model types involved in the plan
    fn collect_involved_models(&self, plan: &PlanNode) -> Vec<ModelType> {
        let mut models = Vec::new();

        match &plan.node_type {
            PlanNodeType::Scan { model_type, .. } => {
                if !models.contains(model_type) {
                    models.push(*model_type);
                }
            }
            PlanNodeType::VectorSearch { .. } => {
                if !models.contains(&ModelType::Vector) {
                    models.push(ModelType::Vector);
                }
            }
            PlanNodeType::GraphTraversal { .. } => {
                if !models.contains(&ModelType::Graph) {
                    models.push(ModelType::Graph);
                }
            }
            PlanNodeType::DocumentQuery { .. } => {
                if !models.contains(&ModelType::Document) {
                    models.push(ModelType::Document);
                }
            }
            PlanNodeType::ObservabilityQuery { .. } => {
                if !models.contains(&ModelType::Observability) {
                    models.push(ModelType::Observability);
                }
            }
            PlanNodeType::HashJoin { left, right, .. } => {
                models.extend(self.collect_involved_models(left));
                models.extend(self.collect_involved_models(right));
            }
            PlanNodeType::NestedLoopJoin { outer, inner, .. } => {
                models.extend(self.collect_involved_models(outer));
                models.extend(self.collect_involved_models(inner));
            }
            PlanNodeType::IndexJoin { left, right, .. } => {
                models.extend(self.collect_involved_models(left));
                models.extend(self.collect_involved_models(right));
            }
            PlanNodeType::Filter { input, .. } |
            PlanNodeType::Project { input, .. } |
            PlanNodeType::Sort { input, .. } |
            PlanNodeType::Limit { input, .. } |
            PlanNodeType::Aggregate { input, .. } => {
                models.extend(self.collect_involved_models(input));
            }
            PlanNodeType::Union { inputs, .. } => {
                for input in inputs {
                    models.extend(self.collect_involved_models(input));
                }
            }
        }

        // Deduplicate
        models.sort_by_key(|m| format!("{:?}", m));
        models.dedup();
        models
    }

    /// Get next node ID
    fn next_id(&self) -> usize {
        self.next_node_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for CrossModelOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimizer_creation() {
        let optimizer = CrossModelOptimizer::new();
        assert!(optimizer.cost_models.contains_key(&ModelType::Vector));
        assert!(optimizer.cost_models.contains_key(&ModelType::Graph));
    }

    #[test]
    fn test_optimize_sql_query() {
        let optimizer = CrossModelOptimizer::new();
        let query = FederatedQuery {
            sql: "SELECT * FROM users".to_string(),
            query_type: QueryType::Sql,
            extensions: vec![],
            targets: vec![super::super::parser::QueryTarget {
                name: "users".to_string(),
                alias: None,
                model_type: TargetModelType::Table,
            }],
            parameters: HashMap::new(),
            is_cross_model_join: false,
        };

        let plan = optimizer.optimize(&query).unwrap();
        assert!(plan.total_cost > 0.0);
        assert!(!plan.metadata.is_cross_model);
    }

    #[test]
    fn test_optimize_vector_search() {
        let optimizer = CrossModelOptimizer::new();
        let query = FederatedQuery {
            sql: "SELECT * FROM VECTOR_SEARCH('embeddings', '[0.1]', 10)".to_string(),
            query_type: QueryType::VectorSearch,
            extensions: vec![SqlExtension::VectorSearch {
                collection: "embeddings".to_string(),
                top_k: 10,
            }],
            targets: vec![],
            parameters: HashMap::new(),
            is_cross_model_join: false,
        };

        let plan = optimizer.optimize(&query).unwrap();
        assert!(plan.metadata.involved_models.contains(&ModelType::Vector));
    }

    #[test]
    fn test_cost_model_defaults() {
        let cost_model = CostModel::default();
        assert_eq!(cost_model.row_scan_cost, 1.0);
        assert_eq!(cost_model.index_lookup_cost, 0.1);
    }
}
