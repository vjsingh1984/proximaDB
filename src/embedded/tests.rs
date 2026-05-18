#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::*;
    use std::collections::HashMap;

    // ========================================================================
    // StorageLocationConfig Tests
    // ========================================================================

    #[test]
    fn test_storage_location_url_conversion() {
        let loc = StorageLocationConfig::new("/nvme1/proximadb");
        assert_eq!(loc.to_url(), "file:///nvme1/proximadb");

        let loc = StorageLocationConfig::new("file:///already/url");
        assert_eq!(loc.to_url(), "file:///already/url");

        let loc = StorageLocationConfig::new("relative/path");
        assert!(loc.to_url().starts_with("file://"));
        assert!(loc.to_url().contains("relative/path"));
    }

    #[test]
    fn test_storage_location_builder() {
        let loc = StorageLocationConfig::new("/data")
            .with_weight(2)
            .with_tag("hot");

        assert_eq!(loc.weight, 2);
        assert!(loc.tags.contains(&"hot".to_string()));
    }

    #[test]
    fn test_storage_location_default_values() {
        let loc = StorageLocationConfig::new("/data");
        assert_eq!(loc.path, "/data");
        assert_eq!(loc.weight, 1);
        assert!(loc.tags.is_empty());
    }

    #[test]
    fn test_storage_location_multiple_tags() {
        let loc = StorageLocationConfig::new("/fast-storage")
            .with_tag("hot")
            .with_tag("nvme")
            .with_tag("primary");

        assert_eq!(loc.tags.len(), 3);
        assert!(loc.tags.contains(&"hot".to_string()));
        assert!(loc.tags.contains(&"nvme".to_string()));
        assert!(loc.tags.contains(&"primary".to_string()));
    }

    #[test]
    fn test_storage_location_high_weight() {
        let loc = StorageLocationConfig::new("/high-capacity").with_weight(10);
        assert_eq!(loc.weight, 10);
    }

    // ========================================================================
    // EmbeddedConfig Tests
    // ========================================================================

    #[test]
    fn test_embedded_config_default() {
        let config = EmbeddedConfig::default();
        assert_eq!(config.cache_size_mb, 512);
        assert_eq!(config.default_engine, "sst");
        assert!(config.enable_wal);
        assert_eq!(config.wal_sync_mode, "batch");
        assert_eq!(config.block_prune_mode, "sqrt");
        assert!(config.enable_rl_planner);
        assert!(config.rl_policy_path.is_none());
        assert_eq!(config.access_mode, AccessMode::Exclusive);
        assert!(config.node_id.is_none());
    }

    #[test]
    fn test_embedded_config_storage_locations_default() {
        let config = EmbeddedConfig::default();
        assert_eq!(config.storage_locations.len(), 1);
        assert_eq!(config.storage_locations[0].path, "./data");
        assert_eq!(config.storage_locations[0].weight, 1);
    }

    #[test]
    fn test_embedded_execute_sql_lowers_agentic_ddl_to_catalog() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = EmbeddedConfig::for_low_memory(temp_dir.path().to_string_lossy().as_ref());
        let db = EmbeddedProximaDB::new(config).expect("create embedded db");

        let result = db
            .execute_sql(
                "CREATE TABLE IF NOT EXISTS \"agent_store\" (
                    \"record_id\" TEXT NOT NULL,
                    \"tenant_id\" TEXT NOT NULL,
                    \"payload\" JSONB NOT NULL DEFAULT '{}'::jsonb,
                    \"embedding\" VECTOR(64),
                    PRIMARY KEY (\"record_id\")
                ) WITH (
                    storage_engine = 'SST',
                    layout = 'hybrid',
                    xcatalog_namespace = 'agentic.embedded',
                    schema_kind = 'agentic_mixed'
                );",
                None,
                None,
            )
            .expect("execute create table ddl");

        assert_eq!(result.row_count, 1);
        assert_eq!(result.rows[0]["success"], serde_json::json!(true));

        db.execute_sql(
            "CREATE INDEX idx_agent_payload ON agent_store USING GIN (payload);",
            None,
            None,
        )
        .expect("execute gin index ddl");
        db.execute_sql(
            "CREATE INDEX idx_agent_embedding ON agent_store USING HNSW (embedding);",
            None,
            None,
        )
        .expect("execute hnsw index ddl");

        let (catalog, table_id) = db
            .runtime
            .block_on(db.catalog_manager.resolve_table("agent_store"))
            .expect("resolve agent_store");
        let schema = db
            .runtime
            .block_on(catalog.get_table(&table_id))
            .expect("catalog schema");

        assert_eq!(schema.primary_key, vec!["record_id".to_string()]);
        assert_eq!(
            schema
                .properties
                .get("xcatalog_namespace")
                .map(String::as_str),
            Some("agentic.embedded")
        );
        assert_eq!(
            schema.properties.get("schema_kind").map(String::as_str),
            Some("agentic_mixed")
        );
        assert_eq!(
            schema
                .columns
                .iter()
                .find(|column| column.name == "embedding")
                .and_then(|column| column.properties.get("dimension"))
                .map(String::as_str),
            Some("64")
        );

        let indexes = db
            .runtime
            .block_on(catalog.list_indexes(&table_id))
            .expect("catalog indexes");
        assert!(
            indexes
                .iter()
                .any(|index| index.index_type == crate::catalog::CatalogIndexType::Gin)
        );
        assert!(
            indexes
                .iter()
                .any(|index| index.index_type == crate::catalog::CatalogIndexType::Hnsw)
        );

        let tables = db
            .execute_sql(
                "SELECT * FROM xcatalog.tables WHERE table_name = 'agent_store';",
                None,
                None,
            )
            .expect("query xcatalog tables");
        assert_eq!(tables.row_count, 1);
        assert_eq!(
            tables.rows[0]["table_name"],
            serde_json::json!("agent_store")
        );
        assert_eq!(
            tables.rows[0]["schema_kind"],
            serde_json::json!("agentic_mixed")
        );
        assert_eq!(tables.rows[0]["storage_engine"], serde_json::json!("SST"));
        assert_eq!(
            tables.rows[0]["xcatalog_namespace"],
            serde_json::json!("agentic.embedded")
        );

        let columns = db
            .execute_sql(
                "SELECT * FROM xcatalog.columns WHERE table_name = 'agent_store';",
                None,
                None,
            )
            .expect("query xcatalog columns");
        assert!(columns.rows.iter().any(|row| {
            row["column_name"] == serde_json::json!("payload")
                && row["data_type"] == serde_json::json!("jsonb")
        }));
        assert!(columns.rows.iter().any(|row| {
            row["column_name"] == serde_json::json!("embedding")
                && row["data_type"] == serde_json::json!("vector")
                && row["vector_dimension"] == serde_json::json!("64")
        }));

        let index_rows = db
            .execute_sql(
                "SELECT * FROM xcatalog.indexes WHERE table_name = 'agent_store';",
                None,
                None,
            )
            .expect("query xcatalog indexes");
        assert!(index_rows.rows.iter().any(|row| {
            row["index_name"] == serde_json::json!("idx_agent_payload")
                && row["index_type"] == serde_json::json!("gin")
        }));
        assert!(index_rows.rows.iter().any(|row| {
            row["index_name"] == serde_json::json!("idx_agent_embedding")
                && row["index_type"] == serde_json::json!("hnsw")
        }));

        let insert = db
            .execute_sql(
                "INSERT INTO agent_store (
                    record_id,
                    tenant_id,
                    payload,
                    embedding
                ) VALUES (
                    'record-1',
                    'tenant-1',
                    '{\"kind\":\"memory\",\"score\":7}'::jsonb,
                    '[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8,
                      0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6,
                      1.7, 1.8, 1.9, 2.0, 2.1, 2.2, 2.3, 2.4,
                      2.5, 2.6, 2.7, 2.8, 2.9, 3.0, 3.1, 3.2,
                      3.3, 3.4, 3.5, 3.6, 3.7, 3.8, 3.9, 4.0,
                      4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 4.7, 4.8,
                      4.9, 5.0, 5.1, 5.2, 5.3, 5.4, 5.5, 5.6,
                      5.7, 5.8, 5.9, 6.0, 6.1, 6.2, 6.3, 6.4]'
                );",
                None,
                None,
            )
            .expect("insert canonical mixed record through DML");
        assert_eq!(insert.row_count, 1);
        assert_eq!(insert.rows[0]["success"], serde_json::json!(true));
        assert_eq!(insert.rows[0]["rows_affected"], serde_json::json!(1));
        assert_eq!(
            insert.rows[0]["inserted_ids"],
            serde_json::json!(["record-1"])
        );

        let update = db
            .execute_sql(
                "UPDATE agent_store
                 SET payload = '{\"kind\":\"updated\",\"score\":9}'::jsonb,
                     embedding = '[9.9, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8,
                       0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6,
                       1.7, 1.8, 1.9, 2.0, 2.1, 2.2, 2.3, 2.4,
                       2.5, 2.6, 2.7, 2.8, 2.9, 3.0, 3.1, 3.2,
                       3.3, 3.4, 3.5, 3.6, 3.7, 3.8, 3.9, 4.0,
                       4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 4.7, 4.8,
                       4.9, 5.0, 5.1, 5.2, 5.3, 5.4, 5.5, 5.6,
                       5.7, 5.8, 5.9, 6.0, 6.1, 6.2, 6.3, 6.4]'
                 WHERE record_id = 'record-1';",
                None,
                None,
            )
            .expect("update canonical mixed record through DML");
        assert_eq!(update.row_count, 1);
        assert_eq!(update.rows[0]["success"], serde_json::json!(true));
        assert_eq!(update.rows[0]["rows_affected"], serde_json::json!(1));

        let updated_record = db
            .get_vector("agent_store", "record-1")
            .expect("get updated record")
            .expect("updated record should exist");
        let updated_vec = updated_record
            .embeddings
            .first()
            .map(|e| e.values.as_slice())
            .unwrap_or(&[]);
        assert_eq!(updated_vec.len(), 64);
        assert!((updated_vec[0] - 9.9).abs() < 0.001);

        let default_insert = db
            .execute_sql(
                "INSERT INTO agent_store (
                    record_id,
                    tenant_id,
                    embedding
                ) VALUES (
                    'record-2',
                    'tenant-1',
                    '[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8,
                      0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6,
                      1.7, 1.8, 1.9, 2.0, 2.1, 2.2, 2.3, 2.4,
                      2.5, 2.6, 2.7, 2.8, 2.9, 3.0, 3.1, 3.2,
                      3.3, 3.4, 3.5, 3.6, 3.7, 3.8, 3.9, 4.0,
                      4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 4.7, 4.8,
                      4.9, 5.0, 5.1, 5.2, 5.3, 5.4, 5.5, 5.6,
                      5.7, 5.8, 5.9, 6.0, 6.1, 6.2, 6.3, 6.4]'
                );",
                None,
                None,
            )
            .expect("insert mixed record with catalog JSONB default");
        assert_eq!(default_insert.row_count, 1);
        assert_eq!(default_insert.rows[0]["success"], serde_json::json!(true));
        assert_eq!(
            default_insert.rows[0]["inserted_ids"],
            serde_json::json!(["record-2"])
        );

        let delete = db
            .execute_sql(
                "DELETE FROM agent_store WHERE record_id = 'record-2';",
                None,
                None,
            )
            .expect("delete canonical mixed record through DML");
        assert_eq!(delete.row_count, 1);
        assert_eq!(delete.rows[0]["success"], serde_json::json!(true));
        assert_eq!(delete.rows[0]["rows_affected"], serde_json::json!(1));
    }

    #[test]
    fn test_embedded_config_for_benchmarks() {
        let config = EmbeddedConfig::for_benchmarks("/tmp/bench");
        assert_eq!(config.cache_size_mb, 1024);
        assert_eq!(config.default_engine, "sst");
        assert!(config.enable_wal);
        assert_eq!(config.wal_sync_mode, "batch");
        assert!(config.enable_rl_planner);
        assert_eq!(config.storage_locations[0].path, "/tmp/bench");
        assert!(
            config.storage_locations[0]
                .tags
                .contains(&"benchmark".to_string())
        );
    }

    #[test]
    fn test_embedded_config_for_low_memory() {
        let config = EmbeddedConfig::for_low_memory("/tmp/lowmem");
        assert_eq!(config.cache_size_mb, 128);
        assert!(!config.enable_rl_planner);
        assert_eq!(config.storage_locations[0].path, "/tmp/lowmem");
    }

    #[test]
    fn test_embedded_config_with_access_mode() {
        let config = EmbeddedConfig::default().with_access_mode(AccessMode::SharedRead);
        assert_eq!(config.access_mode, AccessMode::SharedRead);
    }

    #[test]
    fn test_embedded_config_with_node_id() {
        let config = EmbeddedConfig::default().with_node_id("node-123");
        assert_eq!(config.node_id, Some("node-123".to_string()));
    }

    #[test]
    fn test_embedded_config_chained_builders() {
        let config = EmbeddedConfig::default()
            .with_access_mode(AccessMode::LeaderFollower)
            .with_node_id("leader-node");
        assert_eq!(config.access_mode, AccessMode::LeaderFollower);
        assert_eq!(config.node_id, Some("leader-node".to_string()));
    }

    #[test]
    fn test_collection_engine_name_uses_storage_engine_strings() {
        assert_eq!(
            collection_engine_name(Some(crate::proto::proximadb_v1::StorageEngine::Tst as i32)),
            "tst"
        );
        assert_eq!(
            collection_engine_name(Some(crate::proto::proximadb_v1::StorageEngine::Sst as i32)),
            "sst"
        );
    }

    // ========================================================================
    // GraphNode Tests
    // ========================================================================

    #[test]
    fn test_graph_node_creation() {
        let node = GraphNode::new("node_1");
        assert_eq!(node.id, "node_1");
        assert!(node.labels.is_empty());
        assert!(node.properties.is_empty());
    }

    #[test]
    fn test_graph_node_with_label() {
        let node = GraphNode::new("user_1").with_label("Person");
        assert_eq!(node.labels.len(), 1);
        assert!(node.labels.contains(&"Person".to_string()));
    }

    #[test]
    fn test_graph_node_multiple_labels() {
        let node = GraphNode::new("entity_1")
            .with_label("Person")
            .with_label("Employee")
            .with_label("Manager");
        assert_eq!(node.labels.len(), 3);
        assert!(node.labels.contains(&"Person".to_string()));
        assert!(node.labels.contains(&"Employee".to_string()));
        assert!(node.labels.contains(&"Manager".to_string()));
    }

    #[test]
    fn test_graph_node_with_property() {
        let node = GraphNode::new("user_1").with_property("name", "Alice");
        assert_eq!(node.properties.get("name"), Some(&"Alice".to_string()));
    }

    #[test]
    fn test_graph_node_multiple_properties() {
        let node = GraphNode::new("user_1")
            .with_property("name", "Alice")
            .with_property("email", "alice@example.com")
            .with_property("age", "30");
        assert_eq!(node.properties.len(), 3);
        assert_eq!(node.properties.get("name"), Some(&"Alice".to_string()));
        assert_eq!(
            node.properties.get("email"),
            Some(&"alice@example.com".to_string())
        );
        assert_eq!(node.properties.get("age"), Some(&"30".to_string()));
    }

    #[test]
    fn test_graph_node_builder_chain() {
        let node = GraphNode::new("func_main")
            .with_label("function")
            .with_label("public")
            .with_property("name", "main")
            .with_property("file", "main.rs")
            .with_property("line", "10");

        assert_eq!(node.id, "func_main");
        assert_eq!(node.labels.len(), 2);
        assert_eq!(node.properties.len(), 3);
    }

    #[test]
    fn test_graph_node_to_proto() {
        let node = GraphNode::new("test_node")
            .with_label("TestLabel")
            .with_property("key", "value");

        let proto = node.to_proto();
        assert_eq!(proto.id, "test_node");
        assert!(proto.labels.contains(&"TestLabel".to_string()));
        assert!(proto.properties.contains_key("key"));
    }

    #[test]
    fn test_graph_node_clone() {
        let node = GraphNode::new("original")
            .with_label("Label")
            .with_property("prop", "value");

        let cloned = node.clone();
        assert_eq!(cloned.id, node.id);
        assert_eq!(cloned.labels, node.labels);
        assert_eq!(cloned.properties, node.properties);
    }

    // ========================================================================
    // GraphEdge Tests
    // ========================================================================

    #[test]
    fn test_graph_edge_creation() {
        let edge = GraphEdge::new("node_a", "node_b", "KNOWS");
        assert_eq!(edge.from_node_id, "node_a");
        assert_eq!(edge.to_node_id, "node_b");
        assert_eq!(edge.edge_type, "KNOWS");
        assert!(edge.id.is_none());
        assert!(edge.weight.is_none());
        assert!(edge.properties.is_empty());
    }

    #[test]
    fn test_graph_edge_with_id() {
        let edge = GraphEdge::new("a", "b", "REL").with_id("edge_123");
        assert_eq!(edge.id, Some("edge_123".to_string()));
    }

    #[test]
    fn test_graph_edge_with_weight() {
        let edge = GraphEdge::new("a", "b", "WEIGHTED").with_weight(0.75);
        assert_eq!(edge.weight, Some(0.75));
    }

    #[test]
    fn test_graph_edge_with_property() {
        let edge = GraphEdge::new("a", "b", "RELATIONSHIP").with_property("since", "2024");
        assert_eq!(edge.properties.get("since"), Some(&"2024".to_string()));
    }

    #[test]
    fn test_graph_edge_builder_chain() {
        let edge = GraphEdge::new("user_1", "user_2", "FOLLOWS")
            .with_id("follow_edge")
            .with_weight(1.0)
            .with_property("timestamp", "2024-01-01")
            .with_property("source", "web");

        assert_eq!(edge.from_node_id, "user_1");
        assert_eq!(edge.to_node_id, "user_2");
        assert_eq!(edge.edge_type, "FOLLOWS");
        assert_eq!(edge.id, Some("follow_edge".to_string()));
        assert_eq!(edge.weight, Some(1.0));
        assert_eq!(edge.properties.len(), 2);
    }

    #[test]
    fn test_graph_edge_generated_id() {
        let edge = GraphEdge::new("a", "b", "TYPE");
        // Test the generated_id method indirectly via to_proto
        let proto = edge.to_proto();
        assert_eq!(proto.id, "a->b:TYPE");
    }

    #[test]
    fn test_graph_edge_to_proto() {
        let edge = GraphEdge::new("from", "to", "REL")
            .with_weight(0.5)
            .with_property("key", "value");

        let proto = edge.to_proto();
        assert_eq!(proto.from_node_id, "from");
        assert_eq!(proto.to_node_id, "to");
        assert_eq!(proto.edge_type, "REL");
        assert_eq!(proto.weight, Some(0.5));
    }

    #[test]
    fn test_graph_edge_clone() {
        let edge = GraphEdge::new("a", "b", "TYPE")
            .with_weight(0.8)
            .with_property("key", "value");

        let cloned = edge.clone();
        assert_eq!(cloned.from_node_id, edge.from_node_id);
        assert_eq!(cloned.to_node_id, edge.to_node_id);
        assert_eq!(cloned.edge_type, edge.edge_type);
        assert_eq!(cloned.weight, edge.weight);
    }

    // ========================================================================
    // SearchResult Tests
    // ========================================================================

    #[test]
    fn test_search_result_creation() {
        let mut metadata = HashMap::new();
        metadata.insert("category".to_string(), "technology".to_string());

        let result = SearchResult {
            id: "vec_123".to_string(),
            score: 0.95,
            metadata,
        };

        assert_eq!(result.id, "vec_123");
        assert!((result.score - 0.95).abs() < f32::EPSILON);
        assert_eq!(
            result.metadata.get("category"),
            Some(&"technology".to_string())
        );
    }

    #[test]
    fn test_search_result_empty_metadata() {
        let result = SearchResult {
            id: "id".to_string(),
            score: 0.5,
            metadata: HashMap::new(),
        };
        assert!(result.metadata.is_empty());
    }

    #[test]
    fn test_search_result_clone() {
        let mut metadata = HashMap::new();
        metadata.insert("key".to_string(), "value".to_string());

        let result = SearchResult {
            id: "original".to_string(),
            score: 0.75,
            metadata,
        };

        let cloned = result.clone();
        assert_eq!(cloned.id, result.id);
        assert_eq!(cloned.score, result.score);
        assert_eq!(cloned.metadata, result.metadata);
    }

    // ========================================================================
    // CollectionInfo Tests
    // ========================================================================

    #[test]
    fn test_collection_info_creation() {
        let info = CollectionInfo {
            name: "my_collection".to_string(),
            dimension: 768,
            vector_count: 10000,
            engine: "sst".to_string(),
            disk_usage_bytes: 1024 * 1024 * 100,
        };

        assert_eq!(info.name, "my_collection");
        assert_eq!(info.dimension, 768);
        assert_eq!(info.vector_count, 10000);
        assert_eq!(info.engine, "sst");
        assert_eq!(info.disk_usage_bytes, 104857600);
    }

    #[test]
    fn test_collection_info_clone() {
        let info = CollectionInfo {
            name: "test".to_string(),
            dimension: 256,
            vector_count: 500,
            engine: "helix".to_string(),
            disk_usage_bytes: 1000,
        };

        let cloned = info.clone();
        assert_eq!(cloned.name, info.name);
        assert_eq!(cloned.dimension, info.dimension);
    }

    // ========================================================================
    // StorageStats Tests
    // ========================================================================

    #[test]
    fn test_storage_stats_creation() {
        let stats = StorageStats {
            total_vectors: 50000,
            total_collections: 5,
            disk_usage_bytes: 1024 * 1024 * 500,
            cache_hit_rate: 0.85,
        };

        assert_eq!(stats.total_vectors, 50000);
        assert_eq!(stats.total_collections, 5);
        assert!((stats.cache_hit_rate - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_storage_stats_empty() {
        let stats = StorageStats {
            total_vectors: 0,
            total_collections: 0,
            disk_usage_bytes: 0,
            cache_hit_rate: 0.0,
        };

        assert_eq!(stats.total_vectors, 0);
        assert_eq!(stats.total_collections, 0);
    }

    #[test]
    fn test_storage_stats_clone() {
        let stats = StorageStats {
            total_vectors: 1000,
            total_collections: 2,
            disk_usage_bytes: 5000,
            cache_hit_rate: 0.9,
        };

        let cloned = stats.clone();
        assert_eq!(cloned.total_vectors, stats.total_vectors);
        assert_eq!(cloned.cache_hit_rate, stats.cache_hit_rate);
    }

    // ========================================================================
    // GraphStats Tests
    // ========================================================================

    #[test]
    fn test_graph_stats_creation() {
        let stats = GraphStats {
            total_nodes: 1000,
            total_edges: 5000,
        };

        assert_eq!(stats.total_nodes, 1000);
        assert_eq!(stats.total_edges, 5000);
    }

    #[test]
    fn test_graph_stats_empty() {
        let stats = GraphStats {
            total_nodes: 0,
            total_edges: 0,
        };

        assert_eq!(stats.total_nodes, 0);
        assert_eq!(stats.total_edges, 0);
    }

    #[test]
    fn test_graph_stats_clone() {
        let stats = GraphStats {
            total_nodes: 100,
            total_edges: 500,
        };

        let cloned = stats.clone();
        assert_eq!(cloned.total_nodes, stats.total_nodes);
        assert_eq!(cloned.total_edges, stats.total_edges);
    }

    // ========================================================================
    // AccessMode Tests (from coordination module, but used in EmbeddedConfig)
    // ========================================================================

    #[test]
    fn test_access_mode_equality() {
        assert_eq!(AccessMode::Exclusive, AccessMode::Exclusive);
        assert_eq!(AccessMode::SharedRead, AccessMode::SharedRead);
        assert_eq!(AccessMode::LeaderFollower, AccessMode::LeaderFollower);
        assert_ne!(AccessMode::Exclusive, AccessMode::SharedRead);
    }

    #[test]
    fn test_access_mode_can_write() {
        assert!(AccessMode::Exclusive.can_write());
        assert!(!AccessMode::SharedRead.can_write());
        assert!(AccessMode::LeaderFollower.can_write());
    }

    // ========================================================================
    // CacheStatsSnapshot Tests (internal type)
    // ========================================================================

    #[test]
    fn test_cache_stats_snapshot_creation() {
        let snapshot = CacheStatsSnapshot {
            entries: 1000,
            memory_bytes: 1024 * 1024,
        };

        assert_eq!(snapshot.entries, 1000);
        assert_eq!(snapshot.memory_bytes, 1048576);
    }

    #[test]
    fn test_cache_stats_snapshot_clone() {
        let snapshot = CacheStatsSnapshot {
            entries: 500,
            memory_bytes: 2048,
        };

        let cloned = snapshot.clone();
        assert_eq!(cloned.entries, snapshot.entries);
        assert_eq!(cloned.memory_bytes, snapshot.memory_bytes);
    }

    // ========================================================================
    // Embedded Config Defaults Tests
    // ========================================================================

    #[test]
    fn test_embedded_config_defaults() {
        let config = EmbeddedConfig::default();

        // Core defaults
        assert_eq!(config.metadata_path, "./data/metadata");
        assert_eq!(config.cache_size_mb, 512);
        assert_eq!(config.default_engine, "sst");
        assert!(config.enable_wal);
        assert_eq!(config.wal_sync_mode, "batch");

        // Block pruning defaults
        assert_eq!(config.block_prune_mode, "sqrt");
        assert!((config.block_prune_ratio - 0.2).abs() < f32::EPSILON);
        assert_eq!(config.block_prune_min_keep, 1);
        assert_eq!(config.block_prune_max_keep, 0); // No cap

        // RL planner defaults
        assert!(config.enable_rl_planner);
        assert!(config.rl_policy_path.is_none());

        // Multi-process coordination defaults
        assert_eq!(config.access_mode, AccessMode::Exclusive);
        assert!(config.node_id.is_none());

        // Storage locations default
        assert_eq!(config.storage_locations.len(), 1);
        assert_eq!(config.storage_locations[0].path, "./data");
        assert_eq!(config.storage_locations[0].weight, 1);
        assert!(config.storage_locations[0].tags.is_empty());
    }

    // ========================================================================
    // Embedded Config Builder Tests
    // ========================================================================

    #[test]
    fn test_embedded_config_builder() {
        let config = EmbeddedConfig {
            storage_locations: vec![
                StorageLocationConfig::new("/nvme/data")
                    .with_weight(3)
                    .with_tag("hot"),
                StorageLocationConfig::new("/hdd/data")
                    .with_weight(1)
                    .with_tag("cold"),
            ],
            metadata_path: "/nvme/metadata".to_string(),
            cache_size_mb: 2048,
            default_engine: "viper".to_string(),
            enable_wal: false,
            wal_sync_mode: "immediate".to_string(),
            block_prune_mode: "ratio".to_string(),
            block_prune_ratio: 0.5,
            block_prune_min_keep: 2,
            block_prune_max_keep: 100,
            enable_rl_planner: false,
            rl_policy_path: Some("/custom/rl_policy.json".to_string()),
            access_mode: AccessMode::SharedRead,
            node_id: Some("node-42".to_string()),
        };

        assert_eq!(config.storage_locations.len(), 2);
        assert_eq!(config.storage_locations[0].weight, 3);
        assert!(
            config.storage_locations[0]
                .tags
                .contains(&"hot".to_string())
        );
        assert_eq!(config.storage_locations[1].weight, 1);
        assert!(
            config.storage_locations[1]
                .tags
                .contains(&"cold".to_string())
        );
        assert_eq!(config.metadata_path, "/nvme/metadata");
        assert_eq!(config.cache_size_mb, 2048);
        assert_eq!(config.default_engine, "viper");
        assert!(!config.enable_wal);
        assert_eq!(config.wal_sync_mode, "immediate");
        assert_eq!(config.block_prune_mode, "ratio");
        assert!((config.block_prune_ratio - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.block_prune_min_keep, 2);
        assert_eq!(config.block_prune_max_keep, 100);
        assert!(!config.enable_rl_planner);
        assert_eq!(
            config.rl_policy_path,
            Some("/custom/rl_policy.json".to_string())
        );
        assert_eq!(config.access_mode, AccessMode::SharedRead);
        assert_eq!(config.node_id, Some("node-42".to_string()));

        // Test chained builder methods
        let config2 = EmbeddedConfig::default()
            .with_access_mode(AccessMode::LeaderFollower)
            .with_node_id("leader-1");
        assert_eq!(config2.access_mode, AccessMode::LeaderFollower);
        assert_eq!(config2.node_id, Some("leader-1".to_string()));
    }

    // ========================================================================
    // CollectionInfo Config Creation Tests
    // ========================================================================

    #[test]
    fn test_collection_config_creation() {
        let info = CollectionInfo {
            name: "embeddings".to_string(),
            dimension: 768,
            vector_count: 50000,
            engine: "sst".to_string(),
            disk_usage_bytes: 1024 * 1024 * 100, // 100 MB
        };

        assert_eq!(info.name, "embeddings");
        assert_eq!(info.dimension, 768);
        assert_eq!(info.vector_count, 50000);
        assert_eq!(info.engine, "sst");
        assert_eq!(info.disk_usage_bytes, 104857600);

        // Verify Clone works
        let cloned = info.clone();
        assert_eq!(cloned.name, info.name);
        assert_eq!(cloned.dimension, info.dimension);
        assert_eq!(cloned.vector_count, info.vector_count);
    }

    // ========================================================================
    // SearchResult Defaults Tests
    // ========================================================================

    #[test]
    fn test_search_params_defaults() {
        // Verify SearchResult construction with default-like values
        let result = SearchResult {
            id: "vec_001".to_string(),
            score: 0.0,
            metadata: HashMap::new(),
        };
        assert_eq!(result.id, "vec_001");
        assert!((result.score - 0.0).abs() < f32::EPSILON);
        assert!(result.metadata.is_empty());

        // Verify with populated metadata
        let mut meta = HashMap::new();
        meta.insert("category".to_string(), "test".to_string());
        meta.insert("source".to_string(), "unit_test".to_string());

        let result_with_meta = SearchResult {
            id: "vec_002".to_string(),
            score: 0.95,
            metadata: meta,
        };
        assert_eq!(result_with_meta.metadata.len(), 2);
        assert_eq!(
            result_with_meta.metadata.get("category"),
            Some(&"test".to_string())
        );
    }

    // ========================================================================
    // Embedded Metrics Creation Tests
    // ========================================================================

    #[test]
    fn test_embedded_metrics_creation() {
        let collector = EmbeddedMetricsCollector::new();

        // Take a snapshot with default window
        let metrics = collector.snapshot(RollingWindow::AllTime);

        // All counters should be zero
        assert_eq!(metrics.total_searches, 0);
        assert_eq!(metrics.total_inserts, 0);
        assert_eq!(metrics.total_deletes, 0);
        assert_eq!(metrics.total_flushes, 0);
        assert_eq!(metrics.total_gets, 0);
        assert_eq!(metrics.total_upserts, 0);
        assert_eq!(metrics.total_vectors_inserted, 0);
        assert_eq!(metrics.total_vectors_deleted, 0);
        assert_eq!(metrics.total_bytes_written, 0);
        assert_eq!(metrics.total_bytes_read, 0);
        assert_eq!(metrics.total_errors, 0);

        // Cache stats should be zero
        assert_eq!(metrics.cache_hits, 0);
        assert_eq!(metrics.cache_misses, 0);
        assert_eq!(metrics.cache_entries, 0);
        assert_eq!(metrics.cache_memory_bytes, 0);
        assert_eq!(metrics.cache_evictions, 0);

        // WAL stats should be zero
        assert_eq!(metrics.wal_pending_bytes, 0);
        assert_eq!(metrics.wal_segments_count, 0);

        // Latency stats should have zero count
        assert_eq!(metrics.search_latency.count, 0);
        assert_eq!(metrics.insert_latency.count, 0);
        assert_eq!(metrics.delete_latency.count, 0);

        // Record some operations and verify counters update
        collector.record_search_us(500);
        collector.record_insert_us(200, 10);
        collector.record_error();

        let updated = collector.snapshot(RollingWindow::AllTime);
        assert_eq!(updated.total_searches, 1);
        assert_eq!(updated.total_inserts, 1);
        assert_eq!(updated.total_vectors_inserted, 10);
        assert_eq!(updated.total_errors, 1);
    }

    // ========================================================================
    // Embedded Version Info Tests
    // ========================================================================

    #[test]
    fn test_embedded_version_info() {
        // The crate version is available via env! macro at compile time
        let version = env!("CARGO_PKG_VERSION");

        // Version should be non-empty and follow semver format
        assert!(!version.is_empty());

        let parts: Vec<&str> = version.split('.').collect();
        assert!(
            parts.len() >= 2,
            "Version '{}' should have at least major.minor components",
            version
        );

        // Major version should be parseable as a number
        let major: u32 = parts[0]
            .parse()
            .unwrap_or_else(|_| panic!("Major version '{}' should be numeric", parts[0]));
        // Minor version should be parseable as a number
        let _minor: u32 = parts[1]
            .parse()
            .unwrap_or_else(|_| panic!("Minor version '{}' should be numeric", parts[1]));

        // ProximaDB should be at least v0.x
        assert!(
            major < 100,
            "Major version {} seems unreasonably large",
            major
        );

        // Verify the crate name
        let crate_name = env!("CARGO_PKG_NAME");
        assert_eq!(crate_name, "proximadb");
    }
}
