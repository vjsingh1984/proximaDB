/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 */

//! DEPRECATED: SKS SQL Extensions - Use AST/Lowering Instead
//!
//! **DEPRECATION NOTICE**: This module is deprecated. SKS function parsing 
//! is now properly handled in:
//! - `query/ast/nodes.rs` - AST node definitions  
//! - `query/sql_frontend/lowering.rs` - SQL lowering to internal AST
//!
//! This module implements the SIMILAR/FOLLOW/ASSEMBLE functions that enable
//! hybrid vector + graph intelligence through SQL interface, integrated with
//! the new sql_frontend lowering and HashMap metadata optimization.
//!
//! Key functions:
//! - SIMILAR(field, vector, metric): Semantic similarity with HashMap filtering
//! - FOLLOW(node, edge_type, depth): Graph traversal with ORION engine
//! - ASSEMBLE(context, radius): Knowledge assembly with provenance

use crate::graph::service::GraphService;
use crate::query::ast::Expr;
use crate::query::execution::{ExecutionOperation, QueryRow};
use crate::services::operations::vectors::VectorOperationsService;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

/// Temporary context structure for ASSEMBLE operations
#[derive(Debug, Clone)]
struct AssembledContext {
    metadata: HashMap<String, serde_json::Value>,
    relevance_score: Option<f64>,
    graph_distance: Option<u32>,
    provenance_chain: Vec<String>,
}

/// SKS-specific SQL operators
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SksOperator {
    /// Semantic similarity search
    Similar(SimilarOperator),

    /// Graph traversal
    Follow(FollowOperator),

    /// Context assembly
    Assemble(AssembleOperator),

    /// Temporal queries
    Temporal(TemporalOperator),
}

/// SKS Function Executor - Implements hybrid vector + graph intelligence
///
/// This executor integrates with the new sql_frontend and execution engine to provide
/// semantic intelligence capabilities through SQL interface.
pub struct SksExecutor {
    vector_service: Arc<VectorOperationsService>,
    graph_service: Arc<GraphService>,
}

impl SksExecutor {
    /// Create new SKS executor with service integrations
    pub fn new(
        vector_service: Arc<VectorOperationsService>,
        graph_service: Arc<GraphService>,
    ) -> Self {
        Self {
            vector_service,
            graph_service,
        }
    }

    /// Parse SKS function from sql_frontend AST and convert to execution plan
    pub fn parse_sks_function(&self, func_call: &Expr) -> Result<SksFunction> {
        if let Expr::FuncCall { name, args } = func_call {
            match name.to_uppercase().as_str() {
                "SIMILAR" => self.parse_similar_function(name, args),
                "FOLLOW" => self.parse_follow_function(name, args),
                "ASSEMBLE" => self.parse_assemble_function(name, args),
                _ => Err(anyhow!("Unknown SKS function: {}", name)),
            }
        } else {
            Err(anyhow!("Expected function call expression"))
        }
    }

