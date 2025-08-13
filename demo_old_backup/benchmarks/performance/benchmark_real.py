#!/usr/bin/env python3
"""Real performance benchmarks for ProximaDB"""

import time
import numpy as np
from proximadb import connect_rest, connect_grpc
from proximadb.models import CollectionConfig, VectorRecord, StorageEngine
import statistics
import concurrent.futures

def benchmark_distance_computation():
    """Benchmark real distance computation performance"""
    print("\n=== Distance Computation Benchmarks (Real) ===")
    
    client = connect_rest("http://localhost:5678")
    collection_id = f"benchmark_{int(time.time())}"
    
    # Test different dimensions
    dimensions = [128, 256, 512, 1024]
    results = {}
    
    for dim in dimensions:
        # Create collection
        config = CollectionConfig(
            name=collection_id,
            dimension=dim,
            storage_engine=StorageEngine.LSM
        )
        collection = client.create_collection(collection_id, config)
        
        # Generate test vectors
        num_vectors = 1000
        vectors = []
        for i in range(num_vectors):
            vec = VectorRecord(
                id=f"vec_{i}",
                vector=np.random.rand(dim).tolist(),
                metadata={"index": i}
            )
            vectors.append(vec)
        
        # Benchmark insert
        start = time.time()
        client.insert_vectors(collection_id, vectors)
        insert_time = time.time() - start
        insert_ops = num_vectors / insert_time
        
        # Benchmark search
        query = np.random.rand(dim).tolist()
        search_times = []
        
        for _ in range(100):
            start = time.time()
            results = client.search(collection_id, query, top_k=10)
            search_times.append(time.time() - start)
        
        avg_search_time = statistics.mean(search_times)
        search_qps = 1.0 / avg_search_time
        
        print(f"\nDimension {dim}:")
        print(f"  Insert: {insert_ops:.0f} vectors/sec")
        print(f"  Search: {search_qps:.0f} QPS")
        print(f"  Search latency: {avg_search_time*1000:.2f} ms")
        
        # Cleanup
        client.delete_collection(collection_id)
        collection_id = f"benchmark_{int(time.time())}"
    
    client.close()

def benchmark_concurrent_operations():
    """Benchmark concurrent operation performance"""
    print("\n=== Concurrent Operation Benchmarks (Real) ===")
    
    collection_id = f"concurrent_bench_{int(time.time())}"
    
    # Create collection
    client = connect_rest("http://localhost:5678")
    config = CollectionConfig(
        name=collection_id,
        dimension=128,
        storage_engine=StorageEngine.VIPER
    )
    client.create_collection(collection_id, config)
    
    # Prepare data
    vectors = []
    for i in range(10000):
        vec = VectorRecord(
            id=f"vec_{i}",
            vector=np.random.rand(128).tolist(),
            metadata={"batch": i // 100}
        )
        vectors.append(vec)
    
    # Insert vectors
    client.insert_vectors(collection_id, vectors)
    
    def search_operation():
        """Single search operation"""
        query = np.random.rand(128).tolist()
        start = time.time()
        results = client.search(collection_id, query, top_k=10)
        return time.time() - start
    
    # Test different concurrent loads
    thread_counts = [1, 10, 50, 100]
    
    for num_threads in thread_counts:
        latencies = []
        
        with concurrent.futures.ThreadPoolExecutor(max_workers=num_threads) as executor:
            start = time.time()
            futures = [executor.submit(search_operation) for _ in range(1000)]
            
            for future in concurrent.futures.as_completed(futures):
                latencies.append(future.result())
            
            total_time = time.time() - start
        
        qps = len(futures) / total_time
        p50 = statistics.median(latencies) * 1000
        p99 = np.percentile(latencies, 99) * 1000
        
        print(f"\n{num_threads} concurrent clients:")
        print(f"  QPS: {qps:.0f}")
        print(f"  p50 latency: {p50:.2f} ms")
        print(f"  p99 latency: {p99:.2f} ms")
    
    # Cleanup
    client.delete_collection(collection_id)
    client.close()

def benchmark_storage_engines():
    """Compare LSM vs VIPER performance"""
    print("\n=== Storage Engine Benchmarks (Real) ===")
    
    client = connect_rest("http://localhost:5678")
    
    for engine in [StorageEngine.LSM, StorageEngine.VIPER]:
        print(f"\n{engine.value} Engine:")
        
        collection_id = f"storage_bench_{engine.value}_{int(time.time())}"
        config = CollectionConfig(
            name=collection_id,
            dimension=256,
            storage_engine=engine
        )
        client.create_collection(collection_id, config)
        
        # Prepare batch data
        batch_size = 1000
        vectors = []
        for i in range(batch_size):
            vec = VectorRecord(
                id=f"vec_{i}",
                vector=np.random.rand(256).tolist(),
                metadata={"type": "benchmark", "index": i}
            )
            vectors.append(vec)
        
        # Benchmark batch insert
        start = time.time()
        client.insert_vectors(collection_id, vectors)
        insert_time = time.time() - start
        insert_rate = batch_size / insert_time
        
        # Benchmark point queries
        query_times = []
        for _ in range(100):
            query = np.random.rand(256).tolist()
            start = time.time()
            results = client.search(collection_id, query, top_k=1)
            query_times.append(time.time() - start)
        
        avg_query_time = statistics.mean(query_times)
        query_qps = 1.0 / avg_query_time
        
        print(f"  Batch insert: {insert_rate:.0f} vectors/sec")
        print(f"  Point query: {query_qps:.0f} QPS")
        print(f"  Query latency: {avg_query_time*1000:.2f} ms")
        
        # Cleanup
        client.delete_collection(collection_id)
    
    client.close()

if __name__ == "__main__":
    print("ProximaDB Real Performance Benchmarks")
    print("=====================================")
    print("Note: Ensure ProximaDB server is running on port 5678")
    
    try:
        benchmark_distance_computation()
        benchmark_concurrent_operations()
        benchmark_storage_engines()
        print("\n✅ All benchmarks completed!")
    except Exception as e:
        print(f"\n❌ Benchmark failed: {e}")
        print("Make sure ProximaDB server is running:")