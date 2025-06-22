//! Performance benchmark integration tests

use super::common::*;
use anyhow::Result;
use std::time::{Duration, Instant};

#[cfg(test)]
mod performance_tests {
    use super::*;

    #[tokio::test]
    async fn test_vector_insertion_performance() -> Result<()> {
        init_test_env();
        
        let collection_id = generate_test_collection_name();
        let test_cases = vec![
            ("small_batch", 10, 384),
            ("medium_batch", 100, 384),
            ("large_batch", 1000, 384),
            ("high_dimension", 100, 1536),
        ];
        
        for (test_name, batch_size, dimension) in test_cases {
            let (batch, measurement) = measure_performance!(
                &format!("Vector creation - {}", test_name),
                batch_size,
                {
                    create_test_vector_batch(collection_id.clone(), batch_size, dimension)
                }
            );
            
            assert_eq!(batch.len(), batch_size);
            
            // Performance expectations
            match test_name {
                "small_batch" => assert!(measurement.throughput > 1000.0),
                "medium_batch" => assert!(measurement.throughput > 500.0),
                "large_batch" => assert!(measurement.throughput > 100.0),
                "high_dimension" => assert!(measurement.throughput > 50.0),
                _ => {}
            }
        }
        
        println!("✅ Vector insertion performance test completed");
        Ok(())
    }

