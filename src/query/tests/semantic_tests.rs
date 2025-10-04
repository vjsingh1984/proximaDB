//! Tests for the semantic analyzer.

use crate::query::semantic_analysis::analyzer::Analyzer;
use crate::query::semantic_analysis::scope::{Symbol, DataType};
use crate::services::collection::manager::CollectionService;
use crate::core::config::StorageConfig;
use crate::query::sql_frontend::parser::SqlFrontendParser;
use std::sync::Arc;
use anyhow::Result;

// Mock CollectionService for testing
struct MockCollectionService {
    collections: std::collections::HashMap<String, crate::proto::proximadb_v1::Collection>,
}

impl MockCollectionService {
    fn new() -> Self {
        let mut collections = std::collections::HashMap::new();
        // Add a mock 'products' collection
        collections.insert(
            "products".to_string(),
            crate::proto::proximadb_v1::Collection {
                id: "uuid-products".to_string(),
                config: Some(crate::proto::proximadb_v1::CollectionConfig {
                    name: "products".to_string(),
                    dimension: 1536,
                    distance_metric: 0, // Cosine metric
                    storage_engine: 0, // Default storage engine
                    tags: vec![],
                    description: None,
                    filterable_columns: vec![
                        crate::proto::proximadb_v1::FilterableColumnSpec {
                            name: "id".to_string(),
                            data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                            ..Default::default()
                        },
                        crate::proto::proximadb_v1::FilterableColumnSpec {
                            name: "name".to_string(),
                            data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                            ..Default::default()
                        },
                        crate::proto::proximadb_v1::FilterableColumnSpec {
                            name: "price".to_string(),
                            data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableFloat as i32,
                            ..Default::default()
                        },
                        crate::proto::proximadb_v1::FilterableColumnSpec {
                            name: "category".to_string(),
                            data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                            ..Default::default()
                        },
                        crate::proto::proximadb_v1::FilterableColumnSpec {
                            name: "embedding".to_string(),
                            data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableFloat as i32, // Vector represented as float array
                            ..Default::default()
                        },
                    ],
                    index_configs: vec![],
                    quantization: None,
                    storage_config: None,
                    primary_index: "default".to_string(),
                    auto_index_selection: false,
                    owner: None,
                    embedding_models: vec![],
                    // schema field not in proto - replaced with filterable_columns
                    /*schema: Some(crate::proto::proximadb_v1::CollectionSchema {
                        fields: vec![
                            crate::proto::proximadb_v1::SchemaField {
                                name: "id".to_string(),
                                data_type: "string".to_string(),
                                dimension: None,
                            },
                            crate::proto::proximadb_v1::SchemaField {
                                name: "name".to_string(),
                                data_type: "string".to_string(),
                                dimension: None,
                            },
                            crate::proto::proximadb_v1::SchemaField {
                                name: "price".to_string(),
                                data_type: "float64".to_string(),
                                dimension: None,
                            },
                            crate::proto::proximadb_v1::SchemaField {
                                name: "category".to_string(),
                                data_type: "string".to_string(),
                                dimension: None,
                            },
                            crate::proto::proximadb_v1::SchemaField {
                                name: "embedding".to_string(),
                                data_type: "vector".to_string(),
                                dimension: Some(1536),
                            },
                        ],
                    }),*/
                    ..Default::default()
                }),
                stats: Some(crate::proto::proximadb_v1::CollectionStats {
                    vector_count: 0,
                    index_size_bytes: 0,
                    data_size_bytes: 0,
                }),
                created_at: 0,
                updated_at: 0,
                storage_assignment: None,
            },
        );
        // Add a mock 'users' collection
        collections.insert(
            "users".to_string(),
            crate::proto::proximadb_v1::Collection {
                id: "uuid-users".to_string(),
                config: Some(crate::proto::proximadb_v1::CollectionConfig {
                    name: "users".to_string(),
                    dimension: 0,
                    distance_metric: 0, // Changed from string to enum
                    storage_engine: 0, // Default storage engine
                    tags: vec![],
                    description: None,
                    filterable_columns: vec![
                        crate::proto::proximadb_v1::FilterableColumnSpec {
                            name: "user_id".to_string(),
                            data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                            ..Default::default()
                        },
                        crate::proto::proximadb_v1::FilterableColumnSpec {
                            name: "email".to_string(),
                            data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                            ..Default::default()
                        },
                    ],
                    index_configs: vec![],
                    quantization: None,
                    storage_config: None,
                    primary_index: "default".to_string(),
                    auto_index_selection: false,
                    owner: None,
                    embedding_models: vec![],
                    // schema field not in proto
                    /*schema: Some(crate::proto::proximadb_v1::CollectionSchema {
                        fields: vec![
                            crate::proto::proximadb_v1::SchemaField {
                                name: "user_id".to_string(),
                                data_type: "string".to_string(),
                                dimension: None,
                            },
                            crate::proto::proximadb_v1::SchemaField {
                                name: "email".to_string(),
                                data_type: "string".to_string(),
                                dimension: None,
                            },
                        ],
                    }),*/
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        Self { collections }
    }
}

// Note: CollectionServiceTrait doesn't exist in current codebase
// This mock provides the same interface for testing
impl MockCollectionService {
    async fn create_collection(&self, _config: &crate::proto::proximadb_v1::CollectionConfig) -> Result<crate::proto::proximadb_v1::CollectionResponse> {
        unimplemented!()
    }
    async fn get_collection(&self, id: &str) -> Result<Option<crate::proto::proximadb_v1::Collection>> {
        Ok(self.collections.get(id).cloned())
    }
    async fn delete_collection(&self, _id: &str) -> Result<crate::proto::proximadb_v1::CollectionResponse> {
        unimplemented!()
    }
    async fn list_collections(&self) -> Result<Vec<crate::proto::proximadb_v1::Collection>> {
        Ok(self.collections.values().cloned().collect())
    }
    async fn update_collection(&self, _id: &str, _config: Option<crate::proto::proximadb_v1::CollectionConfig>) -> Result<crate::proto::proximadb_v1::CollectionResponse> {
        unimplemented!()
    }
    async fn resolve_collection_id(&self, name_or_id: &str) -> Result<Option<String>> {
        Ok(self.collections.get(name_or_id).map(|c| c.id.clone()))
    }
    async fn get_collection_by_name(&self, name: &str) -> Result<Option<crate::proto::proximadb_v1::Collection>> {
        Ok(self.collections.get(name).cloned())
    }
}

async fn setup_analyzer_with_mock() -> Analyzer {
    // Create a real CollectionService for testing
    use crate::storage::metadata::backends::universal_backend::{UniversalMetadataBackend, UniversalMetadataConfig};
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let fs_config = FilesystemConfig {
        default_fs: Some(format!("file://{}", temp_dir.path().display())),
        ..Default::default()
    };
    let filesystem_factory = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());

    let config = UniversalMetadataConfig {
        storage_url: format!("file://{}", temp_dir.path().display()),
        ..Default::default()
    };
    let metadata_backend = Arc::new(UniversalMetadataBackend::new(config, filesystem_factory).await.unwrap());

    let storage_config = StorageConfig {
        metadata_url: format!("file://{}", temp_dir.path().display()),
        ..Default::default()
    };

    let collection_service = Arc::new(CollectionService::new(metadata_backend, storage_config).await.unwrap());

    // Create the test collections in the actual service
    let products_config = crate::proto::proximadb_v1::CollectionConfig {
        name: "products".to_string(),
        dimension: 1536,
        distance_metric: 0,
        storage_engine: 0,
        tags: vec![],
        description: None,
        filterable_columns: vec![
            crate::proto::proximadb_v1::FilterableColumnSpec {
                name: "id".to_string(),
                data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                ..Default::default()
            },
            crate::proto::proximadb_v1::FilterableColumnSpec {
                name: "name".to_string(),
                data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                ..Default::default()
            },
            crate::proto::proximadb_v1::FilterableColumnSpec {
                name: "price".to_string(),
                data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableFloat as i32,
                ..Default::default()
            },
            crate::proto::proximadb_v1::FilterableColumnSpec {
                name: "category".to_string(),
                data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                ..Default::default()
            },
            crate::proto::proximadb_v1::FilterableColumnSpec {
                name: "embedding".to_string(),
                data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableFloat as i32,
                ..Default::default()
            },
        ],
        index_configs: vec![],
        quantization: None,
        storage_config: None,
        primary_index: "default".to_string(),
        auto_index_selection: false,
        owner: None,
        embedding_models: vec![],
        ..Default::default()
    };

    let users_config = crate::proto::proximadb_v1::CollectionConfig {
        name: "app_users".to_string(), // Must be at least 8 characters
        dimension: 128, // Non-zero dimension required
        distance_metric: 0,
        storage_engine: 0,
        tags: vec![],
        description: None,
        filterable_columns: vec![
            crate::proto::proximadb_v1::FilterableColumnSpec {
                name: "id".to_string(), // Add id column for ambiguity test
                data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                ..Default::default()
            },
            crate::proto::proximadb_v1::FilterableColumnSpec {
                name: "user_id".to_string(),
                data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                ..Default::default()
            },
            crate::proto::proximadb_v1::FilterableColumnSpec {
                name: "email".to_string(),
                data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                ..Default::default()
            },
        ],
        index_configs: vec![],
        quantization: None,
        storage_config: None,
        primary_index: "default".to_string(),
        auto_index_selection: false,
        owner: None,
        embedding_models: vec![],
        ..Default::default()
    };

    // Create the collections
    collection_service.create_collection(&products_config).await.expect("Failed to create products collection");
    collection_service.create_collection(&users_config).await.expect("Failed to create app_users collection");

    Analyzer::new(collection_service)
}

#[tokio::test]
async fn test_analyze_simple_select_success() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT id, name FROM products";
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_ok(), "Semantic analysis failed: {:?}", result.err());
    let scope = result.unwrap();

    // Verify 'products' table is in scope
    assert!(scope.lookup("products").is_some());
    if let Some(Symbol::Table { name, columns }) = scope.lookup("products") {
        assert_eq!(name, "products");
        assert!(columns.contains_key("id"));
        assert!(columns.contains_key("name"));
        assert_eq!(columns["id"].data_type, DataType::String);
        assert_eq!(columns["name"].data_type, DataType::String);
    }
}

#[tokio::test]
async fn test_analyze_unknown_table() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT id FROM unknown_table";
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Table not found"));
}