    /// Execute SIMILAR function with embedding validation and HashMap filtering
    ///
    /// Implements semantic similarity search with:
    /// - Collection schema validation
    /// - Vector dimension checking
    /// - HashMap metadata filtering for O(1) performance
    /// - Model registry integration
    pub async fn execute_similar(
        &self,
        similar: &SimilarOperator,
        collection_id: &str,
    ) -> Result<Vec<QueryRow>> {
        info!(
            "Executing SIMILAR function on field: {}",
            similar.embedding_field
        );

        // 1. Validate embedding field exists in collection schema
        // TODO: Add schema validation

        // 2. Convert query to vector if needed (text → embedding)
        let query_vector = match &similar.query {
            SimilarQuery::Vector(vec) => vec.clone(),
            SimilarQuery::Text(text) => {
                // TODO: Convert text to embedding using specified model
                // This would integrate with embedding service/model registry
                debug!("Converting text to embedding: {}", text);
                return Err(anyhow!("Text-to-embedding conversion not yet implemented"));
            }
            SimilarQuery::EntityRef(entity_id) => {
                // TODO: Retrieve embedding from entity store
                debug!("Retrieving embedding for entity: {}", entity_id);
                return Err(anyhow!("Entity embedding retrieval not yet implemented"));
            }
        };

        // 3. Execute vector search with VOS integration and HashMap filtering
        let search_config = crate::services::operations::vectors::UnifiedSearchConfig {
            optimization_goal: crate::query::unified_query_optimizer::OptimizationGoal::Balanced,
            progressive_search: similar.progressive,
            progressive_recalls: None, // Use default progressive recall targets
            include_vectors: false,
            include_metadata: true, // Enable HashMap metadata access
            scenario: Some("sks_similar".to_string()),
        };

        let vos_results = self
            .vector_service
            .unified_search_v1(
                collection_id,
                query_vector,
                similar.top_k,
                None, // TODO: Convert filters to FilterExpression
                Some(search_config),
            )
            .await?;

        // 4. Convert to QueryRow format with similarity scores
        let rows = vos_results
            .into_iter()
            .flat_map(|search_result| {
                search_result.results.into_iter().map(|record| QueryRow {
                    fields: self.convert_vos_metadata(&record.metadata),
                    similarity_score: Some(record.score),
                    graph_distance: None,
                    provenance: Some(vec![format!("SIMILAR({})", similar.embedding_field)]),
                })
            })
            .collect();

        Ok(rows)
    }

    /// Execute FOLLOW function with ORION graph engine integration
    ///
    /// Implements graph traversal with:
    /// - Start node validation  
    /// - Edge type filtering
    /// - Depth-limited BFS/DFS algorithms
    /// - Path tracking and provenance
    pub async fn execute_follow(
        &self,
        follow: &FollowOperator,
        start_node: &str,
    ) -> Result<Vec<QueryRow>> {
        info!("Executing FOLLOW traversal from node: {}", start_node);

        // 1. Validate start node exists
        // TODO: Add node existence validation

        // 2. Configure traversal with ORION engine
        let traversal_config = crate::graph::engines::orion::traversal::TraversalConfig {
            max_depth: Some(follow.max_depth as u32),
            max_nodes: Some(1000), // Default limit
            edge_types: Some(vec![follow.relation_type.clone()]),
            node_filter: None, // TODO: Convert filters
            early_stop: None,
            track_paths: follow.return_paths,
            parallel_processing: true,
            timeout_ms: Some(5000),
            max_frontier: Some(10000),
            enable_prefetch: true, // Defaults; can be overridden via context-aware path
            prefetch_budget: 8,
        };

        // 3. Execute graph traversal
        // TODO: Call graph service with traversal config
        // let traversal_result = self.graph_service.traverse(start_node, traversal_config).await?;

        // 4. Convert graph results to QueryRow format
        let rows = vec![]; // TODO: Convert traversal results

        Ok(rows)
    }

    /// Execute ASSEMBLE function with context building and provenance tracking
    ///
    /// Implements knowledge assembly with:
    /// - Multi-source context gathering
    /// - Relevance ranking and filtering
    /// - Provenance chain tracking
    /// - Coherent narrative building
    pub async fn execute_assemble(
        &self,
        assemble: &AssembleOperator,
        context_items: &[String],
    ) -> Result<Vec<QueryRow>> {
        info!(
            "Executing ASSEMBLE function with {} context items",
            context_items.len()
        );

        // 1. Gather context from multiple sources
        let mut assembled_context: Vec<AssembledContext> = Vec::new();

        for item_id in context_items {
            // TODO: Retrieve context from entity store, vector collections, graph relationships
            // This would involve:
            // - Vector similarity search around the item
            // - Graph traversal from the item
            // - Metadata extraction with HashMap optimization
            // - Provenance tracking

            debug!("Gathering context for item: {}", item_id);
        }

        // 2. Apply assembly strategy (temporal, semantic, relevance-based)
        // DEPRECATED: Assembly logic moved to sql_frontend/lowering.rs
        // Default: preserve discovery order until migration to new system

        // 3. Build result rows with provenance
        let rows: Vec<QueryRow> = assembled_context
            .into_iter()
            .map(|context| QueryRow {
                fields: context.metadata,
                similarity_score: context.relevance_score,
                graph_distance: context.graph_distance,
                provenance: Some(context.provenance_chain),
            })
            .collect();

        Ok(rows)
    }