    #[tokio::test]
    async fn test_distance_calculation_performance() -> Result<()> {
        init_test_env();
        
        let dimensions = vec![384, 768, 1536];
        let num_calculations = 1000;
        
        for dim in dimensions {
            let vec1 = generate_random_vector(dim);
            let vec2 = generate_random_vector(dim);
            
            let (similarities, measurement) = measure_performance!(
                &format!("Cosine similarity - {}D", dim),
                num_calculations,
                {
                    let mut results = Vec::new();
                    for _ in 0..num_calculations {
                        let sim = proximadb::compute::distance::cosine_similarity(&vec1, &vec2);
                        results.push(sim);
                    }
                    results
                }
            );
            
            assert_eq!(similarities.len(), num_calculations);
            
            // All similarities should be valid
            for sim in similarities {
                assert!(sim >= -1.0 && sim <= 1.0);
            }
            
            // Performance expectations (calculations per second)
            match dim {
                384 => assert!(measurement.throughput > 10000.0),
                768 => assert!(measurement.throughput > 5000.0),
                1536 => assert!(measurement.throughput > 2000.0),
                _ => {}
            }
        }
        
        println!("✅ Distance calculation performance test completed");
        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_processing_performance() -> Result<()> {
        init_test_env();
        
        let record_counts = vec![100, 500, 1000, 5000];
        let filterable_columns = vec!["category", "author", "doc_type", "year"];
        
        for count in record_counts {
            // Generate test records with extensive metadata
            let records: Vec<HashMap<String, serde_json::Value>> = (0..count)
                .map(|i| {
                    let mut meta = HashMap::new();
                    // Filterable fields
                    meta.insert("category".to_string(), 
                        serde_json::Value::String(format!("category_{}", i % 10)));
                    meta.insert("author".to_string(), 
                        serde_json::Value::String(format!("author_{}", i % 50)));
                    meta.insert("doc_type".to_string(), 
                        serde_json::Value::String("research_paper".to_string()));
                    meta.insert("year".to_string(), 
                        serde_json::Value::Number(serde_json::Number::from(2020 + (i % 5))));
                    
                    // Extra metadata fields
                    meta.insert("title".to_string(), 
                        serde_json::Value::String(format!("Document Title {}", i)));
                    meta.insert("abstract".to_string(), 
                        serde_json::Value::String(format!("Abstract for document {}", i)));
                    meta.insert("keywords".to_string(), 
                        serde_json::Value::Array(vec![
                            serde_json::Value::String("keyword1".to_string()),
                            serde_json::Value::String("keyword2".to_string()),
                        ]));
                    meta.insert("citation_count".to_string(), 
                        serde_json::Value::Number(serde_json::Number::from(i % 100)));
                    meta.insert("download_count".to_string(), 
                        serde_json::Value::Number(serde_json::Number::from(i * 10)));
                    meta.insert("quality_score".to_string(), 
                        serde_json::Value::Number(serde_json::Number::from_f64(0.5 + (i % 50) as f64 / 100.0).unwrap()));
                    
                    meta
                })
                .collect();
            
            // Test metadata transformation performance
            let (transformed, measurement) = measure_performance!(
                &format!("Metadata transformation - {} records", count),
                count,
                {
                    let mut filterable_batch = Vec::new();
                    let mut extra_meta_batch = Vec::new();
                    
                    for record in &records {
                        let mut filterable = HashMap::new();
                        let mut extra_meta = HashMap::new();
                        
                        for (key, value) in record {
                            if filterable_columns.contains(&key.as_str()) {
                                filterable.insert(key.clone(), value.clone());
                            } else {
                                extra_meta.insert(key.clone(), value.clone());
                            }
                        }
                        
                        filterable_batch.push(filterable);
                        extra_meta_batch.push(extra_meta);
                    }
                    
                    (filterable_batch, extra_meta_batch)
                }
            );
            
            let (filterable_batch, extra_meta_batch) = transformed;
            assert_eq!(filterable_batch.len(), count);
            assert_eq!(extra_meta_batch.len(), count);
            
            // Performance expectations (records per second)
            match count {
                100 => assert!(measurement.throughput > 1000.0),
                500 => assert!(measurement.throughput > 500.0),
                1000 => assert!(measurement.throughput > 300.0),
                5000 => assert!(measurement.throughput > 100.0),
                _ => {}
            }
        }
        
        println!("✅ Metadata processing performance test completed");
        Ok(())
    }

    #[tokio::test]
    async fn test_search_performance_simulation() -> Result<()> {
        init_test_env();
        
        let dataset_sizes = vec![1000, 5000, 10000, 50000];
        let query_vector = generate_random_vector(384);
        
        for dataset_size in dataset_sizes {
            // Simulate memtable search (linear scan)
            let (memtable_results, memtable_measurement) = measure_performance!(
                &format!("Memtable search - {} vectors", dataset_size),
                dataset_size,
                {
                    // Simulate linear scan through all vectors
                    let mut similarities = Vec::new();
                    for i in 0..dataset_size {
                        let candidate_vector = generate_random_vector(384);
                        let sim = proximadb::compute::distance::cosine_similarity(&query_vector, &candidate_vector);
                        similarities.push((i, sim));
                    }
                    
                    // Sort by similarity (top-k)
                    similarities.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                    similarities.into_iter().take(10).collect::<Vec<_>>()
                }
            );
            
            // Simulate VIPER search (indexed lookup)
            let index_overhead = (dataset_size as f64).log2() * 0.001; // Log-time index lookup
            let filter_ratio = 0.1; // Assume 10% of vectors match filter
            let filtered_size = (dataset_size as f64 * filter_ratio) as usize;
            
            let (viper_results, viper_measurement) = measure_performance!(
                &format!("VIPER search - {} vectors (indexed)", dataset_size),
                filtered_size,
                {
                    // Simulate index lookup + filtered scan
                    std::thread::sleep(Duration::from_secs_f64(index_overhead));
                    
                    let mut similarities = Vec::new();
                    for i in 0..filtered_size {
                        let candidate_vector = generate_random_vector(384);
                        let sim = proximadb::compute::distance::cosine_similarity(&query_vector, &candidate_vector);
                        similarities.push((i, sim));
                    }
                    
                    similarities.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                    similarities.into_iter().take(10).collect::<Vec<_>>()
                }
            );
            
            // Verify results
            assert_eq!(memtable_results.len(), 10);
            assert_eq!(viper_results.len(), 10);
            
            // Calculate performance improvement
            let speedup = memtable_measurement.duration_ms / viper_measurement.duration_ms;
            
            println!("📊 Dataset size {}: VIPER speedup = {:.1}x", dataset_size, speedup);
            
            // VIPER should be significantly faster for larger datasets
            if dataset_size >= 10000 {
                assert!(speedup > 2.0);
            }
        }
        
        println!("✅ Search performance simulation test completed");
        Ok(())
    }

    #[tokio::test]
    async fn test_concurrent_operations_performance() -> Result<()> {
        init_test_env();
        
        let collection_id = generate_test_collection_name();
        let num_threads = 4;
        let operations_per_thread = 100;
        
        // Test concurrent vector creation
        let start = Instant::now();
        let handles: Vec<_> = (0..num_threads)
            .map(|thread_id| {
                let collection_id = collection_id.clone();
                tokio::spawn(async move {
                    let mut results = Vec::new();
                    for i in 0..operations_per_thread {
                        let vector_id = format!("thread_{}_vector_{}", thread_id, i);
                        let vector_record = create_test_vector_record(
                            vector_id,
                            collection_id.clone(),
                            384
                        );
                        results.push(vector_record);
                    }
                    results
                })
            })
            .collect();
        
        // Wait for all threads to complete
        let mut all_results = Vec::new();
        for handle in handles {
            let thread_results = handle.await.unwrap();
            all_results.extend(thread_results);
        }
        
        let duration = start.elapsed();
        let total_operations = num_threads * operations_per_thread;
        let throughput = total_operations as f64 / duration.as_secs_f64();
        
        assert_eq!(all_results.len(), total_operations);
        
        println!("📊 Concurrent operations:");
        println!("   Threads: {}", num_threads);
        println!("   Total operations: {}", total_operations);
        println!("   Duration: {:.2}ms", duration.as_secs_f64() * 1000.0);
        println!("   Throughput: {:.1} ops/sec", throughput);
        
        // Should achieve reasonable concurrent throughput
        assert!(throughput > 1000.0);
        
        println!("✅ Concurrent operations performance test completed");
        Ok(())
    }

    #[tokio::test]
    async fn test_memory_usage_estimation() -> Result<()> {
        init_test_env();
        
        let vector_counts = vec![1000, 5000, 10000];
        let dimension = 384;
        
        for count in vector_counts {
            let vectors = create_test_vector_batch(
                generate_test_collection_name(),
                count,
                dimension
            );
            
            // Estimate memory usage
            let vector_size_bytes = dimension * 4; // 4 bytes per f32
            let metadata_size_estimate = 500; // Rough estimate per record
            let record_size_estimate = vector_size_bytes + metadata_size_estimate;
            let total_size_mb = (count * record_size_estimate) as f64 / 1024.0 / 1024.0;
            
            println!("📊 Memory estimation for {} vectors:", count);
            println!("   Vector size: {} bytes", vector_size_bytes);
            println!("   Estimated record size: {} bytes", record_size_estimate);
            println!("   Total estimated size: {:.2} MB", total_size_mb);
            
            // Verify reasonable memory usage
            assert!(total_size_mb < 1000.0); // Should be under 1GB for test data
            assert_eq!(vectors.len(), count);
            
            // Verify vector dimensions
            for vector in &vectors {
                assert_eq!(vector.vector.len(), dimension);
            }
        }
        
        println!("✅ Memory usage estimation test completed");
        Ok(())
    }
}