#!/usr/bin/env python3
"""
gRPC + LSM Insert Performance Test with Persistence
Tests data persistence across server restarts
"""

# Set PYTHONPATH to include src directory
import sys
import os
if 'PYTHONPATH' not in os.environ:
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import json
import numpy as np
from proximadb import connect_grpc, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord

# Test configuration
COLLECTION_NAME = "grpc_lsm_persist_test"
DIMENSION = 128
NUM_VECTORS = 100000  # Larger dataset for gRPC
BATCH_SIZE = 3000  # Optimal for gRPC

def generate_test_vectors(num_vectors, dimension):
    """Generate reproducible test vectors"""
    np.random.seed(45)  # Different seed for gRPC LSM
    vectors = []
    
    print(f"📊 Generating {num_vectors:,} test vectors...")
    for i in range(num_vectors):
        vec_data = np.random.randn(dimension).astype(np.float32)
        vec_data = vec_data / np.linalg.norm(vec_data)  # Normalize
        
        vec = VectorRecord(
            id=f"grpc_lsm_{i}",
            vector=vec_data.tolist(),
            metadata={
                "index": i,
                "protocol": "grpc",
                "engine": "lsm",
                "batch": i // BATCH_SIZE
            }
        )
        vectors.append(vec)
    
    return vectors

def main():
    print("🚀 gRPC + LSM Insert Performance Test")
    print(f"   Dataset: {NUM_VECTORS:,} vectors")
    print(f"   Dimension: {DIMENSION}")
    print(f"   Batch size: {BATCH_SIZE}")
    print("="*80)
    
    # Connect to gRPC API
    client = connect_grpc("http://localhost:5679")
    
    # Check if collection already exists
    try:
        existing = client.get_collection(COLLECTION_NAME)
        if existing:
            print(f"⚠️  Collection '{COLLECTION_NAME}' already exists. Delete it first if you want to rerun.")
            return
    except:
        pass  # Collection doesn't exist, proceed
    
    # Create collection
    print("\n📦 Creating LSM collection...")
    config = CollectionConfig(
        name=COLLECTION_NAME,
        dimension=DIMENSION,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.LSM,
        description="gRPC + LSM persistence test"
    )
    
    start = time.time()
    collection = client.create_collection(COLLECTION_NAME, config)
    create_time = (time.time() - start) * 1000
    print(f"✅ Collection created in {create_time:.2f}ms")
    
    # Generate vectors
    vectors = generate_test_vectors(NUM_VECTORS, DIMENSION)
    
    # Insert vectors in batches
    print(f"\n📝 Inserting {NUM_VECTORS:,} vectors in batches of {BATCH_SIZE}...")
    insert_times = []
    total_start = time.time()
    
    for i in range(0, NUM_VECTORS, BATCH_SIZE):
        batch = vectors[i:i+BATCH_SIZE]
        batch_num = i // BATCH_SIZE
        
        batch_start = time.time()
        client.insert_vectors(COLLECTION_NAME, batch)
        batch_time = (time.time() - batch_start) * 1000
        insert_times.append(batch_time)
        
        if (batch_num + 1) % 10 == 0:
            avg_time = sum(insert_times[-10:]) / len(insert_times[-10:])
            rate = (BATCH_SIZE / avg_time) * 1000
            progress = min(i + BATCH_SIZE, NUM_VECTORS)
            print(f"  Progress: {progress:,}/{NUM_VECTORS:,} vectors ({rate:.0f} vec/s)")
    
    total_insert_time = (time.time() - total_start)
    avg_batch_time = sum(insert_times) / len(insert_times)
    insert_rate = NUM_VECTORS / total_insert_time
    
    print(f"\n✅ Insert complete:")
    print(f"   - Total time: {total_insert_time:.2f}s")
    print(f"   - Insert rate: {insert_rate:,.0f} vectors/sec")
    print(f"   - Avg batch time: {avg_batch_time:.2f}ms")
    
    # Get collection info to verify
    print("\n🔍 Verifying collection...")
    collection_info = client.get_collection(COLLECTION_NAME)
    print(f"✅ Collection verified: {COLLECTION_NAME}")
    
    # Save results
    results = {
        "test_type": "gRPC + LSM Insert",
        "collection_name": COLLECTION_NAME,
        "num_vectors": NUM_VECTORS,
        "dimension": DIMENSION,
        "batch_size": BATCH_SIZE,
        "metrics": {
            "create_collection_ms": create_time,
            "total_insert_time_s": total_insert_time,
            "insert_rate_vec_per_s": insert_rate,
            "avg_batch_time_ms": avg_batch_time,
            "batches_processed": len(insert_times)
        },
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S")
    }
    
    with open("grpc_lsm_insert_results.json", "w") as f:
        json.dump(results, f, indent=2)
    
    print("\n📊 Results saved to grpc_lsm_insert_results.json")
    print("\n⚠️  IMPORTANT: Data has been inserted. Server can now be restarted to test persistence.")
    print("   After restart, run the search performance test to verify data recovery.")
    
    # Test immediate search to establish baseline
    print("\n🔍 Testing immediate search (baseline)...")
    query = np.random.randn(DIMENSION).astype(np.float32)
    query = query / np.linalg.norm(query)
    
    search_times = []
    for _ in range(5):
        start = time.time()
        results = client.search(COLLECTION_NAME, query.tolist(), top_k=100)
        search_time = (time.time() - start) * 1000
        search_times.append(search_time)
    
    avg_search = sum(search_times) / len(search_times)
    print(f"✅ Baseline search latency: {avg_search:.2f}ms")
    
    # Save baseline
    with open("grpc_lsm_baseline.json", "w") as f:
        json.dump({
            "baseline_search_ms": avg_search,
            "num_results": len(results) if results else 0
        }, f, indent=2)

if __name__ == "__main__":
    main()