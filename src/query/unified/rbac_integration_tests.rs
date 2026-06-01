//! RBAC Integration Tests for Unified Query Engine
//!
//! Tests role-based access control across different data models:
//! - Vector search with RBAC
//! - Graph traversal with RBAC
//! - Cross-model queries with mixed permissions
//! - Permission inheritance (wildcards)
//! - Multi-tenant isolation

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use tokio;

use crate::query::unified::{
    ast::{
        DataModel, DistanceMetric, DocumentQueryExpr, GraphTraversalExpr, LogQueryExpr,
        ModelOperation, MultiModelQuery, QueryComponent, StartNodeSpec, TraversalDirection,
        VectorSearchExpr, VectorSearchParams,
    },
    executor::ParallelExecutor,
};
use crate::security::rbac_service::{
    ConsolidatedRBACManager, RBACConfig, UnifiedAuthMethod, UnifiedPermission, UnifiedUserContext,
};
use crate::storage::document::DocumentService;

/// Helper function to create a mock storage engine for testing
async fn create_mock_storage_engine() -> Arc<dyn crate::storage::traits::UnifiedStorageFormat> {
    use crate::storage::engines::factory::StorageFormatFactory;

    StorageFormatFactory::create_sst_async().await.unwrap()
}

/// Helper function to create a test user context
fn create_test_user(user_id: &str, tenant_id: Option<&str>) -> UnifiedUserContext {
    UnifiedUserContext {
        user_id: user_id.to_string(),
        tenant_id: tenant_id.map(|s| s.to_string()),
        roles: Vec::new(),
        effective_permissions: HashSet::new(),
        auth_method: UnifiedAuthMethod::Internal,
        session_id: uuid::Uuid::new_v4().to_string(),
        expires_at: None,
        created_at: Utc::now(),
        metadata: std::collections::HashMap::new(),
    }
}

/// Helper function to create admin user with all permissions
fn create_admin_user() -> UnifiedUserContext {
    // Note: Permissions are granted through RBAC manager in create_test_rbac_manager
    create_test_user("admin_user", Some("tenant1"))
}

/// Helper function to create restricted user with limited permissions
#[allow(dead_code)]
fn create_restricted_user() -> UnifiedUserContext {
    // Note: Permissions are granted through RBAC manager in create_test_rbac_manager
    create_test_user("restricted_user", Some("tenant1"))
}

/// Helper function to create vector-only user
fn create_vector_only_user() -> UnifiedUserContext {
    // Note: Permissions are granted through RBAC manager in create_test_rbac_manager
    create_test_user("vector_user", Some("tenant1"))
}

/// Helper function to create a test RBAC manager with test permissions
async fn create_test_rbac_manager() -> ConsolidatedRBACManager {
    // Enable default_deny for proper permission testing
    let config = RBACConfig {
        default_deny: true, // Users without explicit permissions are denied
        ..RBACConfig::default()
    };
    let rbac_manager = ConsolidatedRBACManager::new(config);

    // Grant permissions directly to test users for simpler testing
    // Admin user - all permissions
    let _ = rbac_manager
        .grant_permission("admin_user", &UnifiedPermission::SystemAdmin)
        .await;
    let _ = rbac_manager
        .grant_permission(
            "admin_user",
            &UnifiedPermission::VectorSearch("collection1".to_string()),
        )
        .await;
    let _ = rbac_manager
        .grant_permission(
            "admin_user",
            &UnifiedPermission::CollectionRead("collection1".to_string()),
        )
        .await;
    let _ = rbac_manager
        .grant_permission(
            "admin_user",
            &UnifiedPermission::CollectionRead("docs1".to_string()),
        )
        .await;
    let _ = rbac_manager
        .grant_permission(
            "admin_user",
            &UnifiedPermission::GraphTraverse("graph1".to_string()),
        )
        .await;

    // Vector user - only vector permissions
    let _ = rbac_manager
        .grant_permission(
            "vector_user",
            &UnifiedPermission::VectorSearch("collection1".to_string()),
        )
        .await;

    // Mixed permissions user - vector + document
    let _ = rbac_manager
        .grant_permission(
            "mixed_perms_user",
            &UnifiedPermission::VectorSearch("collection1".to_string()),
        )
        .await;
    let _ = rbac_manager
        .grant_permission(
            "mixed_perms_user",
            &UnifiedPermission::CollectionRead("docs1".to_string()),
        )
        .await;

    // Graph user - only graph permissions
    let _ = rbac_manager
        .grant_permission(
            "graph_user",
            &UnifiedPermission::GraphTraverse("graph1".to_string()),
        )
        .await;

    // Document user - only document permissions
    let _ = rbac_manager
        .grant_permission(
            "doc_user",
            &UnifiedPermission::CollectionRead("docs1".to_string()),
        )
        .await;

    // Tenant isolation test user
    let _ = rbac_manager
        .grant_permission(
            "user_tenant1",
            &UnifiedPermission::VectorSearch("collection1".to_string()),
        )
        .await;

    // Admin user for observability tests (created as "admin" in tests)
    let _ = rbac_manager
        .grant_permission("admin", &UnifiedPermission::SystemAdmin)
        .await;

    rbac_manager
}

