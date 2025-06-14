// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! API layer performance benchmarks

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use std::time::Duration;
use tokio::runtime::Runtime;
use serde_json::json;

use super::{BenchmarkConfig, BenchmarkDataGenerator, BenchmarkRunner};

/// Benchmark REST API performance
pub fn benchmark_rest_api_performance(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("rest_api_performance");
    group.measurement_time(Duration::from_secs(30));
    
    let request_counts = vec![10, 100, 500];
    let dimension = 256;
    
    for &request_count in &request_counts {
        group.throughput(Throughput::Elements(request_count as u64));
        
        // Vector insertion via REST API
        group.bench_with_input(
            BenchmarkId::new("rest_insert", format!("{}requests", request_count)),
            &request_count,
            |b, &request_count| {
                let vectors = generator.generate_vectors(request_count, dimension, 0.1, "rest_bench");
                
                b.iter(|| {
                    // Simulate REST API calls
                    for vector in &vectors {
                        let payload = json!({
                            "id": vector.id,
                            "collection_id": vector.collection_id,
                            "vector": vector.vector,
                            "metadata": vector.metadata,
                            "timestamp": vector.timestamp.to_rfc3339(),
                            "expires_at": vector.expires_at.map(|t| t.to_rfc3339())
                        });
                        
                        // Simulate JSON serialization overhead
                        let _serialized = serde_json::to_string(&payload).unwrap();
                        black_box(_serialized);
                    }
                });
            },
        );
        
        // Vector search via REST API
        group.bench_with_input(
            BenchmarkId::new("rest_search", format!("{}requests", request_count)),
            &request_count,
            |b, &request_count| {
                let queries = generator.generate_query_vectors(request_count, dimension, 0.1);
                
                b.iter(|| {
                    for query in &queries {
                        let search_payload = json!({
                            "collection_id": "rest_bench",
                            "vector": query,
                            "k": 10,
                            "similarity_threshold": 0.8,
                            "metadata_filters": {}
                        });
                        
                        // Simulate JSON serialization overhead
                        let _serialized = serde_json::to_string(&search_payload).unwrap();
                        black_box(_serialized);
                    }
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark gRPC API performance
pub fn benchmark_grpc_api_performance(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("grpc_api_performance");
    group.measurement_time(Duration::from_secs(25));
    
    let request_counts = vec![10, 100, 500];
    let dimension = 256;
    
    for &request_count in &request_counts {
        group.throughput(Throughput::Elements(request_count as u64));
        
        // gRPC vector insertion
        group.bench_with_input(
            BenchmarkId::new("grpc_insert", format!("{}requests", request_count)),
            &request_count,
            |b, &request_count| {
                let vectors = generator.generate_vectors(request_count, dimension, 0.1, "grpc_bench");
                
                b.iter(|| {
                    for vector in &vectors {
                        // Simulate Protocol Buffer serialization
                        let proto_size = estimate_protobuf_size(&vector);
                        black_box(proto_size);
                    }
                });
            },
        );
        
        // gRPC streaming operations
        group.bench_with_input(
            BenchmarkId::new("grpc_stream", format!("{}requests", request_count)),
            &request_count,
            |b, &request_count| {
                let vectors = generator.generate_vectors(request_count, dimension, 0.1, "grpc_stream");
                
                b.iter(|| {
                    // Simulate streaming batch insert
                    let mut total_size = 0;
                    for vector in &vectors {
                        total_size += estimate_protobuf_size(&vector);
                    }
                    black_box(total_size);
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark API request routing and authentication
pub fn benchmark_api_routing_auth(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("api_routing_auth");
    group.measurement_time(Duration::from_secs(20));
    
    let request_counts = vec![100, 1000, 5000];
    
    for &request_count in &request_counts {
        group.throughput(Throughput::Elements(request_count as u64));
        
        // Authentication overhead
        group.bench_with_input(
            BenchmarkId::new("auth_validation", format!("{}requests", request_count)),
            &request_count,
            |b, &request_count| {
                b.iter(|| {
                    for i in 0..request_count {
                        // Simulate JWT token validation
                        let token = format!("bearer_token_{}", i);
                        let _validated = simulate_jwt_validation(&token);
                        black_box(_validated);
                    }
                });
            },
        );
        
        // Request routing overhead
        group.bench_with_input(
            BenchmarkId::new("request_routing", format!("{}requests", request_count)),
            &request_count,
            |b, &request_count| {
                b.iter(|| {
                    for i in 0..request_count {
                        // Simulate request routing logic
                        let path = format!("/api/v1/collections/bench_{}/vectors", i % 10);
                        let _route = simulate_route_matching(&path);
                        black_box(_route);
                    }
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark API rate limiting
pub fn benchmark_api_rate_limiting(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("api_rate_limiting");
    group.measurement_time(Duration::from_secs(20));
    
    let request_counts = vec![100, 1000, 5000];
    let tenant_counts = vec![1, 10, 100];
    
    for &request_count in &request_counts {
        for &tenant_count in &tenant_counts {
            if request_count <= 1000 || tenant_count <= 10 { // Skip large combinations
                group.throughput(Throughput::Elements(request_count as u64));
                
                group.bench_with_input(
                    BenchmarkId::new("rate_limit_check", format!("{}req_{}tenants", request_count, tenant_count)),
                    &(request_count, tenant_count),
                    |b, &(request_count, tenant_count)| {
                        b.iter(|| {
                            for i in 0..request_count {
                                let tenant_id = format!("tenant_{}", i % tenant_count);
                                let _allowed = simulate_rate_limit_check(&tenant_id);
                                black_box(_allowed);
                            }
                        });
                    },
                );
            }
        }
    }
    
    group.finish();
}

/// Benchmark API response serialization
pub fn benchmark_api_serialization(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("api_serialization");
    group.measurement_time(Duration::from_secs(25));
    
    let result_counts = vec![10, 100, 1000];
    let dimension = 256;
    
    for &result_count in &result_counts {
        group.throughput(Throughput::Elements(result_count as u64));
        
        // JSON serialization benchmark
        group.bench_with_input(
            BenchmarkId::new("json_serialization", format!("{}results", result_count)),
            &result_count,
            |b, &result_count| {
                let vectors = generator.generate_vectors(result_count, dimension, 0.1, "serial_bench");
                
                b.iter(|| {
                    let response = json!({
                        "results": vectors.iter().map(|v| json!({
                            "id": v.id,
                            "collection_id": v.collection_id,
                            "vector": v.vector,
                            "metadata": v.metadata,
                            "similarity_score": 0.85,
                            "timestamp": v.timestamp.to_rfc3339()
                        })).collect::<Vec<_>>(),
                        "total_found": result_count,
                        "query_time_ms": 15.2
                    });
                    
                    let _serialized = serde_json::to_string(&response).unwrap();
                    black_box(_serialized);
                });
            },
        );
        
        // Protocol Buffer serialization benchmark
        group.bench_with_input(
            BenchmarkId::new("protobuf_serialization", format!("{}results", result_count)),
            &result_count,
            |b, &result_count| {
                let vectors = generator.generate_vectors(result_count, dimension, 0.1, "proto_bench");
                
                b.iter(|| {
                    let mut total_size = 0;
                    for vector in &vectors {
                        total_size += estimate_protobuf_size(&vector);
                    }
                    black_box(total_size);
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark API error handling
pub fn benchmark_api_error_handling(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("api_error_handling");
    group.measurement_time(Duration::from_secs(15));
    
    let error_counts = vec![100, 1000];
    let error_types = vec![
        ("validation_error", "Invalid vector dimension"),
        ("not_found_error", "Collection not found"),
        ("auth_error", "Unauthorized access"),
        ("rate_limit_error", "Rate limit exceeded"),
    ];
    
    for &error_count in &error_counts {
        for (error_type, error_message) in &error_types {
            group.throughput(Throughput::Elements(error_count as u64));
            
            group.bench_with_input(
                BenchmarkId::new("error_handling", format!("{}_{}", error_type, error_count)),
                &(error_count, error_message),
                |b, &(error_count, error_message)| {
                    b.iter(|| {
                        for i in 0..error_count {
                            let error_response = json!({
                                "error": {
                                    "code": "INVALID_REQUEST",
                                    "message": error_message,
                                    "details": {
                                        "request_id": format!("req_{}", i),
                                        "timestamp": chrono::Utc::now().to_rfc3339()
                                    }
                                }
                            });
                            
                            let _serialized = serde_json::to_string(&error_response).unwrap();
                            black_box(_serialized);
                        }
                    });
                },
            );
        }
    }
    
    group.finish();
}

// Helper functions for simulation

fn estimate_protobuf_size(vector: &proximadb::core::VectorRecord) -> usize {
    // Rough estimation of Protocol Buffer serialized size
    let base_size = 50; // Fixed fields
    let vector_size = vector.vector.len() * 4; // f32 values
    let metadata_size = vector.metadata.len() * 20; // Estimated metadata size
    base_size + vector_size + metadata_size
}

fn simulate_jwt_validation(token: &str) -> bool {
    // Simulate JWT validation overhead
    let hash = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    let mut hasher = hash;
    token.hash(&mut hasher);
    hasher.finish() % 2 == 0 // Simulate success/failure
}

fn simulate_route_matching(path: &str) -> String {
    // Simulate route matching logic
    if path.contains("/vectors") {
        "vector_handler".to_string()
    } else if path.contains("/collections") {
        "collection_handler".to_string()
    } else {
        "default_handler".to_string()
    }
}

fn simulate_rate_limit_check(tenant_id: &str) -> bool {
    // Simulate rate limiting check
    let hash = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    let mut hasher = hash;
    tenant_id.hash(&mut hasher);
    hasher.finish() % 100 < 95 // 95% success rate
}

criterion_group!(
    api_benches,
    benchmark_rest_api_performance,
    benchmark_grpc_api_performance,
    benchmark_api_routing_auth,
    benchmark_api_rate_limiting,
    benchmark_api_serialization,
    benchmark_api_error_handling
);

criterion_main!(api_benches);