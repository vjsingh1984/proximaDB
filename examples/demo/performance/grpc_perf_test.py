#!/usr/bin/env python3
"""
gRPC Performance Test for ProximaDB
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import numpy as np
from proximadb import connect_grpc, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord

def test_grpc_performance():
    """Test gRPC protocol performance"""
    
    print("🚀 Testing gRPC Protocol Performance")
    
    # Connect to gRPC server
    client = connect_grpc("http://localhost:5679")
    
    # Create test collection
    collection_name = f"grpc_test_{int(time.time())}"
    config = CollectionConfig(
        name=collection_name,
        dimension=128,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER,
        description="gRPC performance test"
    )
    
    print(f"\n📊 Creating collection via gRPC...")
    start = time.time()
    collection = client.create_collection(collection_name, config)
    create_time = (time.time() - start) * 1000
    print(f"✅ Collection created: {create_time:.2f}ms")
    
    # Test batch insert
    print(f"\n📝 Testing batch insert...")
    vectors = []
    for i in range(100):
        vec = VectorRecord(
            id=f"vec_{i}",
            vector=np.random.random(128).astype(np.float32).tolist(),
            metadata={"index": i}
        )
        vectors.append(vec)
    
    start = time.time()
    result = client.insert_vectors(collection_name, vectors)
    insert_time = (time.time() - start) * 1000
    vectors_per_sec = (100 / insert_time) * 1000
    print(f"✅ Insert 100 vectors: {insert_time:.2f}ms ({vectors_per_sec:.0f} vectors/sec)")
    
    # Test search
    print(f"\n🔍 Testing search...")
    query = np.random.random(128).astype(np.float32).tolist()
    
    start = time.time()
    results = client.search(collection_name, query, top_k=10)
    search_time = (time.time() - start) * 1000
    print(f"✅ Search top-10: {search_time:.2f}ms")
    
    # Cleanup
    client.delete_collection(collection_name)
    
    return {
        "protocol": "grpc",
        "create_ms": create_time,
        "insert_rate": vectors_per_sec,
        "search_ms": search_time
    }

if __name__ == "__main__":
    try:
        results = test_grpc_performance()
        print(f"\n📊 gRPC Performance Summary:")
        print(f"  - Collection Create: {results['create_ms']:.2f}ms")
        print(f"  - Insert Rate: {results['insert_rate']:.0f} vectors/sec")
        print(f"  - Search Latency: {results['search_ms']:.2f}ms")
    except Exception as e:
        print(f"❌ Error: {e}")
        import traceback
        traceback.print_exc()