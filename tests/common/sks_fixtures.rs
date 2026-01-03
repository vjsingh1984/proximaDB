// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! SKS test fixtures for graph-first migration
//!
//! This module provides test data generators for validating the SKS graph-first architecture.

use proximadb::proto::proximadb_v1::{
    EmbeddingVersion, Entity, Modality, Provenance, Relation, TypedField, TypedMetadata,
};
use rand::Rng;
use std::collections::HashMap;

/// Test knowledge graph with entities and relations
pub struct TestKnowledgeGraph {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub embeddings: Vec<Vec<f32>>,
}

impl TestKnowledgeGraph {
    /// Create a small test graph (100 entities, 300 relations)
    pub fn small() -> Self {
        Self::generate(100, 300, 128)
    }

    /// Create a medium test graph (1K entities, 5K relations)
    pub fn medium() -> Self {
        Self::generate(1000, 5000, 384)
    }

    /// Create a large test graph (10K entities, 50K relations)
    pub fn large() -> Self {
        Self::generate(10_000, 50_000, 768)
    }

    /// Generate a test graph with specified parameters
    fn generate(num_entities: usize, num_relations: usize, dim: usize) -> Self {
        let mut rng = rand::thread_rng();
        let mut entities = Vec::new();
        let mut embeddings = Vec::new();

        // Generate entities with embeddings
        for i in 0..num_entities {
            let embedding: Vec<f32> = (0..dim).map(|_| rng.r#gen::<f32>()).collect();
            embeddings.push(embedding.clone());

            let entity = Entity {
                id: format!("entity-{}", i),
                collection_id: "test-collection".to_string(),
                embeddings: vec![EmbeddingVersion {
                    model_id: "test-model".to_string(),
                    model_version: "v1".to_string(),
                    vector: embedding,
                    dimension: dim as u32,
                    created_at_ms: 1234567890,
                    model_params: HashMap::new(),
                    modality: Modality::Text as i32,
                }],
                typed_metadata: None,
                flexible_metadata: HashMap::new(),
                provenance: Some(Provenance {
                    source_id: format!("source-{}", i % 10),
                    chunk_id: format!("chunk-{}", i),
                    chunk_position: (i % 100) as u32,
                    extraction_method: "test-extraction".to_string(),
                    extracted_at_ms: 1234567890,
                    metadata: HashMap::new(),
                }),
                temporal: None,
                relations: vec![],
            };
            entities.push(entity);
        }

        // Generate relations (create a graph structure)
        let mut relations = Vec::new();
        let num_entities_usize = num_entities;

        for i in 0..num_relations {
            let source_idx = rng.gen_range(0..num_entities_usize);
            let target_idx = rng.gen_range(0..num_entities_usize);

            // Avoid self-loops
            if source_idx == target_idx {
                continue;
            }

            let relation = Relation {
                source_entity_id: format!("entity-{}", source_idx),
                target_entity_id: format!("entity-{}", target_idx),
                relation_type: Self::random_relation_type(&mut rng),
                weight: rng.r#gen::<f32>(),
                created_at_ms: 1234567890,
                properties: HashMap::new(),
            };
            relations.push(relation);
        }

        Self {
            entities,
            relations,
            embeddings,
        }
    }

    /// Create a realistic research paper citation graph
    pub fn research_papers() -> Self {
        let num_papers = 500;
        let dim = 768; // BERT-like embeddings
        let mut rng = rand::thread_rng();
        let mut entities = Vec::new();
        let mut embeddings = Vec::new();

        // Generate papers with metadata
        for i in 0..num_papers {
            let embedding: Vec<f32> = (0..dim).map(|_| rng.r#gen::<f32>()).collect();
            embeddings.push(embedding.clone());

            let mut fields = HashMap::new();
            fields.insert(
                "title".to_string(),
                TypedField {
                    value: Some(
                        proximadb::proto::proximadb_v1::typed_field::Value::StringValue(format!(
                            "Research Paper {}",
                            i
                        )),
                    ),
                    indexed: true,
                    filterable: true,
                },
            );
            fields.insert(
                "year".to_string(),
                TypedField {
                    value: Some(
                        proximadb::proto::proximadb_v1::typed_field::Value::IntValue(
                            2020 + (i % 5) as i64,
                        ),
                    ),
                    indexed: true,
                    filterable: true,
                },
            );
            fields.insert(
                "citations".to_string(),
                TypedField {
                    value: Some(
                        proximadb::proto::proximadb_v1::typed_field::Value::IntValue(
                            rng.gen_range(0..100),
                        ),
                    ),
                    indexed: true,
                    filterable: true,
                },
            );

            let entity = Entity {
                id: format!("paper-{}", i),
                collection_id: "research-papers".to_string(),
                embeddings: vec![EmbeddingVersion {
                    model_id: "bert-base-uncased".to_string(),
                    model_version: "v1".to_string(),
                    vector: embedding,
                    dimension: dim as u32,
                    created_at_ms: 1234567890,
                    model_params: HashMap::new(),
                    modality: Modality::Text as i32,
                }],
                typed_metadata: Some(TypedMetadata { fields }),
                flexible_metadata: HashMap::new(),
                provenance: Some(Provenance {
                    source_id: "arxiv".to_string(),
                    chunk_id: format!("arxiv-{}", i),
                    chunk_position: 0,
                    extraction_method: "pdf-parser".to_string(),
                    extracted_at_ms: 1234567890,
                    metadata: HashMap::new(),
                }),
                temporal: None,
                relations: vec![],
            };
            entities.push(entity);
        }

        // Generate citation relations (older papers cited by newer ones)
        let mut relations = Vec::new();
        for i in 0..num_papers {
            // Each paper cites 2-5 previous papers
            let num_citations = rng.gen_range(2..6).min(i);
            for _ in 0..num_citations {
                let cited_paper = rng.gen_range(0..i);
                relations.push(Relation {
                    source_entity_id: format!("paper-{}", i),
                    target_entity_id: format!("paper-{}", cited_paper),
                    relation_type: "cites".to_string(),
                    weight: 1.0,
                    created_at_ms: 1234567890,
                    properties: HashMap::new(),
                });
            }
        }

        Self {
            entities,
            relations,
            embeddings,
        }
    }

    /// Create an e-commerce product catalog
    pub fn ecommerce() -> Self {
        let num_products = 1000;
        let dim = 512;
        let mut rng = rand::thread_rng();
        let mut entities = Vec::new();
        let mut embeddings = Vec::new();

        let categories = vec!["Electronics", "Clothing", "Books", "Home", "Sports"];

        // Generate products with metadata
        for i in 0..num_products {
            let embedding: Vec<f32> = (0..dim).map(|_| rng.r#gen::<f32>()).collect();
            embeddings.push(embedding.clone());

            let category = categories[i % categories.len()];

            let mut fields = HashMap::new();
            fields.insert(
                "name".to_string(),
                TypedField {
                    value: Some(
                        proximadb::proto::proximadb_v1::typed_field::Value::StringValue(format!(
                            "{} Product {}",
                            category, i
                        )),
                    ),
                    indexed: true,
                    filterable: true,
                },
            );
            fields.insert(
                "price".to_string(),
                TypedField {
                    value: Some(
                        proximadb::proto::proximadb_v1::typed_field::Value::DoubleValue(
                            rng.gen_range(10.0..1000.0),
                        ),
                    ),
                    indexed: true,
                    filterable: true,
                },
            );
            fields.insert(
                "category".to_string(),
                TypedField {
                    value: Some(
                        proximadb::proto::proximadb_v1::typed_field::Value::StringValue(
                            category.to_string(),
                        ),
                    ),
                    indexed: true,
                    filterable: true,
                },
            );
            fields.insert(
                "rating".to_string(),
                TypedField {
                    value: Some(
                        proximadb::proto::proximadb_v1::typed_field::Value::DoubleValue(
                            rng.gen_range(3.0..5.0),
                        ),
                    ),
                    indexed: true,
                    filterable: true,
                },
            );

            let entity = Entity {
                id: format!("product-{}", i),
                collection_id: "ecommerce".to_string(),
                embeddings: vec![EmbeddingVersion {
                    model_id: "product-embeddings-v1".to_string(),
                    model_version: "v1".to_string(),
                    vector: embedding,
                    dimension: dim as u32,
                    created_at_ms: 1234567890,
                    model_params: HashMap::new(),
                    modality: Modality::Text as i32,
                }],
                typed_metadata: Some(TypedMetadata { fields }),
                flexible_metadata: HashMap::new(),
                provenance: Some(Provenance {
                    source_id: "product-catalog".to_string(),
                    chunk_id: format!("prod-{}", i),
                    chunk_position: 0,
                    extraction_method: "api-import".to_string(),
                    extracted_at_ms: 1234567890,
                    metadata: HashMap::new(),
                }),
                temporal: None,
                relations: vec![],
            };
            entities.push(entity);
        }

        // Generate relations (related products, same category, bought together)
        let mut relations = Vec::new();
        for i in 0..num_products {
            // Each product has 3-7 related products
            let num_related = rng.gen_range(3..8);
            for _ in 0..num_related {
                let related_product = rng.gen_range(0..num_products);
                if related_product == i {
                    continue;
                }

                relations.push(Relation {
                    source_entity_id: format!("product-{}", i),
                    target_entity_id: format!("product-{}", related_product),
                    relation_type: "related_to".to_string(),
                    weight: rng.r#gen::<f32>(),
                    created_at_ms: 1234567890,
                    properties: HashMap::new(),
                });
            }
        }

        Self {
            entities,
            relations,
            embeddings,
        }
    }

    fn random_relation_type(rng: &mut impl Rng) -> String {
        let types = vec!["related_to", "derived_from", "references", "similar_to"];
        types[rng.gen_range(0..types.len())].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_graph_creation() {
        let graph = TestKnowledgeGraph::small();
        assert_eq!(graph.entities.len(), 100);
        assert!(graph.relations.len() <= 300);
        assert_eq!(graph.embeddings.len(), 100);
        assert_eq!(graph.entities[0].embeddings[0].dimension, 128);
    }

    #[test]
    fn test_medium_graph_creation() {
        let graph = TestKnowledgeGraph::medium();
        assert_eq!(graph.entities.len(), 1000);
        assert!(graph.relations.len() <= 5000);
        assert_eq!(graph.embeddings.len(), 1000);
        assert_eq!(graph.entities[0].embeddings[0].dimension, 384);
    }

    #[test]
    fn test_research_papers_fixture() {
        let graph = TestKnowledgeGraph::research_papers();
        assert_eq!(graph.entities.len(), 500);
        assert!(graph.relations.len() > 0);

        // Verify metadata structure
        let paper = &graph.entities[0];
        assert!(paper.typed_metadata.is_some());
        let metadata = paper.typed_metadata.as_ref().unwrap();
        assert!(metadata.fields.contains_key("title"));
        assert!(metadata.fields.contains_key("year"));
        assert!(metadata.fields.contains_key("citations"));

        // Verify all relations are citations
        for relation in &graph.relations {
            assert_eq!(relation.relation_type, "cites");
        }
    }

    #[test]
    fn test_ecommerce_fixture() {
        let graph = TestKnowledgeGraph::ecommerce();
        assert_eq!(graph.entities.len(), 1000);
        assert!(graph.relations.len() > 0);

        // Verify product metadata
        let product = &graph.entities[0];
        assert!(product.typed_metadata.is_some());
        let metadata = product.typed_metadata.as_ref().unwrap();
        assert!(metadata.fields.contains_key("name"));
        assert!(metadata.fields.contains_key("price"));
        assert!(metadata.fields.contains_key("category"));
        assert!(metadata.fields.contains_key("rating"));
    }

    #[test]
    fn test_relations_no_self_loops() {
        let graph = TestKnowledgeGraph::small();
        for relation in &graph.relations {
            assert_ne!(relation.source_entity_id, relation.target_entity_id);
        }
    }

    #[test]
    fn test_embeddings_match_entities() {
        let graph = TestKnowledgeGraph::small();
        for (entity, embedding) in graph.entities.iter().zip(graph.embeddings.iter()) {
            assert_eq!(entity.embeddings[0].vector.len(), embedding.len());
            assert_eq!(&entity.embeddings[0].vector, embedding);
        }
    }
}
