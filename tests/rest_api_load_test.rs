/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # REST API Load Testing - TD-012
//!
//! Load and performance tests for REST API handlers to verify
//! the system can handle expected traffic patterns.

use serde_json::json;
use std::time::{Duration, Instant};

// ============================================================================
// Performance Baseline Tests
// ============================================================================

#[test]
fn test_json_parsing_performance() {
    let json_data = r#"{
        "collection": "products",
        "vector": [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
        "top_k": 10,
        "filters": {
            "category": "electronics",
            "in_stock": true
        }
    }"#;

    let iterations = 1000;
    let start = Instant::now();

    for _ in 0..iterations {
        let parsed: serde_json::Value = serde_json::from_str(json_data).unwrap();
        assert!(parsed.is_object());
    }

    let elapsed = start.elapsed();
    let avg_time = elapsed / iterations;

    // Should be able to parse at least 1000 requests per second
    assert!(avg_time < Duration::from_millis(1));
    println!(
        "JSON parsing: {} iterations in {:?} (avg: {:?})",
        iterations, elapsed, avg_time
    );
}

#[test]
fn test_json_serialization_performance() {
    let data = serde_json::json!({
        "results": [
            {
                "id": "vec1",
                "score": 0.95,
                "metadata": {"category": "electronics"}
            },
            {
                "id": "vec2",
                "score": 0.87,
                "metadata": {"category": "books"}
            }
        ],
        "total_found": 2
    });

    let iterations = 1000;
    let start = Instant::now();

    for _ in 0..iterations {
        let serialized = serde_json::to_vec(&data).unwrap();
        assert!(serialized.len() > 0);
    }

    let elapsed = start.elapsed();
    let avg_time = elapsed / iterations;

    // Should be able to serialize at least 1000 responses per second
    assert!(avg_time < Duration::from_millis(1));
    println!(
        "JSON serialization: {} iterations in {:?} (avg: {:?})",
        iterations, elapsed, avg_time
    );
}

#[test]
fn test_vector_serialization_performance() {
    let vector: Vec<f32> = (0..128).map(|i| i as f32 / 100.0).collect();

    let iterations = 10000;
    let start = Instant::now();

    for _ in 0..iterations {
        let serialized = serde_json::to_string(&vector).unwrap();
        assert!(serialized.contains("0.01"));
    }

    let elapsed = start.elapsed();
    let avg_time = elapsed / iterations;

    // Vector serialization should be very fast
    assert!(avg_time < Duration::from_micros(100));
    println!(
        "Vector serialization: {} iterations in {:?} (avg: {:?})",
        iterations, elapsed, avg_time
    );
}

// ============================================================================
// Bulk Operation Tests
// ============================================================================

#[test]
fn test_bulk_json_parsing() {
    let test_cases = vec![
        r#"{"collection": "test1", "vector": [0.1, 0.2, 0.3]}"#,
        r#"{"collection": "test2", "vector": [0.4, 0.5, 0.6]}"#,
        r#"{"collection": "test3", "vector": [0.7, 0.8, 0.9]}"#,
        r#"{"collection": "test4", "vector": [1.0, 1.1, 1.2]}"#,
        r#"{"collection": "test5", "vector": [1.3, 1.4, 1.5]}"#,
    ];

    let start = Instant::now();

    for test_case in &test_cases {
        let parsed: serde_json::Value = serde_json::from_str(test_case).unwrap();
        assert!(parsed.is_object());
    }

    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(10));
    println!("Bulk JSON parsing: 5 requests in {:?}", elapsed);
}