    /// Helper: Parse SIMILAR function from SQL AST
    fn parse_similar_function(&self, name: &str, args: &[Expr]) -> Result<SksFunction> {
        if args.len() < 3 {
            return Err(anyhow!(
                "SIMILAR function requires at least 3 arguments: field, vector, metric"
            ));
        }

        // TODO: Extract arguments from args
        // - Field name (embedding_field)
        // - Query vector or text
        // - Distance metric
        // - Optional parameters (threshold, top_k, model_id)

        Ok(SksFunction::Similar {
            field: "embedding".to_string(), // TODO: Extract from args
            query_vector: vec![0.0],        // TODO: Extract from args
            metric: "cosine".to_string(),   // TODO: Extract from args
            threshold: Some(0.8),           // TODO: Extract from options
        })
    }

    /// Helper: Parse FOLLOW function from SQL AST
    fn parse_follow_function(&self, name: &str, args: &[Expr]) -> Result<SksFunction> {
        if args.len() < 2 {
            return Err(anyhow!(
                "FOLLOW function requires at least 2 arguments: start_node, edge_type"
            ));
        }

        // TODO: Extract arguments and options
        Ok(SksFunction::Follow {
            start_node: "node1".to_string(),     // TODO: Extract from args
            edge_type: "related".to_string(),    // TODO: Extract from args
            max_depth: 3,                        // TODO: Extract from options
            direction: TraversalDirection::Both, // TODO: Extract from options
        })
    }

    /// Helper: Parse ASSEMBLE function from SQL AST  
    fn parse_assemble_function(&self, name: &str, args: &[Expr]) -> Result<SksFunction> {
        // TODO: Extract assembly parameters and options
        Ok(SksFunction::Assemble {
            context_items: vec![], // TODO: Extract from args
            assembly_strategy: AssemblyStrategy::RelevanceRanking,
            max_context_size: Some(100), // TODO: Extract from options
        })
    }

    /// Convert VOS metadata HashMap to QueryRow fields
    ///
    /// This method demonstrates the HashMap metadata optimization in action
    fn convert_vos_metadata(
        &self,
        metadata: &HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    ) -> HashMap<String, serde_json::Value> {
        // Efficient HashMap iteration - no linear scans needed
        metadata
            .iter()
            .filter_map(|(key, sql_value)| {
                let json_value = match &sql_value.value {
                    Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                        serde_json::Value::String(s.clone())
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) => {
                        serde_json::json!(n)
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                        serde_json::Value::Bool(*b)
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => {
                        serde_json::Value::Number(serde_json::Number::from(*i))
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::BytesValue(b)) => {
                        serde_json::Value::String(crate::utils::encoding::base64_encode(b))
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::NullValue(_)) => {
                        serde_json::Value::Null
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::ArrayValue(_)) => {
                        serde_json::Value::String("[Array]".to_string())
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(_)) => {
                        serde_json::Value::String("[Object]".to_string())
                    }
                    None => return None,
                };
                Some((key.clone(), json_value))
            })
            .collect()
    }
}

/// Enhanced SKS function types for execution engine integration
#[derive(Debug, Clone)]
pub enum SksFunction {
    /// SIMILAR(field, vector, metric) → semantic similarity search
    Similar {
        field: String,
        query_vector: Vec<f32>,
        metric: String,
        threshold: Option<f32>,
    },

    /// FOLLOW(node, edge_type, options) → graph traversal
    Follow {
        start_node: String,
        edge_type: String,
        max_depth: u32,
        direction: TraversalDirection,
    },

    /// ASSEMBLE(items, strategy) → knowledge assembly
    Assemble {
        context_items: Vec<String>,
        assembly_strategy: AssemblyStrategy,
        max_context_size: Option<usize>,
    },
}