#[tokio::test]
async fn test_analyze_unknown_column() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT unknown_col FROM products";
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Identifier not found"));
}

#[tokio::test]
async fn test_analyze_where_clause_type_mismatch() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT id FROM products WHERE price = 'abc'"; // price is float64, 'abc' is string
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Type mismatch"));
}

#[tokio::test]
async fn test_analyze_vector_similarity_function() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT VECTOR_SIMILARITY(embedding, [0.1, 0.2]) FROM products";
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_ok(), "Semantic analysis failed: {:?}", result.err());
}

#[tokio::test]
async fn test_analyze_vector_similarity_function_invalid_args() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT VECTOR_SIMILARITY(id, name) FROM products"; // id and name are strings
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Invalid arguments for vector similarity function"));
}

#[tokio::test]
async fn test_analyze_group_by() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT category, COUNT(*) FROM products GROUP BY category";
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_ok(), "Semantic analysis failed: {:?}", result.err());
}

#[tokio::test]
async fn test_analyze_having() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT category, COUNT(*) FROM products GROUP BY category HAVING COUNT(*) > 10";
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_ok(), "Semantic analysis failed: {:?}", result.err());
}

#[tokio::test]
async fn test_analyze_join() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT p.name, u.email FROM products AS p JOIN app_users AS u ON p.id = u.user_id";
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_ok(), "Semantic analysis failed: {:?}", result.err());

    let scope = result.unwrap();
    // Verify both tables are in scope
    assert!(scope.lookup("p").is_some());
    assert!(scope.lookup("u").is_some());

    // Verify columns are resolved
    if let Some(Symbol::Table { columns, .. }) = scope.lookup("p") {
        assert!(columns.contains_key("name"));
        assert!(columns.contains_key("id"));
    }
    if let Some(Symbol::Table { columns, .. }) = scope.lookup("u") {
        assert!(columns.contains_key("email"));
        assert!(columns.contains_key("user_id"));
    }
}

