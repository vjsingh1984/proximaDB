#!/usr/bin/env python3
"""
ProximaDB SDK Performance Benchmarks

Comprehensive performance testing moved from pytest suite to keep tests fast.
Run this separately for performance analysis.
"""

import time
import numpy as np
from typing import List, Dict, Any

from proximadb import ProximaDBClient, Protocol
from proximadb.models import CollectionConfig, VectorRecord, DistanceMetric, StorageEngine
from proximadb import proximadb_pb2 as pb2


def benchmark_upsert_performance():
    """Comprehensive upsert performance comparison"""
    print("\n🚀 SDK Upsert Performance Benchmark")
    print("=" * 60)
    
    # Initialize clients
    rest_client = ProximaDBClient(force_protocol=Protocol.REST)
    grpc_client = ProximaDBClient(force_protocol=Protocol.GRPC)
    
    # Create test collections
    rest_collection = rest_client.create_collection(
        f"perf_rest_{int(time.time())}", 
        CollectionConfig(dimension=128, storage_engine=StorageEngine.VIPER)
    )
    
    grpc_collection = grpc_client.create_collection(
        f"perf_grpc_{int(time.time())}", 
        CollectionConfig(dimension=128, storage_engine=StorageEngine.VIPER)
    )
    
    # Test different batch sizes
    batch_sizes = [10, 50, 100, 500, 1000]
    
    print(f"{'Batch Size':<12} {'REST (s)':<12} {'gRPC (s)':<12} {'Speedup':<12}")
    print("-" * 60)
    
    for batch_size in batch_sizes:
        # Prepare test data
        vectors = []
        for i in range(batch_size):
            vectors.append(VectorRecord(
                id=f"perf_{i:06d}",
                vector=np.random.rand(128).tolist(),
                metadata={"batch": batch_size, "index": i, "type": "benchmark"}
            ))
        
        # Test REST performance
        start_time = time.time()
        rest_result = rest_client.insert_vectors(rest_collection.config.name, records=vectors)
        rest_duration = time.time() - start_time
        
        # Test gRPC performance
        start_time = time.time()
        grpc_result = grpc_client.insert_vectors(grpc_collection.config.name, records=vectors)
        grpc_duration = time.time() - start_time
        
        # Calculate speedup
        speedup = rest_duration / grpc_duration if grpc_duration > 0 else 0
        
        print(f"{batch_size:<12} {rest_duration:<12.3f} {grpc_duration:<12.3f} {speedup:<12.2f}x")
        
        # Verify results
        assert rest_result.success, f"REST failed for batch size {batch_size}"
        assert grpc_result.success, f"gRPC failed for batch size {batch_size}"
    
    # Cleanup
    rest_client.delete_collection(rest_collection.config.name)
    grpc_client.delete_collection(grpc_collection.config.name)
    rest_client.close()
    grpc_client.close()


def benchmark_proto_serialization():
    """Proto serialization performance benchmark"""
    print("\n🚀 Proto Serialization Performance")
    print("=" * 60)
    
    # Create test data
    num_vectors = 1000
    dimension = 256
    
    # Time proto serialization
    start_time = time.time()
    
    batch = pb2.VectorBatchRequest()
    batch.collection_id = "perf_test"
    
    for i in range(num_vectors):
        record = batch.vectors.add()
        record.id = f"perf_{i:06d}"
        record.vector.extend(np.random.rand(dimension).tolist())
        
        # Add metadata items
        index_item = record.metadata.add()
        index_item.key = "index"
        index_item.value.string_value = str(i)
        
        category_item = record.metadata.add()  
        category_item.key = "category"
        category_item.value.string_value = f"category_{i % 10}"
        
        score_item = record.metadata.add()
        score_item.key = "score"
        score_item.value.double_value = np.random.rand()
    
    proto_time = time.time() - start_time
    
    # Serialize to bytes
    start_time = time.time()
    serialized_data = batch.SerializeToString()
    serialize_time = time.time() - start_time
    
    # Deserialize
    start_time = time.time()
    new_batch = pb2.VectorBatchRequest()
    new_batch.ParseFromString(serialized_data)
    deserialize_time = time.time() - start_time
    
    # Calculate statistics
    data_size_mb = len(serialized_data) / (1024 * 1024)
    vectors_per_second = num_vectors / proto_time
    
    print(f"Vectors: {num_vectors:,}")
    print(f"Dimension: {dimension}")
    print(f"Proto Creation: {proto_time:.3f}s ({vectors_per_second:.0f} vectors/s)")
    print(f"Serialization: {serialize_time:.3f}s")
    print(f"Deserialization: {deserialize_time:.3f}s")
    print(f"Data Size: {data_size_mb:.2f} MB")
    print(f"Throughput: {data_size_mb/proto_time:.2f} MB/s")