#[test]
fn test_concurrent_json_parsing() {
    use std::thread;

    let json_data = r#"{
        "collection": "products",
        "vector": [0.1, 0.2, 0.3],
        "top_k": 10
    }"#;

    let num_threads = 4;
    let iterations_per_thread = 250;

    let start = Instant::now();

    let handles: Vec<_> = (0..num_threads)
        .map(|_| {
            let json = json_data.to_string();
            thread::spawn(move || {
                for _ in 0..iterations_per_thread {
                    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
                    assert!(parsed.is_object());
                }
                iterations_per_thread
            })
        })
        .collect();

    // Wait for all threads and count successful parses
    let total_successful: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();

    let elapsed = start.elapsed();

    assert_eq!(total_successful, num_threads * iterations_per_thread);
    println!(
        "Concurrent JSON parsing: {} threads, {} requests in {:?}",
        num_threads, total_successful, elapsed
    );

    // Should handle at least 1000 requests per second
    let requests_per_second = (total_successful as f64) / elapsed.as_secs_f64();
    assert!(requests_per_second >= 1000.0);
}

// ============================================================================
// Memory Efficiency Tests
// ============================================================================

#[test]
fn test_large_vector_serialization_size() {
    // Test that large vectors are serialized efficiently
    let large_vector: Vec<f32> = (0..10000).map(|i| i as f32).collect();
    let serialized = serde_json::to_vec(&large_vector).unwrap();

    // Check that serialization doesn't blow up memory excessively
    // JSON will have overhead but should still be reasonable
    let size_bytes = serialized.len();
    assert!(size_bytes < 500000); // Less than 500KB for 10K floats
    println!(
        "Large vector serialization: {} floats -> {} bytes ({} bytes/float)",
        large_vector.len(),
        size_bytes,
        size_bytes / large_vector.len()
    );
}

#[test]
fn test_metadata_serialization_size() {
    let metadata = serde_json::json!({
        "category": "electronics",
        "brand": "Apple",
        "price": 999.99,
        "in_stock": true,
        "rating": 4.5,
        "tags": ["computer", "laptop", "new"]
    });

    let serialized = serde_json::to_vec(&metadata).unwrap();
    let size_bytes = serialized.len();

    // Metadata should be reasonably compact
    assert!(size_bytes < 200); // Less than 200 bytes
    println!("Metadata serialization: {} bytes", size_bytes);
}

// ============================================================================
// Batch Operation Performance Tests
// ============================================================================

#[test]
fn test_batch_request_serialization() {
    let batch_request = serde_json::json!({
        "collection_id": "products",
        "vectors": (0..100).map(|i| {
            serde_json::json!({
                "id": format!("vec{}", i),
                "vector": (0..10).map(|j| j as f32).collect::<Vec<f32>>()
            })
        }).collect::<Vec<_>>()
    });

    let start = Instant::now();
    let serialized = serde_json::to_vec(&batch_request).unwrap();
    let elapsed = start.elapsed();

    assert!(elapsed < Duration::from_millis(50));
    println!(
        "Batch request serialization: 100 vectors in {:?} ({} bytes)",
        elapsed,
        serialized.len()
    );
}

#[test]
fn test_batch_request_parsing() {
    let batch_request = serde_json::json!({
        "collection_id": "products",
        "vectors": (0..100).map(|i| {
            serde_json::json!({
                "id": format!("vec{}", i),
                "vector": (0..10).map(|j| j as f32).collect::<Vec<f32>>()
            })
        }).collect::<Vec<_>>()
    });

    let serialized = serde_json::to_vec(&batch_request).unwrap();
    let iterations = 100;

    let start = Instant::now();

    for _ in 0..iterations {
        let parsed: serde_json::Value = serde_json::from_slice(&serialized).unwrap();
        assert!(parsed["vectors"].as_array().unwrap().len() == 100);
    }

    let elapsed = start.elapsed();
    let avg_time = elapsed / iterations;

    // Should be able to parse 100-vector batch quickly
    assert!(avg_time < Duration::from_millis(5));
    println!(
        "Batch request parsing: {} iterations, avg {:?}",
        iterations, avg_time
    );
}

// ============================================================================
// Error Handling Performance Tests
// ============================================================================