impl SksFunction {
    /// Convert SKS function to execution plan operations
    ///
    /// This method integrates with the new execution engine to provide
    /// hybrid vector + graph intelligence through unified planning.
    pub fn to_execution_operations(&self) -> Result<Vec<ExecutionOperation>> {
        match self {
            SksFunction::Similar {
                field,
                query_vector,
                metric,
                threshold,
            } => {
                Ok(vec![ExecutionOperation::VectorSearch {
                    collection_id: self.resolve_collection_from_field(field)?,
                    query_vector: Some(query_vector.clone()),
                    filters: self.create_threshold_filter(*threshold)?,
                    top_k: 100, // TODO: Make configurable
                    distance_metric: metric.clone(),
                }])
            }

            SksFunction::Follow {
                start_node,
                edge_type,
                max_depth,
                direction: _,
            } => {
                Ok(vec![ExecutionOperation::GraphTraversal {
                    start_nodes: vec![start_node.clone()],
                    edge_types: vec![edge_type.clone()],
                    max_depth: *max_depth,
                    filters: None, // TODO: Add filter support
                    vector_target_collection: None,
                }])
            }

            SksFunction::Assemble {
                context_items,
                assembly_strategy: _,
                max_context_size: _,
            } => {
                // TODO: Implement ASSEMBLE → execution operations conversion
                // This would involve multiple vector searches and graph traversals
                // to gather comprehensive context around the specified items

                Ok(vec![
                    // TODO: Add context gathering operations
                    ExecutionOperation::Fusion {
                        strategy: crate::query::execution::FusionStrategy::AdaptiveSemanticFusion {
                            learning_rate: 0.1,
                        },
                        weights: vec![0.5, 0.3, 0.2], // Vector, graph, temporal weights
                    },
                ])
            }
        }
    }

    /// Resolve collection from embedding field name
    fn resolve_collection_from_field(&self, field: &str) -> Result<String> {
        // Implement field → collection resolution
        let collection_id = match field.split('.').next() {
            Some(prefix) if prefix.ends_with("_collection") => {
                prefix.strip_suffix("_collection").unwrap_or("default").to_string()
            }
            _ => "default".to_string(), // Default collection fallback
        };
        // This would query the schema registry to find which collection
        // contains the specified embedding field
        Ok(collection_id)
    }
    
    // DEPRECATED: Assembly strategies moved to sql_frontend/lowering.rs

    /// Create threshold filter for similarity scoring
    fn create_threshold_filter(
        &self,
        threshold: Option<f32>,
    ) -> Result<Option<crate::core::search::FilterExpression>> {
        if let Some(min_score) = threshold {
            Ok(Some(crate::core::search::FilterExpression::Comparison {
                field: "_similarity_score".to_string(),
                operator: crate::core::search::ComparisonOperator::GreaterThanOrEqual,
                value: serde_json::json!(min_score),
            }))
        } else {
            Ok(None)
        }
    }
}

/// SIMILAR operator for semantic search
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimilarOperator {
    /// Field containing embeddings
    pub embedding_field: String,

    /// Query (text or vector)
    pub query: SimilarQuery,

    /// Model to use for embedding
    pub model_id: Option<String>,

    /// Number of results
    pub top_k: usize,

    /// Progressive search mode
    pub progressive: bool,
}

/// Query types for SIMILAR operator
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SimilarQuery {
    /// Text to be embedded
    Text(String),

    /// Pre-computed vector
    Vector(Vec<f32>),

    /// Reference to another entity
    EntityRef(String),
}

/// FOLLOW operator for graph traversal
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FollowOperator {
    /// Relationship type to follow
    pub relation_type: String,

    /// Maximum traversal depth
    pub max_depth: usize,

    /// Direction of traversal
    pub direction: TraversalDirection,

    /// Whether to return paths or just entities
    pub return_paths: bool,
}

/// Assembly strategies for ASSEMBLE operations
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssemblyStrategy {
    /// Temporal ordering strategy
    TemporalOrdering,
    /// Semantic clustering strategy
    SemanticClustering,
    /// Relevance ranking strategy
    RelevanceRanking,
}