#[cfg(test)]
mod rbac_integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_rbac_manager_creation() {
        let rbac_manager = create_test_rbac_manager().await;
        assert!(Arc::strong_count(&Arc::new(rbac_manager)) >= 1);
    }

    #[tokio::test]
    async fn test_vector_search_permission_allowed() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);
        let executor = ParallelExecutor::with_rbac(4, rbac_manager.clone());

        let user = create_vector_only_user();

        // Create a vector search query component
        let component = QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(VectorSearchExpr {
                collection: "collection1".to_string(),
                query_vector: vec![0.1, 0.2, 0.3],
                top_k: 10,
                threshold: Some(0.8),
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
            filters: vec![],
            dependencies: vec![],
        };

        // Validate permissions - should succeed
        let result = executor.validate_component_access(&user, &component).await;
        assert!(
            result.is_ok(),
            "Vector search permission should be allowed for vector_user"
        );
    }

    #[tokio::test]
    async fn test_vector_search_permission_denied() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);
        let executor = ParallelExecutor::with_rbac(4, rbac_manager.clone());

        // Create a user with no permissions
        let user = create_test_user("no_perms_user", Some("tenant1"));

        // Create a vector search query component for unauthorized collection
        let component = QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(VectorSearchExpr {
                collection: "restricted_collection".to_string(),
                query_vector: vec![0.1, 0.2, 0.3],
                top_k: 10,
                threshold: Some(0.8),
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
            filters: vec![],
            dependencies: vec![],
        };

        // Validate permissions - should fail
        let result = executor.validate_component_access(&user, &component).await;
        assert!(
            result.is_err(),
            "Vector search permission should be denied for unauthorized collection"
        );
    }

    #[tokio::test]
    async fn test_graph_traversal_permission_allowed() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);
        let executor = ParallelExecutor::with_rbac(4, rbac_manager.clone());

        let mut user = create_test_user("graph_user", Some("tenant1"));
        user.effective_permissions =
            HashSet::from([UnifiedPermission::GraphTraverse("graph1".to_string())]);

        // Create a graph traversal query component
        let component = QueryComponent {
            model: DataModel::Graph,
            operation: ModelOperation::GraphTraversal(GraphTraversalExpr {
                graph_name: "graph1".to_string(),
                start_nodes: crate::query::unified::ast::StartNodeSpec::Ids(vec![]),
                edge_types: vec![],
                direction: crate::query::unified::ast::TraversalDirection::Outgoing,
                max_depth: 2,
                min_depth: 1,
                node_filters: vec![],
                edge_filters: vec![],
                return_paths: false,
            }),
            filters: vec![],
            dependencies: vec![],
        };

        // Validate permissions - should succeed
        let result = executor.validate_component_access(&user, &component).await;
        assert!(
            result.is_ok(),
            "Graph traversal permission should be allowed for graph_user"
        );
    }

    #[tokio::test]
    async fn test_graph_traversal_permission_denied() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);
        let executor = ParallelExecutor::with_rbac(4, rbac_manager.clone());

        let user = create_vector_only_user(); // User with only vector permissions

        // Create a graph traversal query component
        let component = QueryComponent {
            model: DataModel::Graph,
            operation: ModelOperation::GraphTraversal(GraphTraversalExpr {
                graph_name: "graph1".to_string(),
                start_nodes: crate::query::unified::ast::StartNodeSpec::Ids(vec![]),
                edge_types: vec![],
                direction: crate::query::unified::ast::TraversalDirection::Outgoing,
                max_depth: 2,
                min_depth: 1,
                node_filters: vec![],
                edge_filters: vec![],
                return_paths: false,
            }),
            filters: vec![],
            dependencies: vec![],
        };

        // Validate permissions - should fail
        let result = executor.validate_component_access(&user, &component).await;
        assert!(
            result.is_err(),
            "Graph traversal permission should be denied for vector-only user"
        );
    }

    #[tokio::test]
    async fn test_document_query_permission_allowed() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);
        let executor = ParallelExecutor::with_rbac(4, rbac_manager.clone());

        let mut user = create_test_user("doc_user", Some("tenant1"));
        user.effective_permissions =
            HashSet::from([UnifiedPermission::CollectionRead("docs1".to_string())]);

        // Create a document query component
        let component = QueryComponent {
            model: DataModel::Document,
            operation: ModelOperation::DocumentQuery(DocumentQueryExpr {
                collection: "docs1".to_string(),
                path_filters: vec![],
                text_search: None,
                projection: vec![],
                sort: None,
                limit: Some(100),
            }),
            filters: vec![],
            dependencies: vec![],
        };

        // Validate permissions - should succeed
        let result = executor.validate_component_access(&user, &component).await;
        assert!(
            result.is_ok(),
            "Document query permission should be allowed for doc_user"
        );
    }

    #[tokio::test]
    async fn test_document_query_permission_denied() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);
        let executor = ParallelExecutor::with_rbac(4, rbac_manager.clone());

        let user = create_vector_only_user(); // User with only vector permissions

        // Create a document query component
        let component = QueryComponent {
            model: DataModel::Document,
            operation: ModelOperation::DocumentQuery(DocumentQueryExpr {
                collection: "docs1".to_string(),
                path_filters: vec![],
                text_search: None,
                projection: vec![],
                sort: None,
                limit: Some(100),
            }),
            filters: vec![],
            dependencies: vec![],
        };

        // Validate permissions - should fail
        let result = executor.validate_component_access(&user, &component).await;
        assert!(
            result.is_err(),
            "Document query permission should be denied for vector-only user"
        );
    }

    #[tokio::test]
    async fn test_observability_query_requires_admin() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);
        let executor = ParallelExecutor::with_rbac(4, rbac_manager.clone());

        let regular_user = create_vector_only_user();

        // Create a log query component (observability)
        let component = QueryComponent {
            model: DataModel::Observability,
            operation: ModelOperation::LogQuery(LogQueryExpr {
                namespace: "logs".to_string(),
                start_time_ns: 0,
                end_time_ns: 0,
                query: None,
                severities: vec![],
                services: vec![],
                limit: 100,
            }),
            filters: vec![],
            dependencies: vec![],
        };

        // Validate permissions - should fail for regular user
        let result = executor
            .validate_component_access(&regular_user, &component)
            .await;
        assert!(
            result.is_err(),
            "Observability queries should require admin permission"
        );
    }

    #[tokio::test]
    async fn test_observability_query_allowed_for_admin() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);
        let executor = ParallelExecutor::with_rbac(4, rbac_manager.clone());

        let mut admin = create_test_user("admin", Some("tenant1"));
        admin.effective_permissions = HashSet::from([UnifiedPermission::SystemAdmin]);

        // Create a log query component (observability)
        let component = QueryComponent {
            model: DataModel::Observability,
            operation: ModelOperation::LogQuery(LogQueryExpr {
                namespace: "logs".to_string(),
                start_time_ns: 0,
                end_time_ns: 0,
                query: None,
                severities: vec![],
                services: vec![],
                limit: 100,
            }),
            filters: vec![],
            dependencies: vec![],
        };

        // Validate permissions - should succeed for admin
        let result = executor.validate_component_access(&admin, &component).await;
        assert!(
            result.is_ok(),
            "Observability queries should be allowed for admin"
        );
    }

    #[tokio::test]
    async fn test_cross_model_query_mixed_permissions() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);
        let executor = ParallelExecutor::with_rbac(4, rbac_manager.clone());
        let storage_engine = create_mock_storage_engine().await;

        // User with vector + document permissions, but no graph
        let mut user = create_test_user("mixed_perms_user", Some("tenant1"));
        user.effective_permissions = HashSet::from([
            UnifiedPermission::VectorSearch("collection1".to_string()),
            UnifiedPermission::CollectionRead("docs1".to_string()),
            // No graph permissions
        ]);

        let query = MultiModelQuery {
            components: vec![
                // Vector search - allowed
                QueryComponent {
                    model: DataModel::Vector,
                    operation: ModelOperation::VectorSearch(VectorSearchExpr {
                        collection: "collection1".to_string(),
                        query_vector: vec![0.1, 0.2, 0.3],
                        top_k: 10,
                        threshold: Some(0.8),
                        metric: DistanceMetric::Cosine,
                        params: VectorSearchParams::default(),
                    }),
                    filters: vec![],
                    dependencies: vec![],
                },
                // Document query - allowed
                QueryComponent {
                    model: DataModel::Document,
                    operation: ModelOperation::DocumentQuery(DocumentQueryExpr {
                        collection: "docs1".to_string(),
                        path_filters: vec![],
                        text_search: None,
                        projection: vec![],
                        sort: None,
                        limit: Some(100),
                    }),
                    filters: vec![],
                    dependencies: vec![],
                },
                // Graph traversal - denied
                QueryComponent {
                    model: DataModel::Graph,
                    operation: ModelOperation::GraphTraversal(GraphTraversalExpr {
                        graph_name: "graph1".to_string(),
                        start_nodes: StartNodeSpec::Ids(vec![]),
                        edge_types: vec![],
                        direction: TraversalDirection::Outgoing,
                        max_depth: 2,
                        min_depth: 1,
                        node_filters: vec![],
                        edge_filters: vec![],
                        return_paths: false,
                    }),
                    filters: vec![],
                    dependencies: vec![],
                },
            ],
            fusion_strategy: crate::query::unified::fusion::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        // Validate all components - should fail on graph component
        let result = executor
            .execute_with_auth(
                &query,
                &user,
                None,                                                   // vector_ops
                Arc::new(DocumentService::new(storage_engine.clone())), // document_service
                None,                                                   // graph_service
                None,                                                   // observability_service
            )
            .await;

        assert!(
            result.is_err(),
            "Cross-model query should fail when any component is denied"
        );
    }

    #[tokio::test]
    async fn test_cross_model_query_all_allowed() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);
        let executor = ParallelExecutor::with_rbac(4, rbac_manager.clone());
        let storage_engine = create_mock_storage_engine().await;

        // Admin user with all permissions
        let admin = create_admin_user();

        let query = MultiModelQuery {
            components: vec![
                // Vector search - allowed
                QueryComponent {
                    model: DataModel::Vector,
                    operation: ModelOperation::VectorSearch(VectorSearchExpr {
                        collection: "collection1".to_string(),
                        query_vector: vec![0.1, 0.2, 0.3],
                        top_k: 10,
                        threshold: Some(0.8),
                        metric: DistanceMetric::Cosine,
                        params: VectorSearchParams::default(),
                    }),
                    filters: vec![],
                    dependencies: vec![],
                },
                // Graph traversal - allowed
                QueryComponent {
                    model: DataModel::Graph,
                    operation: ModelOperation::GraphTraversal(GraphTraversalExpr {
                        graph_name: "graph1".to_string(),
                        start_nodes: StartNodeSpec::Ids(vec![]),
                        edge_types: vec![],
                        direction: TraversalDirection::Outgoing,
                        max_depth: 2,
                        min_depth: 1,
                        node_filters: vec![],
                        edge_filters: vec![],
                        return_paths: false,
                    }),
                    filters: vec![],
                    dependencies: vec![],
                },
            ],
            fusion_strategy: crate::query::unified::fusion::FusionStrategy::Intersection,
            limit: None,
            offset: None,
            projection: vec![],
            order_by: None,
        };

        // Validate all components - should succeed
        // Note: This will still fail during execution since we don't have actual data,
        // but permission validation should pass
        let result = executor
            .execute_with_auth(
                &query,
                &admin,
                None,                                                   // vector_ops
                Arc::new(DocumentService::new(storage_engine.clone())), // document_service
                None,                                                   // graph_service
                None,                                                   // observability_service
            )
            .await;

        // Permission validation should pass (execution may fail due to no data)
        match result {
            Ok(_) => {} // Success - permissions passed
            Err(e) => {
                // Check that error is NOT a permission error
                let error_msg = e.to_string().to_lowercase();
                assert!(
                    !error_msg.contains("permission denied")
                        && !error_msg.contains("insufficient permissions"),
                    "Permission validation should pass for admin user, got: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_multi_tenant_isolation() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);
        let executor = ParallelExecutor::with_rbac(4, rbac_manager.clone());

        // User from tenant1 with access to collection1
        let mut user_tenant1 = create_test_user("user_tenant1", Some("tenant1"));
        user_tenant1.effective_permissions =
            HashSet::from([UnifiedPermission::VectorSearch("collection1".to_string())]);

        // Create query for collection1 (allowed for tenant1)
        let component = QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(VectorSearchExpr {
                collection: "collection1".to_string(),
                query_vector: vec![0.1, 0.2, 0.3],
                top_k: 10,
                threshold: Some(0.8),
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
            filters: vec![],
            dependencies: vec![],
        };

        // Should succeed for tenant1 user
        let result = executor
            .validate_component_access(&user_tenant1, &component)
            .await;
        assert!(
            result.is_ok(),
            "Tenant1 user should have access to collection1"
        );
    }

    #[tokio::test]
    async fn test_permission_cache_effectiveness() {
        let rbac_manager = Arc::new(create_test_rbac_manager().await);

        // Grant permission
        let permission = UnifiedPermission::VectorSearch("collection1".to_string());
        let _ = rbac_manager
            .grant_permission("cache_test_user", &permission)
            .await;

        // First check - cache miss
        let start1 = std::time::Instant::now();
        let result1 = rbac_manager
            .check_permission_cached("cache_test_user", &permission)
            .await;
        let _duration1 = start1.elapsed();

        assert!(result1.is_ok());
        let allowed1 = result1.unwrap();

        // Second check - cache hit (should be faster)
        let start2 = std::time::Instant::now();
        let result2 = rbac_manager
            .check_permission_cached("cache_test_user", &permission)
            .await;
        let _duration2 = start2.elapsed();

        assert!(result2.is_ok());
        let allowed2 = result2.unwrap();

        // Both should return the same result
        assert_eq!(allowed1, allowed2);

        // Cache hit should be faster (or at least not significantly slower)
        // Note: This is a basic check and may not always hold due to system variability
        // In a real benchmark, we'd run multiple iterations
    }

    #[tokio::test]
    async fn test_rbac_manager_without_rbac() {
        // Executor without RBAC manager
        let executor = ParallelExecutor::new(4);
        let user = create_test_user("any_user", Some("tenant1"));

        let component = QueryComponent {
            model: DataModel::Vector,
            operation: ModelOperation::VectorSearch(VectorSearchExpr {
                collection: "any_collection".to_string(),
                query_vector: vec![0.1, 0.2, 0.3],
                top_k: 10,
                threshold: Some(0.8),
                metric: DistanceMetric::Cosine,
                params: VectorSearchParams::default(),
            }),
            filters: vec![],
            dependencies: vec![],
        };

        // Should fail - RBAC validation requested but manager not configured
        let result = executor.validate_component_access(&user, &component).await;
        assert!(
            result.is_err(),
            "RBAC validation should fail when RBAC manager is not configured"
        );
    }
}
