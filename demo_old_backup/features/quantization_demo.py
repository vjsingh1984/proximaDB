#!/usr/bin/env python3
"""
ProximaDB Quantization Demo

This example demonstrates how to use quantization and search optimization
features in ProximaDB for improved performance.
"""

import time
import numpy as np
from typing import List, Dict, Any

# Import ProximaDB client
from proximadb import connect_rest
from proximadb.models import (
    CollectionConfig, 
    DistanceMetric,
    QuantizationConfig,
    QuantizationType
)


def create_random_vectors(num_vectors: int, dimension: int) -> List[np.ndarray]:
    """Generate random normalized vectors for testing"""
    vectors = []
    for _ in range(num_vectors):
        vec = np.random.randn(dimension).astype(np.float32)
        # Normalize to unit length
        vec = vec / np.linalg.norm(vec)
        vectors.append(vec)
    return vectors


def benchmark_search(client, collection_name: str, query_vector: np.ndarray, 
                    optimization_hints: Dict[str, Any] = None) -> tuple:
    """Benchmark search with given optimization hints"""
    start_time = time.time()
    
    results = client.search(
        collection_name,
        query=query_vector.tolist(),
        k=10,
        include_metadata=True,
        optimization_hints=optimization_hints
    )
    
    search_time = time.time() - start_time
    return results, search_time


