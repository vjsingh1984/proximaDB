#!/usr/bin/env python3
"""
Large Dataset Performance Test for ProximaDB
Tests with 100,000 vectors using batch size of 512
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import json
import numpy as np
from typing import List, Dict, Any, Tuple
from proximadb import connect_rest, connect_grpc, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord
from sklearn.metrics.pairwise import cosine_similarity

# Test configuration
DIMENSION = 128
NUM_VECTORS = 100000  # 100K vectors
BATCH_SIZE = 512     # Optimal batch size
NUM_QUERIES = 100
TOP_K = 100

def generate_large_dataset(num_vectors: int, dimension: int) -> Tuple[List[VectorRecord], List[np.ndarray], np.ndarray]:
    """Generate large dataset with realistic distribution"""
    print(f"📊 Generating {num_vectors:,} vectors ({dimension}D)...")
    
    vectors = []
    raw_vectors = []
    
    # Generate in chunks to avoid memory issues
    chunk_size = 10000
    num_chunks = num_vectors // chunk_size
    
    for chunk_idx in range(num_chunks):
        chunk_start = chunk_idx * chunk_size
        chunk_end = min(chunk_start + chunk_size, num_vectors)
        
        # Generate chunk of vectors
        chunk_vectors = np.random.randn(chunk_end - chunk_start, dimension).astype(np.float32)
        # Normalize
        norms = np.linalg.norm(chunk_vectors, axis=1, keepdims=True)
        chunk_vectors = chunk_vectors / norms
        
        for i, vec_data in enumerate(chunk_vectors):
            idx = chunk_start + i
            raw_vectors.append(vec_data)
            
            vec = VectorRecord(
                id=f"vec_{idx}",
                vector=vec_data.tolist(),
                metadata={
                    "index": idx,
                    "category": f"cat_{idx % 100}",
                    "chunk": chunk_idx
                }
            )
            vectors.append(vec)
        
        if (chunk_idx + 1) % 5 == 0:
            print(f"  Generated {(chunk_idx + 1) * chunk_size:,} vectors...")
    
    # Generate query vectors
    queries = []
    for i in range(NUM_QUERIES):
        query = np.random.randn(dimension).astype(np.float32)
        query = query / np.linalg.norm(query)
        queries.append(query)
    
    raw_vectors_array = np.array(raw_vectors)
    print(f"✅ Dataset generation complete: {len(vectors):,} vectors")
    
    return vectors, queries, raw_vectors_array

def test_protocol_performance(
    protocol: str,
    engine: str,
    vectors: List[VectorRecord],
    queries: List[np.ndarray]
) -> Dict[str, Any]:
    """Test performance with large dataset"""
    
    print(f"\n{'='*80}")
    print(f"Testing: {protocol.upper()} + {engine.upper()} Engine")
    print(f"Dataset: {NUM_VECTORS:,} vectors, Batch size: {BATCH_SIZE}")
    print(f"{'='*80}")
    
    results = {
        "protocol": protocol,
        "engine": engine,
        "dataset_size": NUM_VECTORS,
        "batch_size": BATCH_SIZE,
        "metrics": {}
    }
    
    # Connect to server
    if protocol == "rest":
        client = connect_rest("http://localhost:5678")
    else:
        client = connect_grpc("http://localhost:5679")
    
    # Create collection
    collection_name = f"large_{protocol}_{engine}_{int(time.time())}"
    config = CollectionConfig(
        name=collection_name,
        dimension=DIMENSION,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER if engine == "viper" else StorageEngine.LSM,
        description=f"Large dataset test: {NUM_VECTORS} vectors"
    )
    
    print(f"\n📦 Creating collection...")
    start = time.time()
    collection = client.create_collection(collection_name, config)
    create_time = (time.time() - start) * 1000
    results["metrics"]["create_collection_ms"] = create_time
    print(f"✅ Collection created: {create_time:.2f}ms")
    
    # Insert vectors in batches
    print(f"\n📝 Inserting {NUM_VECTORS:,} vectors in batches of {BATCH_SIZE}...")
    insert_times = []
    total_start = time.time()
    
    num_batches = (NUM_VECTORS + BATCH_SIZE - 1) // BATCH_SIZE
    
    for i in range(0, NUM_VECTORS, BATCH_SIZE):
        batch_num = i // BATCH_SIZE
        batch = vectors[i:i+BATCH_SIZE]
        
        batch_start = time.time()
        client.insert_vectors(collection_name, batch)
        batch_time = (time.time() - batch_start) * 1000
        insert_times.append(batch_time)
        
        if (batch_num + 1) % 50 == 0:
            avg_time = sum(insert_times[-50:]) / len(insert_times[-50:])
            rate = (BATCH_SIZE / avg_time) * 1000
            print(f"  Progress: {i+BATCH_SIZE:,}/{NUM_VECTORS:,} vectors ({rate:.0f} vec/s)")
    
    total_insert_time = (time.time() - total_start) * 1000
    avg_batch_time = sum(insert_times) / len(insert_times)
    insert_rate = (NUM_VECTORS / total_insert_time) * 1000
    
    results["metrics"]["insert"] = {
        "total_vectors": NUM_VECTORS,
        "batch_size": BATCH_SIZE,
        "num_batches": num_batches,
        "total_time_ms": total_insert_time,
        "avg_batch_ms": avg_batch_time,
        "vectors_per_second": insert_rate,
        "batches_per_second": 1000 / avg_batch_time
    }
    
    print(f"✅ Insert complete:")
    print(f"   - Total time: {total_insert_time/1000:.2f}s")
    print(f"   - Insert rate: {insert_rate:,.0f} vectors/sec")
    print(f"   - Avg batch time: {avg_batch_time:.2f}ms")
    
    # Wait for data to stabilize
    if engine == "viper":
        print("⏳ Waiting for VIPER flush...")
        time.sleep(5)
    
    # Test search performance
    print(f"\n🔍 Testing search performance (top-{TOP_K})...")
    search_times = []
    
    # Warm-up queries
    for _ in range(5):
        client.search(collection_name, queries[0].tolist(), top_k=TOP_K)
    
    # Actual search tests
    for i, query in enumerate(queries[:50]):  # Test 50 queries
        start = time.time()
        results_list = client.search(collection_name, query.tolist(), top_k=TOP_K)
        search_time = (time.time() - start) * 1000
        search_times.append(search_time)
        
        if (i + 1) % 10 == 0:
            avg_search = sum(search_times[-10:]) / 10
            print(f"  Progress: {i+1}/50 queries (avg: {avg_search:.2f}ms)")
    
    # Calculate percentiles
    search_times_sorted = sorted(search_times)
    p50 = search_times_sorted[len(search_times_sorted) // 2]
    p95 = search_times_sorted[int(len(search_times_sorted) * 0.95)]
    p99 = search_times_sorted[int(len(search_times_sorted) * 0.99)]
    
    avg_search_time = sum(search_times) / len(search_times)
    
    results["metrics"]["search"] = {
        "dataset_size": NUM_VECTORS,
        "top_k": TOP_K,
        "queries_tested": len(search_times),
        "avg_latency_ms": avg_search_time,
        "p50_latency_ms": p50,
        "p95_latency_ms": p95,
        "p99_latency_ms": p99,
        "min_latency_ms": min(search_times),
        "max_latency_ms": max(search_times),
        "searches_per_second": 1000 / avg_search_time
    }
    
    print(f"✅ Search complete:")
    print(f"   - Avg latency: {avg_search_time:.2f}ms")
    print(f"   - P50 latency: {p50:.2f}ms")
    print(f"   - P95 latency: {p95:.2f}ms")
    print(f"   - P99 latency: {p99:.2f}ms")
    
    # Cleanup
    print("\n🧹 Cleaning up...")
    client.delete_collection(collection_name)
    
    return results

def run_comprehensive_tests():
    """Run all test combinations"""
    
    print("🚀 ProximaDB Large Dataset Performance Test")
    print(f"   Dataset size: {NUM_VECTORS:,} vectors")
    print(f"   Dimension: {DIMENSION}")
    print(f"   Batch size: {BATCH_SIZE}")
    
    # Generate dataset once
    vectors, queries, raw_vectors = generate_large_dataset(NUM_VECTORS, DIMENSION)
    
    all_results = []
    
    # Test configurations
    test_configs = [
        ("rest", "viper"),
        ("rest", "lsm"),
        ("grpc", "viper"),
        ("grpc", "lsm"),
    ]
    
    for protocol, engine in test_configs:
        try:
            result = test_protocol_performance(protocol, engine, vectors, queries)
            all_results.append(result)
        except Exception as e:
            print(f"❌ Error testing {protocol}/{engine}: {e}")
            import traceback
            traceback.print_exc()
    
    # Save results
    results_data = {
        "test_config": {
            "dataset_size": NUM_VECTORS,
            "dimension": DIMENSION,
            "batch_size": BATCH_SIZE,
            "num_queries": len(queries),
            "top_k": TOP_K
        },
        "results": all_results
    }
    
    with open("large_dataset_results.json", "w") as f:
        json.dump(results_data, f, indent=2)
    
    # Print summary
    print_performance_summary(all_results)

def print_performance_summary(results: List[Dict[str, Any]]):
    """Print formatted summary"""
    
    print("\n" + "="*100)
    print("LARGE DATASET PERFORMANCE SUMMARY")
    print(f"Dataset: {NUM_VECTORS:,} vectors ({DIMENSION}D), Batch size: {BATCH_SIZE}")
    print("="*100)
    
    # Insert performance
    print("\n📝 INSERT PERFORMANCE:")
    print("-"*80)
    print(f"{'Protocol':<10} {'Engine':<10} {'Total Time':<15} {'Insert Rate':<20} {'Batch Time':<15}")
    print("-"*80)
    
    for result in results:
        protocol = result["protocol"].upper()
        engine = result["engine"].upper()
        metrics = result["metrics"]["insert"]
        total_time = f"{metrics['total_time_ms']/1000:.1f}s"
        insert_rate = f"{metrics['vectors_per_second']:,.0f} vec/s"
        batch_time = f"{metrics['avg_batch_ms']:.2f}ms"
        
        print(f"{protocol:<10} {engine:<10} {total_time:<15} {insert_rate:<20} {batch_time:<15}")
    
    # Search performance
    print("\n🔍 SEARCH PERFORMANCE (top-100):")
    print("-"*80)
    print(f"{'Protocol':<10} {'Engine':<10} {'Avg (ms)':<10} {'P50 (ms)':<10} {'P95 (ms)':<10} {'P99 (ms)':<10} {'QPS':<10}")
    print("-"*80)
    
    for result in results:
        protocol = result["protocol"].upper()
        engine = result["engine"].upper()
        metrics = result["metrics"]["search"]
        
        print(f"{protocol:<10} {engine:<10} {metrics['avg_latency_ms']:<10.2f} "
              f"{metrics['p50_latency_ms']:<10.2f} {metrics['p95_latency_ms']:<10.2f} "
              f"{metrics['p99_latency_ms']:<10.2f} {metrics['searches_per_second']:<10.0f}")
    
    # Key insights
    print("\n📊 KEY INSIGHTS:")
    
    # Best insert performance
    best_insert = max(results, key=lambda x: x["metrics"]["insert"]["vectors_per_second"])
    print(f"\n  Fastest Insert: {best_insert['protocol'].upper()} + {best_insert['engine'].upper()}")
    print(f"    - Rate: {best_insert['metrics']['insert']['vectors_per_second']:,.0f} vectors/sec")
    print(f"    - Time for 100K vectors: {best_insert['metrics']['insert']['total_time_ms']/1000:.1f}s")
    
    # Best search performance
    best_search = min(results, key=lambda x: x["metrics"]["search"]["avg_latency_ms"])
    print(f"\n  Fastest Search: {best_search['protocol'].upper()} + {best_search['engine'].upper()}")
    print(f"    - Avg latency: {best_search['metrics']['search']['avg_latency_ms']:.2f}ms")
    print(f"    - P99 latency: {best_search['metrics']['search']['p99_latency_ms']:.2f}ms")
    
    # Protocol comparison
    rest_results = [r for r in results if r["protocol"] == "rest"]
    grpc_results = [r for r in results if r["protocol"] == "grpc"]
    
    if rest_results and grpc_results:
        rest_insert_avg = sum(r["metrics"]["insert"]["vectors_per_second"] for r in rest_results) / len(rest_results)
        grpc_insert_avg = sum(r["metrics"]["insert"]["vectors_per_second"] for r in grpc_results) / len(grpc_results)
        
        print(f"\n  Protocol Comparison:")
        print(f"    - gRPC is {grpc_insert_avg/rest_insert_avg:.1f}x faster for inserts")
        print(f"    - REST avg: {rest_insert_avg:,.0f} vec/s, gRPC avg: {grpc_insert_avg:,.0f} vec/s")

if __name__ == "__main__":
    run_comprehensive_tests()