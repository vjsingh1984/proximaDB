#!/usr/bin/env python3
"""
ProximaDB Performance Test Suite
Captures detailed performance metrics for documentation
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import json
import numpy as np
from typing import List, Dict, Any
from proximadb import connect_rest, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord

def time_operation(func, *args, **kwargs):
    """Time an operation and return result with timing"""
    start = time.time()
    result = func(*args, **kwargs)
    end = time.time()
    return result, (end - start) * 1000  # Convert to ms

def run_performance_tests():
    """Run comprehensive performance tests"""
    
    results = {
        "timestamp": time.time(),
        "operations": {}
    }
    
    # Connect to server
    print("🔗 Connecting to ProximaDB...")
    client = connect_rest("http://localhost:5678")
    
    # Test 1: Health Check
    print("\n📊 Test 1: Health Check Performance")
    _, health_time = time_operation(client.health)
    results["operations"]["health_check"] = {"time_ms": health_time}
    print(f"  ✅ Health check: {health_time:.2f}ms")
    
    # Test 2: List Collections (with 40+ collections)
    print("\n📊 Test 2: List Collections Performance")
    collections, list_time = time_operation(client.list_collections)
    results["operations"]["list_collections"] = {
        "time_ms": list_time,
        "collection_count": len(collections)
    }
    print(f"  ✅ List {len(collections)} collections: {list_time:.2f}ms")
    
    # Test 3: Create Collection
    print("\n📊 Test 3: Create Collection Performance")
    collection_name = f"perf_test_{int(time.time())}"
    config = CollectionConfig(
        name=collection_name,
        dimension=128,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER,
        description="Performance test collection"
    )
    
    collection, create_time = time_operation(client.create_collection, collection_name, config)
    results["operations"]["create_collection"] = {"time_ms": create_time}
    print(f"  ✅ Create collection: {create_time:.2f}ms")
    
    # Test 4: Batch Insert Performance (various sizes)
    print("\n📊 Test 4: Batch Insert Performance")
    batch_sizes = [1, 10, 100, 500, 1000]
    results["operations"]["batch_insert"] = {}
    
    for batch_size in batch_sizes:
        vectors = []
        for i in range(batch_size):
            vec = VectorRecord(
                id=f"vec_{batch_size}_{i}",
                vector=np.random.random(128).astype(np.float32).tolist(),
                metadata={"batch": batch_size, "index": i}
            )
            vectors.append(vec)
        
        result, insert_time = time_operation(client.insert_vectors, collection_name, vectors)
        results["operations"]["batch_insert"][f"batch_{batch_size}"] = {
            "time_ms": insert_time,
            "vectors_per_second": (batch_size / insert_time) * 1000 if insert_time > 0 else 0,
            "success": result.success,
            "failed": result.failed
        }
        print(f"  ✅ Insert {batch_size} vectors: {insert_time:.2f}ms ({(batch_size / insert_time) * 1000:.0f} vectors/sec)")
    
    # Test 5: Search Performance (various top_k)
    print("\n📊 Test 5: Search Performance")
    query_vector = np.random.random(128).astype(np.float32).tolist()
    top_k_values = [1, 10, 50, 100]
    results["operations"]["search"] = {}
    
    for k in top_k_values:
        search_results, search_time = time_operation(client.search, collection_name, query_vector, top_k=k)
        results["operations"]["search"][f"top_{k}"] = {
            "time_ms": search_time,
            "results_returned": len(search_results)
        }
        print(f"  ✅ Search top-{k}: {search_time:.2f}ms ({len(search_results)} results)")
    
    # Test 6: Get Collection Performance
    print("\n📊 Test 6: Get Collection Performance")
    _, get_time = time_operation(client.get_collection, collection_name)
    results["operations"]["get_collection"] = {"time_ms": get_time}
    print(f"  ✅ Get collection: {get_time:.2f}ms")
    
    # Test 7: Delete Collection Performance
    print("\n📊 Test 7: Delete Collection Performance")
    _, delete_time = time_operation(client.delete_collection, collection_name)
    results["operations"]["delete_collection"] = {"time_ms": delete_time}
    print(f"  ✅ Delete collection: {delete_time:.2f}ms")
    
    # Test 8: Large-scale Performance Test
    print("\n📊 Test 8: Large-scale Performance Test")
    large_collection = f"large_perf_test_{int(time.time())}"
    large_config = CollectionConfig(
        name=large_collection,
        dimension=512,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER,
        description="Large-scale performance test"
    )
    
    client.create_collection(large_collection, large_config)
    
    # Insert 10,000 vectors in batches of 100
    total_vectors = 10000
    batch_size = 100
    insert_times = []
    
    print(f"\n  Inserting {total_vectors} vectors in batches of {batch_size}...")
    start_total = time.time()
    
    for batch_num in range(total_vectors // batch_size):
        vectors = []
        for i in range(batch_size):
            idx = batch_num * batch_size + i
            vec = VectorRecord(
                id=f"large_vec_{idx}",
                vector=np.random.random(512).astype(np.float32).tolist(),
                metadata={"batch": batch_num, "index": idx}
            )
            vectors.append(vec)
        
        _, batch_time = time_operation(client.insert_vectors, large_collection, vectors)
        insert_times.append(batch_time)
        
        if batch_num % 10 == 0:
            print(f"    Progress: {(batch_num + 1) * batch_size}/{total_vectors} vectors")
    
    total_time = (time.time() - start_total) * 1000
    avg_batch_time = sum(insert_times) / len(insert_times)
    
    results["operations"]["large_scale_insert"] = {
        "total_vectors": total_vectors,
        "batch_size": batch_size,
        "total_time_ms": total_time,
        "avg_batch_time_ms": avg_batch_time,
        "vectors_per_second": (total_vectors / total_time) * 1000
    }
    
    print(f"  ✅ Inserted {total_vectors} vectors in {total_time:.2f}ms ({(total_vectors / total_time) * 1000:.0f} vectors/sec)")
    
    # Search on large dataset
    print("\n  Testing search on large dataset...")
    search_times = []
    for _ in range(10):
        query = np.random.random(512).astype(np.float32).tolist()
        _, search_time = time_operation(client.search, large_collection, query, top_k=100)
        search_times.append(search_time)
    
    avg_search_time = sum(search_times) / len(search_times)
    results["operations"]["large_scale_search"] = {
        "dataset_size": total_vectors,
        "top_k": 100,
        "avg_search_time_ms": avg_search_time,
        "searches_per_second": 1000 / avg_search_time
    }
    
    print(f"  ✅ Average search time (top-100): {avg_search_time:.2f}ms ({1000 / avg_search_time:.0f} searches/sec)")
    
    # Cleanup
    client.delete_collection(large_collection)
    
    # Save results
    with open("performance_results.json", "w") as f:
        json.dump(results, f, indent=2)
    
    print("\n📊 Performance test complete! Results saved to performance_results.json")
    
    return results

if __name__ == "__main__":
    run_performance_tests()