def main():
    print("ProximaDB Quantization Demo")
    print("=" * 50)
    
    # Connect to ProximaDB
    client = connect_rest("http://localhost:5678")
    
    # Configuration
    dimension = 768  # Typical BERT embedding dimension
    num_vectors = 10000
    
    # Create collections with different quantization settings
    collections = [
        {
            "name": "no_quantization",
            "config": CollectionConfig(
                dimension=dimension,
                distance_metric=DistanceMetric.COSINE
            ),
            "description": "Baseline - No quantization (FP32)"
        },
        {
            "name": "scalar_quantization",
            "config": CollectionConfig(
                dimension=dimension,
                distance_metric=DistanceMetric.COSINE,
                quantization_config=QuantizationConfig(
                    enabled=True,
                    type=QuantizationType.SCALAR,
                    bits_per_vector=8,
                    accuracy_threshold=0.99
                )
            ),
            "description": "Scalar INT8 quantization"
        },
        {
            "name": "product_quantization",
            "config": CollectionConfig(
                dimension=dimension,
                distance_metric=DistanceMetric.COSINE,
                quantization_config=QuantizationConfig(
                    enabled=True,
                    type=QuantizationType.PRODUCT,
                    bits_per_subvector=8,
                    num_subvectors=96,  # 768/8 = 96
                    compression_ratio_target=8.0,
                    accuracy_threshold=0.95
                )
            ),
            "description": "Product quantization (PQ8)"
        },
        {
            "name": "binary_quantization",
            "config": CollectionConfig(
                dimension=dimension,
                distance_metric=DistanceMetric.COSINE,
                quantization_config=QuantizationConfig(
                    enabled=True,
                    type=QuantizationType.BINARY,
                    compression_ratio_target=32.0,
                    accuracy_threshold=0.85
                )
            ),
            "description": "Binary quantization (1-bit)"
        }
    ]
    
    # Create collections and insert data
    print("\n1. Creating collections and inserting vectors...")
    
    # Generate random vectors
    vectors = create_random_vectors(num_vectors, dimension)
    
    for col_info in collections:
        print(f"\n   Creating {col_info['name']} - {col_info['description']}")
        
        try:
            # Create collection
            client.create_collection(col_info['name'], col_info['config'])
            
            # Insert vectors in batches
            batch_size = 1000
            insert_start = time.time()
            
            for i in range(0, num_vectors, batch_size):
                batch = vectors[i:i+batch_size]
                ids = [f"vec_{j}" for j in range(i, min(i+batch_size, num_vectors))]
                metadata = [{"index": j, "batch": i//batch_size} for j in range(i, min(i+batch_size, num_vectors))]
                
                client.insert_batch(
                    col_info['name'],
                    vectors=batch,
                    ids=ids,
                    metadata=metadata
                )
            
            insert_time = time.time() - insert_start
            print(f"   ✓ Inserted {num_vectors} vectors in {insert_time:.2f}s")
            
        except Exception as e:
            print(f"   ✗ Error: {e}")
    
    # Wait for indexing
    print("\n2. Waiting for indexing to complete...")
    time.sleep(2)
    
    # Benchmark searches
    print("\n3. Benchmarking search performance...")
    
    # Generate query vector
    query_vector = create_random_vectors(1, dimension)[0]
    
    # Test different optimization strategies
    optimization_strategies = [
        {
            "name": "No optimization",
            "hints": None
        },
        {
            "name": "Basic two-stage search",
            "hints": {
                "enable_two_stage_search": True,
                "quantization_hint": "FP32",
                "candidate_multiplier": 3.0
            }
        },
        {
            "name": "INT8 two-stage search",
            "hints": {
                "enable_two_stage_search": True,
                "quantization_hint": "INT8",
                "candidate_multiplier": 3.0,
                "enable_parallel_search": True
            }
        },
        {
            "name": "PQ8 aggressive optimization",
            "hints": {
                "enable_two_stage_search": True,
                "quantization_hint": "PQ8",
                "candidate_multiplier": 5.0,
                "min_candidates": 50,
                "max_candidates": 500,
                "enable_clustering_optimization": True,
                "enable_parallel_search": True
            }
        },
        {
            "name": "Binary ultra-fast search",
            "hints": {
                "enable_two_stage_search": True,
                "quantization_hint": "BINARY",
                "candidate_multiplier": 10.0,
                "timeout_ms": 50,
                "custom_hints": {
                    "use_simd": "true",
                    "prefetch_size": "128"
                }
            }
        }
    ]
    
    print("\nSearch Results:")
    print("-" * 80)
    print(f"{'Collection':<25} {'Strategy':<30} {'Time (ms)':<12} {'Top Score':<10}")
    print("-" * 80)
    
    for col_info in collections:
        for strategy in optimization_strategies:
            try:
                results, search_time = benchmark_search(
                    client, 
                    col_info['name'],
                    query_vector,
                    strategy['hints']
                )
                
                top_score = results[0].score if results else 0.0
                
                print(f"{col_info['name']:<25} {strategy['name']:<30} "
                      f"{search_time*1000:>10.2f} {top_score:>10.4f}")
                
            except Exception as e:
                print(f"{col_info['name']:<25} {strategy['name']:<30} "
                      f"{'ERROR':>10} {str(e)[:10]}")
    
    # Compare accuracy
    print("\n4. Accuracy comparison...")
    
    # Get baseline results (no quantization, no optimization)
    baseline_results, _ = benchmark_search(client, "no_quantization", query_vector, None)
    baseline_ids = [r.id for r in baseline_results[:10]]
    
    print(f"\nBaseline top-10 IDs: {baseline_ids}")
    
    # Compare other configurations
    for col_info in collections[1:]:  # Skip baseline
        results, _ = benchmark_search(
            client, 
            col_info['name'],
            query_vector,
            {
                "enable_two_stage_search": True,
                "quantization_hint": "FP32",  # Use full precision for final stage
                "candidate_multiplier": 3.0
            }
        )
        
        result_ids = [r.id for r in results[:10]]
        overlap = len(set(baseline_ids) & set(result_ids))
        recall = overlap / 10.0
        
        print(f"\n{col_info['name']}:")
        print(f"  Recall@10: {recall*100:.1f}%")
        print(f"  Top result: {results[0].id if results else 'None'} "
              f"(score: {results[0].score:.4f if results else 0})")
    
    # Cleanup
    print("\n5. Cleaning up...")
    for col_info in collections:
        try:
            client.delete_collection(col_info['name'])
            print(f"   ✓ Deleted {col_info['name']}")
        except:
            pass
    
    print("\nDemo complete!")


if __name__ == "__main__":
    main()