#!/usr/bin/env python3
"""
Simple gRPC API Test
Tests basic operations to verify the API is working correctly.
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients', 'python', 'src'))

import time
import numpy as np
import proximadb
from proximadb.models import CollectionConfig, DistanceMetric

def test_simple_grpc():
    """Simple test to verify basic operations"""
    print("🚀 Starting Simple gRPC API Test")
    
    # Create gRPC client (connecting to dedicated gRPC port)
    client = proximadb.connect_grpc(url="127.0.0.1:5680")
    
    try:
        # 1. Health check
        print("\n1️⃣ Health check...")
        health = client.health()
        print(f"✅ Server health: {health.status}")
        
        # 2. Create collection
        print("\n2️⃣ Creating collection...")
        collection_name = f"test_simple_{int(time.time())}"
        
        config = CollectionConfig(
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            storage_layout="viper",
            filterable_metadata_fields=["category", "priority"]
        )
        
        collection = client.create_collection(collection_name, config)
        print(f"✅ Created collection: {collection.name} (ID: {collection.id})")
        
        # 3. Insert single vector
        print("\n3️⃣ Inserting single vector...")
        vector = np.random.random(128).astype(np.float32)
        
        result = client.insert_vector(
            collection_id=collection.id,
            vector_id="test_vec_001",
            vector=vector.tolist(),
            metadata={"category": "test", "priority": 5}
        )
        print(f"✅ Inserted vector: {result.successful_count} vectors")
        
        # 4. Insert multiple vectors
        print("\n4️⃣ Inserting multiple vectors...")
        vectors = np.random.random((5, 128)).astype(np.float32)
        ids = [f"test_vec_{i:03d}" for i in range(2, 7)]
        metadata = [{"category": "test", "priority": i} for i in range(1, 6)]
        
        batch_result = client.insert_vectors(
            collection_id=collection.id,
            vectors=vectors.tolist(),
            ids=ids,
            metadata=metadata
        )
        print(f"✅ Inserted batch: {batch_result.successful_count} vectors")
        
        # 5. Get collection info
        print("\n5️⃣ Getting collection info...")
        coll_info = client.get_collection(collection.id)
        print(f"✅ Collection vector count: {coll_info.vector_count}")
        
        # 6. Search
        print("\n6️⃣ Searching vectors...")
        query = np.random.random(128).astype(np.float32)
        
        results = client.search(
            collection_id=collection.id,
            query=query.tolist(),
            k=3,
            include_metadata=True
        )
        print(f"✅ Found {len(results)} results")
        
        if results:
            for i, result in enumerate(results):
                print(f"   Result {i+1}: ID={result.id}, Score={result.score:.4f}")
        
        # 7. Get specific vector
        print("\n7️⃣ Getting specific vector...")
        vector_data = client.get_vector(
            collection_id=collection.id,
            vector_id="test_vec_001",
            include_metadata=True
        )
        if vector_data:
            print(f"✅ Found vector: {vector_data.get('id')}")
            print(f"   Metadata: {vector_data.get('metadata')}")
        
        # 8. Delete collection
        print("\n8️⃣ Cleaning up...")
        deleted = client.delete_collection(collection.id)
        print(f"✅ Deleted collection: {deleted}")
        
        print("\n🎉 Simple test completed successfully!")
        
    except Exception as e:
        print(f"❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
    finally:
        client.close()

if __name__ == "__main__":
    test_simple_grpc()