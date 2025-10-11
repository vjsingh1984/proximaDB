/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Example usage of the Semantic Knowledge Store (SKS) feature

use proximadb::proto::proximadb_v1::{
    VectorRecord,
};
use std::collections::HashMap;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    println!("🚀 ProximaDB Vector Operations Example\n");

    // Example 1: Create a sample vector record
    println!("📄 Example 1: Creating a sample vector record");
    let vector_record = create_sample_vector_record()?;
    println!("   ✅ Created vector record with ID: {}", vector_record.id);
    println!("   ✅ Vector dimension: {}", vector_record.vector.len());

    // Example 2: Demonstrate vector operations
    println!("\n🔍 Example 2: Vector operations");
    let similarity = calculate_cosine_similarity(&vector_record.vector, &vector_record.vector);
    println!("   ✅ Self-similarity (should be 1.0): {:.6}", similarity);

    // Example 3: Demonstrate metadata filtering
    println!("\n📊 Example 3: Metadata operations");
    if let Some(title) = vector_record.metadata.get("title") {
        println!("   ✅ Title metadata: {:?}", title);
    }
    if let Some(year) = vector_record.metadata.get("year") {
        println!("   ✅ Year metadata: {:?}", year);
    }

    // Example 4: Demonstrate metadata operations
    println!("\n📖 Example 4: Advanced metadata operations");
    demonstrate_metadata_operations(&vector_record);

    // Example 5: Search concepts
    println!("\n💼 Example 5: Search concepts and capabilities");
    demonstrate_search_concepts();

    // Example 6: Create a search result example
    println!("\n🌐 Example 6: Search result processing");
    demonstrate_search_results(&vector_record);

    println!("\n✨ ProximaDB vector operations completed successfully!");

    Ok(())
}

/// Create a sample vector record for demonstration
fn create_sample_vector_record() -> anyhow::Result<VectorRecord> {
    let mut metadata = std::collections::HashMap::new();

    // Add some sample metadata
    metadata.insert(
        "title".to_string(),
        proximadb::proto::proximadb_v1::SqlValue {
            value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(
                "Attention Is All You Need".to_string()
            )),
        },
    );

    metadata.insert(
        "year".to_string(),
        proximadb::proto::proximadb_v1::SqlValue {
            value: Some(proximadb::proto::proximadb_v1::sql_value::Value::Int64Value(2017)),
        },
    );

    metadata.insert(
        "domain".to_string(),
        proximadb::proto::proximadb_v1::SqlValue {
            value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(
                "machine learning".to_string()
            )),
        },
    );

    Ok(VectorRecord {
        id: "paper_transformer_2017".to_string(),
        vector: vec![0.1; 1536], // Example 1536-dimensional embedding
        metadata,
        timestamp: Some(chrono::Utc::now().timestamp_millis()),
        updated_at: Some(chrono::Utc::now().timestamp_millis()),
        expires_at: None,
        version: Some(1),
        source: None,
    })
}

/// Calculate cosine similarity between two vectors
fn calculate_cosine_similarity(vec1: &[f32], vec2: &[f32]) -> f32 {
    if vec1.len() != vec2.len() {
        return 0.0;
    }

    let dot_product: f32 = vec1.iter().zip(vec2.iter()).map(|(a, b)| a * b).sum();
    let norm1: f32 = vec1.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm2: f32 = vec2.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm1 == 0.0 || norm2 == 0.0 {
        return 0.0;
    }

    dot_product / (norm1 * norm2)
}

/// Demonstrate metadata filtering capabilities
fn demonstrate_metadata_operations(record: &VectorRecord) {
    println!("\n📋 Metadata operations:");

    for (key, value) in &record.metadata {
        match &value.value {
            Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                println!("   - {}: {} (string)", key, s);
            },
            Some(proximadb::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => {
                println!("   - {}: {} (int)", key, i);
            },
            Some(proximadb::proto::proximadb_v1::sql_value::Value::NumberValue(f)) => {
                println!("   - {}: {} (float)", key, f);
            },
            Some(proximadb::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                println!("   - {}: {} (bool)", key, b);
            },
            _ => {
                println!("   - {}: <unknown type>", key);
            }
        }
    }
}

/// Demonstrate search concepts
fn demonstrate_search_concepts() {
    println!("\n🔍 Search concepts:");
    println!("   - Vector similarity: Find semantically similar vectors using cosine similarity");
    println!("   - Metadata filtering: Filter results based on structured metadata");
    println!("   - Hybrid search: Combine vector similarity with metadata constraints");
    println!("   - Distance metrics: Cosine, Euclidean, Manhattan, Dot Product");

    // Example distance metric usage
    let distance_metrics = vec![
        "Cosine",
        "Euclidean",
        "Manhattan",
        "DotProduct"
    ];

    println!("   - Available distance metrics: {}", distance_metrics.join(", "));
}

/// Example search result structure
#[derive(Debug)]
struct SearchResult {
    record_id: String,
    similarity_score: f32,
    title: String,
    year: i32,
}

impl SearchResult {
    fn from_vector_record(record: &VectorRecord, score: f32) -> Option<Self> {
        let title = match record.metadata.get("title")?.value.as_ref()? {
            proximadb::proto::proximadb_v1::sql_value::Value::StringValue(s) => s.clone(),
            _ => return None,
        };

        let year = match record.metadata.get("year")?.value.as_ref()? {
            proximadb::proto::proximadb_v1::sql_value::Value::Int64Value(y) => *y as i32,
            _ => return None,
        };

        Some(SearchResult {
            record_id: record.id.clone(),
            similarity_score: score,
            title,
            year,
        })
    }
}

/// Demonstrate search result processing
fn demonstrate_search_results(record: &VectorRecord) {
    println!("   🔍 Creating example search result");

    let search_result = SearchResult::from_vector_record(record, 0.95);

    if let Some(result) = search_result {
        println!("   📊 Search result:");
        println!("      - ID: {}", result.record_id);
        println!("      - Score: {:.3}", result.similarity_score);
        println!("      - Title: {}", result.title);
        println!("      - Year: {}", result.year);
    } else {
        println!("   ⚠️ Could not create search result from record");
    }

    println!("   🎯 Search result processing demonstrates how to extract structured data");
}