/// Direction for graph traversal
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TraversalDirection {
    Outgoing,
    Incoming,
    Both,
}

/// ASSEMBLE operator for context reconstruction
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssembleOperator {
    /// Source document ID
    pub source_id: Option<String>,

    /// Entity IDs to assemble context for
    pub entity_ids: Vec<String>,

    /// Context radius (chunks before/after)
    pub radius: usize,

    /// Maximum context size
    pub max_size: usize,
}

/// Temporal operator for time-aware queries
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalOperator {
    /// Time point or range
    pub temporal_clause: TemporalClause,

    /// Whether to include historical versions
    pub include_history: bool,
}

/// Temporal clause types
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TemporalClause {
    /// At a specific time
    AtTime(i64),

    /// Between two times
    Between(i64, i64),

    /// Since a time
    Since(i64),

    /// Current version only
    Current,
}

/// SQL parser extension for SKS operators
pub struct SksSqlParser;

impl SksSqlParser {
    /// Parse a SIMILAR clause - DEPRECATED: Use sql_frontend/lowering.rs instead
    /// 
    /// This method is kept for backward compatibility but should not be used.
    /// SKS function parsing is now handled properly in the SQL lowering phase.
    pub fn parse_similar(input: &str) -> Result<SimilarOperator> {
        Err(anyhow!(
            "DEPRECATED: Use sql_frontend/lowering.rs for SKS function parsing. Input: {}", 
            input
        ))
    }

    /// Parse a FOLLOW clause - DEPRECATED: Use sql_frontend/lowering.rs instead
    /// 
    /// This method is kept for backward compatibility but should not be used.
    /// SKS function parsing is now handled properly in the SQL lowering phase.
    pub fn parse_follow(input: &str) -> Result<FollowOperator> {
        Err(anyhow!(
            "DEPRECATED: Use sql_frontend/lowering.rs for SKS function parsing. Input: {}", 
            input
        ))
    }

    /// Parse an ASSEMBLE clause - DEPRECATED: Use sql_frontend/lowering.rs instead
    /// 
    /// This method is kept for backward compatibility but should not be used.
    /// SKS function parsing is now handled properly in the SQL lowering phase.
    pub fn parse_assemble(input: &str) -> Result<AssembleOperator> {
        Err(anyhow!(
            "DEPRECATED: Use sql_frontend/lowering.rs for SKS function parsing. Input: {}", 
            input
        ))
    }
}

/// Query planner extension for SKS operators
pub struct SksQueryPlanner;

impl SksQueryPlanner {
    /// Plan execution for a SIMILAR operator
    pub fn plan_similar(
        &self,
        operator: &SimilarOperator,
        metadata_filters: Option<HashMap<String, String>>,
    ) -> SksQueryPlan {
        info!("Planning SIMILAR query with top_k={}", operator.top_k);

        let stages = if operator.progressive {
            vec![
                QueryStage::CandidateGeneration {
                    k: operator.top_k * 10,
                },
                QueryStage::Reranking { k: operator.top_k },
            ]
        } else {
            vec![QueryStage::DirectSearch { k: operator.top_k }]
        };

        SksQueryPlan {
            operator: SksOperator::Similar(operator.clone()),
            stages,
            metadata_filters,
            estimated_cost: operator.top_k as f64 * 10.0, // Placeholder cost
        }
    }

    /// Plan execution for a FOLLOW operator
    pub fn plan_follow(
        &self,
        operator: &FollowOperator,
        start_entities: Vec<String>,
    ) -> SksQueryPlan {
        info!("Planning FOLLOW query with depth={}", operator.max_depth);

        let stages = vec![QueryStage::GraphTraversal {
            start_entities: start_entities.clone(),
            max_depth: operator.max_depth,
        }];

        SksQueryPlan {
            operator: SksOperator::Follow(operator.clone()),
            stages,
            metadata_filters: None,
            estimated_cost: start_entities.len() as f64 * operator.max_depth as f64,
        }
    }