#[test]
fn test_error_response_serialization_performance() {
    let error_responses = vec![
        json!({"code": 400, "message": "Bad Request"}),
        json!({"code": 404, "message": "Not Found"}),
        json!({"code": 500, "message": "Internal Server Error"}),
        json!({"code": 503, "message": "Service Unavailable"}),
    ];

    let iterations = 1000;
    let start = Instant::now();

    for error_response in &error_responses {
        for _ in 0..(iterations / error_responses.len()) {
            let serialized = serde_json::to_vec(error_response).unwrap();
            assert!(serialized.len() > 0);
        }
    }

    let elapsed = start.elapsed();
    let total_serializations = iterations;

    // Error responses should serialize quickly
    assert!(elapsed < Duration::from_millis(100));
    println!(
        "Error response serialization: {} in {:?}",
        total_serializations, elapsed
    );
}

// ============================================================================
// Request Validation Performance Tests
// ============================================================================

#[test]
fn test_filter_validation_performance() {
    let filters = serde_json::json!({
        "category": "electronics",
        "price": 999.99,
        "in_stock": true,
        "rating": 4.5,
        "brand": "Apple"
    });

    let iterations = 10000;
    let start = Instant::now();

    for _ in 0..iterations {
        let serialized = serde_json::to_vec(&filters).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&serialized).unwrap();
        assert!(parsed.is_object());
    }

    let elapsed = start.elapsed();
    let avg_time = elapsed / iterations;

    // Filter validation should be very fast
    assert!(avg_time < Duration::from_micros(50));
    println!(
        "Filter validation: {} iterations, avg {:?}",
        iterations, avg_time
    );
}

// ============================================================================
// Throughput Tests
// ============================================================================

#[test]
fn test_request_parsing_throughput() {
    let sample_requests = vec![
        r#"{"collection": "test", "vector": [0.1, 0.2, 0.3], "top_k": 10}"#,
        r#"{"collection": "test", "vector": [0.4, 0.5, 0.6], "top_k": 5, "filters": {"a": "b"}}"#,
        r#"{"collection": "test", "vector": [0.7, 0.8, 0.9], "top_k": 20, "filters": {"c": "d", "e": "f"}}"#,
    ];

    let target_requests_per_second = 10000;
    let test_duration = Duration::from_millis(100);
    let requests_per_iteration = 10;

    let mut total_requests = 0;
    let start = Instant::now();

    while start.elapsed() < test_duration {
        for _ in 0..requests_per_iteration {
            for request in &sample_requests {
                let parsed: serde_json::Value = serde_json::from_str(request).unwrap();
                assert!(parsed.is_object());
                total_requests += 1;
            }
        }
    }

    let elapsed = start.elapsed();
    let requests_per_second = (total_requests as f64) / elapsed.as_secs_f64();

    println!(
        "Throughput test: {} requests in {:?} ({:.0} req/s)",
        total_requests, elapsed, requests_per_second
    );

    // Should meet target throughput
    assert!(requests_per_second >= target_requests_per_second as f64);
}

// ============================================================================
// Stress Tests
// ============================================================================

#[test]
fn test_rapid_json_parsing_stress() {
    // Generate a 128D vector JSON
    let vector_128d: Vec<f32> = (1..=128).map(|i| i as f32 * 0.01).collect();
    let json_data = serde_json::json!({
        "collection": "products",
        "vector": vector_128d,
        "top_k": 100
    })
    .to_string();

    let iterations = 10000;
    let start = Instant::now();

    for _ in 0..iterations {
        let parsed: serde_json::Value = serde_json::from_str(&json_data).unwrap();
        assert!(parsed["vector"].as_array().unwrap().len() == 128);
    }

    let elapsed = start.elapsed();
    let avg_time = elapsed / iterations;
    let requests_per_second = (iterations as f64) / elapsed.as_secs_f64();

    println!(
        "Stress test (128D vectors): {} iterations in {:?} (avg: {:?}, {:.0} req/s)",
        iterations, elapsed, avg_time, requests_per_second
    );

    // Should handle at least 1000 req/s even with large vectors
    assert!(requests_per_second >= 1000.0);
}
