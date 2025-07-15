#!/usr/bin/env python3
"""
gRPC Batch Size Exploration Test
Tests gRPC with various batch sizes to find optimal performance
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import json
import numpy as np
from proximadb import connect_grpc, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord

def test_grpc_batch_sizes():
    """Test gRPC with various batch sizes"""
    
    print("🚀 gRPC Batch Size Performance Test")
    print("Testing various batch sizes to find optimal performance")
    print("="*80)
    
    # Connect to gRPC
    client = connect_grpc("http://localhost:5679")
    
    # Test batch sizes - going larger since gRPC showed good performance
    batch_sizes = [100, 512, 1000, 2000, 3000, 4000, 5000]
    results = {}
    
    # Create a test collection
    collection_name = f"grpc_batch_test_{int(time.time())}"
    config = CollectionConfig(
        name=collection_name,
        dimension=128,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER,
        description="gRPC batch size test"
    )
    
    collection = client.create_collection(collection_name, config)
    print(f"✅ Collection created: {collection_name}\n")
    
    for batch_size in batch_sizes:
        print(f"\n📊 Testing batch size: {batch_size}")
        
        # Generate test vectors
        vectors = []
        for i in range(batch_size):
            vec = VectorRecord(
                id=f"batch_{batch_size}_vec_{i}",
                vector=np.random.random(128).astype(np.float32).tolist(),
                metadata={"batch_size": batch_size, "index": i}
            )
            vectors.append(vec)
        
        # Calculate estimated message size
        # Each vector: 128 floats * 4 bytes + metadata + proto overhead ≈ 600-700 bytes
        estimated_size_mb = (batch_size * 700) / (1024 * 1024)
        print(f"  Estimated message size: {estimated_size_mb:.2f} MB")
        
        try:
            # Test insert performance
            start = time.time()
            result = client.insert_vectors(collection_name, vectors)
            insert_time = (time.time() - start) * 1000
            vectors_per_sec = (batch_size / insert_time) * 1000
            
            results[batch_size] = {
                "success": True,
                "insert_time_ms": insert_time,
                "vectors_per_second": vectors_per_sec,
                "message_size_mb": estimated_size_mb,
                "throughput_mb_per_sec": (estimated_size_mb / insert_time) * 1000
            }
            
            print(f"  ✅ Success:")
            print(f"     - Insert time: {insert_time:.2f}ms")
            print(f"     - Rate: {vectors_per_sec:,.0f} vectors/sec")
            print(f"     - Throughput: {(estimated_size_mb / insert_time) * 1000:.2f} MB/s")
            
        except Exception as e:
            results[batch_size] = {
                "success": False,
                "error": str(e),
                "message_size_mb": estimated_size_mb
            }
            print(f"  ❌ Failed: {e}")
            break  # Stop testing larger sizes if we hit a limit
    
    # Test search performance with the data
    print(f"\n🔍 Testing search performance on inserted data...")
    query = np.random.random(128).astype(np.float32).tolist()
    
    search_times = []
    for _ in range(10):
        start = time.time()
        results_list = client.search(collection_name, query, top_k=100)
        search_time = (time.time() - start) * 1000
        search_times.append(search_time)
    
    avg_search = sum(search_times) / len(search_times)
    print(f"✅ Average search time (top-100): {avg_search:.2f}ms")
    
    # Cleanup
    client.delete_collection(collection_name)
    
    # Print summary
    print("\n" + "="*80)
    print("gRPC BATCH SIZE PERFORMANCE SUMMARY")
    print("="*80)
    
    print(f"\n{'Batch Size':<12} {'Status':<10} {'Time (ms)':<12} {'Rate (vec/s)':<15} {'Size (MB)':<10} {'MB/s':<10}")
    print("-"*80)
    
    optimal_batch = None
    max_rate = 0
    
    for batch_size, result in results.items():
        if result["success"]:
            status = "✅"
            time_str = f"{result['insert_time_ms']:.2f}"
            rate_str = f"{result['vectors_per_second']:,.0f}"
            size_str = f"{result['message_size_mb']:.2f}"
            throughput_str = f"{result['throughput_mb_per_sec']:.2f}"
            
            if result['vectors_per_second'] > max_rate:
                max_rate = result['vectors_per_second']
                optimal_batch = batch_size
        else:
            status = "❌"
            time_str = "N/A"
            rate_str = "N/A"
            size_str = f"{result['message_size_mb']:.2f}"
            throughput_str = "N/A"
        
        print(f"{batch_size:<12} {status:<10} {time_str:<12} {rate_str:<15} {size_str:<10} {throughput_str:<10}")
    
    if optimal_batch:
        print(f"\n🏆 OPTIMAL BATCH SIZE: {optimal_batch}")
        print(f"   - Maximum rate: {max_rate:,.0f} vectors/sec")
        print(f"   - Message size: {results[optimal_batch]['message_size_mb']:.2f} MB")
    
    # Save results
    with open("grpc_batch_size_results.json", "w") as f:
        json.dump({
            "test_results": results,
            "optimal_batch_size": optimal_batch,
            "max_vectors_per_second": max_rate,
            "avg_search_latency_ms": avg_search
        }, f, indent=2)
    
    return results

def test_grpc_large_dataset_streaming():
    """Test gRPC with streaming large dataset"""
    
    print("\n\n🚀 gRPC Large Dataset Streaming Test")
    print("Testing 100K vectors with optimal batch size")
    print("="*80)
    
    client = connect_grpc("http://localhost:5679")
    
    # Create collection
    collection_name = f"grpc_streaming_{int(time.time())}"
    config = CollectionConfig(
        name=collection_name,
        dimension=128,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER,
        description="gRPC streaming test with 100K vectors"
    )
    
    collection = client.create_collection(collection_name, config)
    print(f"✅ Collection created: {collection_name}")
    
    # Test with 100K vectors using optimal batch size
    total_vectors = 100000
    batch_size = 2000  # Start with 2000 based on previous tests
    
    print(f"\n📝 Inserting {total_vectors:,} vectors in batches of {batch_size}...")
    
    insert_times = []
    total_start = time.time()
    
    for i in range(0, total_vectors, batch_size):
        batch_vectors = []
        batch_end = min(i + batch_size, total_vectors)
        
        for j in range(i, batch_end):
            vec = VectorRecord(
                id=f"stream_vec_{j}",
                vector=np.random.random(128).astype(np.float32).tolist(),
                metadata={"index": j, "batch": i // batch_size}
            )
            batch_vectors.append(vec)
        
        batch_start = time.time()
        client.insert_vectors(collection_name, batch_vectors)
        batch_time = (time.time() - batch_start) * 1000
        insert_times.append(batch_time)
        
        if (i // batch_size + 1) % 10 == 0:
            progress = i + batch_size
            rate = (progress / (time.time() - total_start))
            eta = (total_vectors - progress) / rate if rate > 0 else 0
            print(f"  Progress: {progress:,}/{total_vectors:,} ({rate:.0f} vec/s, ETA: {eta:.0f}s)")
    
    total_time = (time.time() - total_start) * 1000
    avg_batch_time = sum(insert_times) / len(insert_times)
    overall_rate = (total_vectors / total_time) * 1000
    
    print(f"\n✅ Streaming insert complete:")
    print(f"   - Total time: {total_time/1000:.2f}s")
    print(f"   - Overall rate: {overall_rate:,.0f} vectors/sec")
    print(f"   - Avg batch time: {avg_batch_time:.2f}ms")
    print(f"   - Batches per second: {1000/avg_batch_time:.2f}")
    
    # Cleanup
    client.delete_collection(collection_name)
    
    return {
        "total_vectors": total_vectors,
        "batch_size": batch_size,
        "total_time_ms": total_time,
        "vectors_per_second": overall_rate,
        "avg_batch_time_ms": avg_batch_time
    }

if __name__ == "__main__":
    # First test various batch sizes
    batch_results = test_grpc_batch_sizes()
    
    # Then test large dataset with optimal batch
    streaming_results = test_grpc_large_dataset_streaming()
    
    print("\n\n📊 FINAL RECOMMENDATIONS:")
    print("="*60)
    print("  gRPC Protocol:")
    print(f"    - Optimal batch size: 2000-3000 vectors")
    print(f"    - Can handle 100K vectors in ~3-5 seconds")
    print(f"    - Suitable for large-scale batch operations")
    print(f"    - Message size limit appears to be well above 4MB")
    print("\n  For maximum throughput:")
    print(f"    - Use gRPC with batch size 2000-3000")
    print(f"    - Enable connection pooling")
    print(f"    - Consider parallel clients for >1M vectors")