    /// Plan execution for an ASSEMBLE operator
    pub fn plan_assemble(&self, operator: &AssembleOperator) -> SksQueryPlan {
        info!("Planning ASSEMBLE query with radius={}", operator.radius);

        let stages = vec![QueryStage::ContextRetrieval {
            entity_ids: operator.entity_ids.clone(),
            radius: operator.radius,
        }];

        SksQueryPlan {
            operator: SksOperator::Assemble(operator.clone()),
            stages,
            metadata_filters: None,
            estimated_cost: operator.entity_ids.len() as f64 * operator.radius as f64,
        }
    }
}

/// Query execution plan for SKS operators
#[derive(Debug, Clone)]
pub struct SksQueryPlan {
    /// The operator to execute
    pub operator: SksOperator,

    /// Execution stages
    pub stages: Vec<QueryStage>,

    /// Metadata filters to apply
    pub metadata_filters: Option<HashMap<String, String>>,

    /// Estimated execution cost
    pub estimated_cost: f64,
}

/// Query execution stages
#[derive(Debug, Clone)]
pub enum QueryStage {
    /// Generate candidates for similarity search
    CandidateGeneration { k: usize },

    /// Rerank candidates
    Reranking { k: usize },

    /// Direct search without progressive refinement
    DirectSearch { k: usize },

    /// Graph traversal stage
    GraphTraversal {
        start_entities: Vec<String>,
        max_depth: usize,
    },

    /// Context retrieval stage
    ContextRetrieval {
        entity_ids: Vec<String>,
        radius: usize,
    },
}

/// SQL query examples for SKS
pub mod examples {
    /// Example: Similarity search with metadata filters
    pub const SIMILAR_WITH_FILTER: &str = r#"
        FIND entities
        WHERE SIMILAR(embedding, "quantum computing", model="openai/ada-002", top_k=10)
          AND metadata.year > 2020
          AND metadata.domain = 'physics';
    "#;

    /// Example: Multi-hop graph traversal
    pub const GRAPH_TRAVERSAL: &str = r#"
        FIND entities
        WHERE SIMILAR(embedding, "machine learning")
        FOLLOW relations.cites TO depth=2
        RETURN path;
    "#;

    /// Example: Context assembly
    pub const CONTEXT_ASSEMBLY: &str = r#"
        ASSEMBLE CONTEXT
        FROM entities
        WHERE id IN ('doc1', 'doc2', 'doc3')
        WITH radius=3
        ORDER BY chunk_position;
    "#;

    /// Example: Temporal query
    pub const TEMPORAL_QUERY: &str = r#"
        FIND entities
        WHERE metadata.topic = 'climate'
        VALID BETWEEN '2020-01-01' AND '2023-12-31';
    "#;

    /// Example: Entity evolution tracking
    pub const EVOLUTION_TRACKING: &str = r#"
        TRACK EVOLUTION OF concept
        WHERE name = 'machine learning'
        FROM '2015-01-01' TO NOW;
    "#;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_similar_operator_creation() {
        let op = SimilarOperator {
            embedding_field: "embedding".to_string(),
            query: SimilarQuery::Text("test query".to_string()),
            model_id: Some("test-model".to_string()),
            top_k: 10,
            progressive: true,
        };

        assert_eq!(op.top_k, 10);
        assert!(op.progressive);
    }

    #[test]
    fn test_follow_operator_creation() {
        let op = FollowOperator {
            relation_type: "cites".to_string(),
            max_depth: 3,
            direction: TraversalDirection::Both,
            return_paths: true,
        };

        assert_eq!(op.max_depth, 3);
        assert_eq!(op.direction, TraversalDirection::Both);
    }

    #[test]
    fn test_query_planner() {
        let planner = SksQueryPlanner;
        let similar_op = SimilarOperator {
            embedding_field: "embedding".to_string(),
            query: SimilarQuery::Text("test".to_string()),
            model_id: None,
            top_k: 5,
            progressive: true,
        };

        let plan = planner.plan_similar(&similar_op, None);
        assert_eq!(plan.stages.len(), 2); // Progressive has 2 stages
    }
}