def benchmark_search_performance():
    """Search performance benchmark"""
    print("\n🚀 Search Performance Benchmark")
    print("=" * 60)
    
    client = ProximaDBClient(force_protocol=Protocol.GRPC)
    
    # Create collection with larger dataset
    collection = client.create_collection(
        f"search_perf_{int(time.time())}", 
        CollectionConfig(dimension=128, storage_engine=StorageEngine.VIPER)
    )
    
    # Insert baseline data
    print("Inserting baseline data...")
    vectors = []
    for i in range(5000):
        vectors.append(VectorRecord(
            id=f"search_{i:06d}",
            vector=np.random.rand(128).tolist(),
            metadata={"category": f"cat_{i % 20}", "value": i}
        ))
    
    # Insert in batches to avoid timeouts
    batch_size = 500
    for i in range(0, len(vectors), batch_size):
        batch = vectors[i:i + batch_size]
        result = client.insert_vectors(collection.config.name, records=batch)
        assert result.success, f"Failed to insert batch {i//batch_size}"
    
    print(f"Inserted {len(vectors)} vectors")
    
    # Wait for indexing
    time.sleep(2)
    
    # Test search performance with different k values
    query_vector = np.random.rand(128).tolist()
    k_values = [1, 10, 50, 100, 500]
    
    print(f"\n{'k':<8} {'Time (s)':<12} {'Results':<12} {'QPS':<12}")
    print("-" * 50)
    
    for k in k_values:
        start_time = time.time()
        results = client.search(
            collection_id=collection.config.name,
            vector=query_vector,
            k=k,
            include_metadata=True
        )
        search_time = time.time() - start_time
        qps = 1 / search_time if search_time > 0 else 0
        
        print(f"{k:<8} {search_time:<12.4f} {len(results):<12} {qps:<12.1f}")
    
    # Cleanup
    client.delete_collection(collection.config.name)
    client.close()


def benchmark_concurrent_performance():
    """Concurrent operations benchmark"""
    print("\n🚀 Concurrent Operations Benchmark")
    print("=" * 60)
    
    from concurrent.futures import ThreadPoolExecutor, as_completed
    
    client = ProximaDBClient(force_protocol=Protocol.GRPC)
    
    # Create test collection
    collection = client.create_collection(
        f"concurrent_perf_{int(time.time())}", 
        CollectionConfig(dimension=64, storage_engine=StorageEngine.VIPER)
    )
    
    def concurrent_worker(worker_id: int, operations: int) -> Dict[str, Any]:
        """Worker function for concurrent operations"""
        start_time = time.time()
        success_count = 0
        
        for i in range(operations):
            try:
                vector_id = f"worker_{worker_id}_vec_{i}"
                vector = np.random.rand(64).tolist()
                metadata = {"worker": worker_id, "operation": i}
                
                result = client.insert_vector(
                    collection_id=collection.config.name,
                    vector_id=vector_id,
                    vector=vector,
                    metadata=metadata
                )
                
                if result.success:
                    success_count += 1
                    
            except Exception as e:
                print(f"Worker {worker_id} error: {e}")
        
        duration = time.time() - start_time
        return {
            "worker_id": worker_id,
            "operations": operations,
            "success_count": success_count,
            "duration": duration,
            "ops_per_second": success_count / duration if duration > 0 else 0
        }
    
    # Test different concurrency levels
    concurrency_levels = [1, 2, 4, 8]
    operations_per_worker = 50
    
    print(f"{'Workers':<10} {'Total Ops':<12} {'Success':<12} {'Duration':<12} {'Total OPS':<12}")
    print("-" * 70)
    
    for num_workers in concurrency_levels:
        start_time = time.time()
        
        with ThreadPoolExecutor(max_workers=num_workers) as executor:
            futures = [
                executor.submit(concurrent_worker, i, operations_per_worker) 
                for i in range(num_workers)
            ]
            results = [f.result() for f in as_completed(futures)]
        
        total_duration = time.time() - start_time
        total_operations = sum(r["operations"] for r in results)
        total_success = sum(r["success_count"] for r in results)
        total_ops_per_second = total_success / total_duration if total_duration > 0 else 0
        
        print(f"{num_workers:<10} {total_operations:<12} {total_success:<12} {total_duration:<12.3f} {total_ops_per_second:<12.1f}")
    
    # Cleanup
    client.delete_collection(collection.config.name)
    client.close()


if __name__ == "__main__":
    print("🎯 ProximaDB SDK Performance Benchmarks")
    print("=" * 80)
    
    try:
        benchmark_upsert_performance()
        benchmark_proto_serialization()
        benchmark_search_performance()
        benchmark_concurrent_performance()
        
        print("\n✅ All benchmarks completed successfully!")
        
    except Exception as e:
        print(f"\n❌ Benchmark failed: {e}")
        import traceback
        traceback.print_exc()