#[tokio::test]
async fn test_analyze_join_ambiguous_column() {
    let analyzer = setup_analyzer_with_mock().await;
    // Both products and app_users have an 'id' column (products.id, app_users.user_id - but parser might simplify)
    // This test assumes the parser might simplify 'p.id' to 'id' if not careful
    let sql = "SELECT id FROM products JOIN app_users ON products.id = app_users.user_id";
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    // Expect an error because 'id' is ambiguous without qualification
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Ambiguous column reference"));
}

#[tokio::test]
async fn test_analyze_sks_similar() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT SIMILAR(embedding, [0.1, 0.2]) FROM products";
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_ok(), "Semantic analysis failed for SKS_SIMILAR: {:?}", result.err());
}

#[tokio::test]
async fn test_analyze_sks_similar_invalid_field() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT SIMILAR(name, [0.1, 0.2]) FROM products"; // 'name' is string, not vector
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("SIMILAR field must be a vector type"));
}

#[tokio::test]
async fn test_analyze_sks_follow() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT FOLLOW('user1', 'friends', 3) FROM app_users";
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_ok(), "Semantic analysis failed for SKS_FOLLOW: {:?}", result.err());
}

#[tokio::test]
async fn test_analyze_sks_follow_invalid_start_node() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT FOLLOW(price, 'friends', 3) FROM products"; // 'price' is float, not string/int
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("FOLLOW start node must be a string or integer ID"));
}

#[tokio::test]
async fn test_analyze_sks_assemble() {
    let analyzer = setup_analyzer_with_mock().await;
    let sql = "SELECT ASSEMBLE(id, name, category) FROM products";
    let parser = SqlFrontendParser::new();
    let query = parser.parse(sql).unwrap();

    let result = analyzer.analyze(&query).await;
    assert!(result.is_ok(), "Semantic analysis failed for SKS_ASSEMBLE: {:?}", result.err());
}