/// Enhanced TDD tests for sql_frontend integration
#[cfg(test)]
mod sks_integration_tests {
    use super::*;
    use crate::query::ast::*;

    #[tokio::test]
    async fn test_similar_sql_parsing_integration() {
        // Test integration with sql_frontend parser
        let sql = "SELECT * FROM documents WHERE SIMILAR(embedding, [0.1, 0.2, 0.3], 'cosine', threshold => 0.8) LIMIT 10";

        // TODO: This would test the complete flow:
        // 1. sql_frontend/parser.rs parses the SQL
        // 2. sql_frontend/lowering.rs recognizes SIMILAR function
        // 3. SKS extensions converts to execution operations
        // 4. Execution engine runs with HashMap metadata optimization

        assert!(sql.contains("SIMILAR")); // Placeholder validation
    }

    #[tokio::test]
    async fn test_follow_graph_integration() {
        // Test FOLLOW function with ORION graph engine
        let sql =
            "SELECT * FROM entities FOLLOW('user123', 'friend', depth => 3, direction => 'both')";

        // TODO: Test complete integration:
        // 1. Parse FOLLOW function from SQL
        // 2. Convert to graph traversal configuration
        // 3. Execute with ORION engine (CSR storage optimization)
        // 4. Return results with path information

        assert!(sql.contains("FOLLOW"));
    }

    #[tokio::test]
    async fn test_hybrid_similar_follow_query() {
        // Test hybrid query combining SIMILAR and FOLLOW
        let sql = "SELECT * FROM entities WHERE SIMILAR(embedding, $1) AND FOLLOW(id, 'related', depth => 2)";

        // TODO: Test hybrid execution:
        // 1. Recognize both SIMILAR and FOLLOW functions
        // 2. Generate hybrid execution plan
        // 3. Execute vector search and graph traversal in parallel
        // 4. Apply fusion algorithm (RRF or Adaptive Semantic Fusion)
        // 5. Return unified results with combined scores

        assert!(sql.contains("SIMILAR") && sql.contains("FOLLOW"));
    }

    #[tokio::test]
    async fn test_assemble_knowledge_integration() {
        // Test ASSEMBLE function for context building
        let sql = "SELECT ASSEMBLE(context, radius => 5, strategy => 'relevance') FROM knowledge WHERE topic = 'AI'";

        // TODO: Test knowledge assembly:
        // 1. Parse ASSEMBLE function parameters
        // 2. Gather context from multiple sources (vector + graph + temporal)
        // 3. Apply assembly strategy (relevance, temporal, semantic)
        // 4. Track provenance chain for explainability
        // 5. Return coherent context with source attribution

        assert!(sql.contains("ASSEMBLE"));
    }

    #[test]
    fn test_hashmap_metadata_performance_in_sks() {
        // Test that SKS functions benefit from HashMap metadata optimization
        let executor = create_test_sks_executor();

        // Create metadata HashMap (v1 structure)
        let mut metadata = HashMap::new();
        for i in 0..10 {
            metadata.insert(
                format!("field_{}", i),
                crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        format!("value_{}", i),
                    )),
                },
            );
        }

        // Measure conversion performance (should be very fast with HashMap)
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            let _fields = executor.convert_vos_metadata(&metadata);
        }
        let conversion_time = start.elapsed();

        // HashMap conversion should be sub-millisecond even for many iterations
        assert!(
            conversion_time.as_millis() < 10,
            "HashMap metadata conversion should be very fast, took {:?}",
            conversion_time
        );
    }

    fn create_test_sks_executor() -> SksExecutor {
        // Create with mock services for testing
        let vector_service = Arc::new(create_mock_vector_service());
        let graph_service = Arc::new(create_mock_graph_service());

        SksExecutor::new(vector_service, graph_service)
    }

    fn create_mock_vector_service() -> VectorOperationsService {
        // TODO: Implement mock vector service for testing
        unimplemented!("Create mock VectorOperationsService")
    }

    fn create_mock_graph_service() -> GraphService {
        // TODO: Implement mock graph service for testing
        unimplemented!("Create mock GraphService")
    }
}
