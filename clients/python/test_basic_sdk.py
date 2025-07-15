#!/usr/bin/env python3
"""
Basic SDK functionality test to verify the Python client works
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import numpy as np
from proximadb import connect_rest, CollectionConfig, DistanceMetric
import logging

# Enable debug logging
logging.basicConfig(level=logging.DEBUG)

def test_basic_functionality():
    """Test basic SDK functionality"""
    try:
        # Connect to the server
        print("🔗 Connecting to ProximaDB...")
        client = connect_rest("http://localhost:5678")
        
        # Test health check
        print("🩺 Checking server health...")
        try:
            health = client.health()
            print(f"✅ Server health: {health}")
        except:
            print("⚠️ Health endpoint not available, continuing...")
        
        # List collections
        print("📋 Listing collections...")
        collections = client.list_collections()
        print(f"✅ Found {len(collections)} collections")
        
        # Create a test collection
        print("🗂️ Creating test collection...")
        collection_name = f"basic_test_{int(np.random.random() * 10000)}"
        config = CollectionConfig(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            description="Basic functionality test collection"
        )
        
        collection = client.create_collection(collection_name, config)
        print(f"✅ Created collection: {collection}")
        
        # Insert some vectors
        print("📝 Inserting test vectors...")
        from proximadb.models import VectorRecord
        
        vectors = []
        for i in range(5):
            vec = VectorRecord(
                id=f"vec_{i}",
                vector=np.random.random(128).astype(np.float32).tolist(),
                metadata={"index": i, "category": f"test_{i % 2}"}
            )
            vectors.append(vec)
        
        result = client.insert_vectors(collection_name, vectors)
        print(f"✅ Inserted vectors: {result}")
        
        # Search for similar vectors
        print("🔍 Searching for similar vectors...")
        query_vector = np.random.random(128).astype(np.float32).tolist()
        search_results = client.search(collection_name, query_vector, top_k=3)
        print(f"✅ Search results: {len(search_results)} results")
        
        # Get collection info
        print("ℹ️ Getting collection info...")
        collection_info = client.get_collection(collection_name)
        print(f"✅ Collection info: {collection_info}")
        
        # Cleanup
        print("🧹 Cleaning up...")
        client.delete_collection(collection_name)
        print("✅ Collection deleted")
        
        print("\n🎉 All basic functionality tests passed!")
        return True
        
    except Exception as e:
        print(f"\n❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

if __name__ == "__main__":
    success = test_basic_functionality()
    sys.exit(0 if success else 1)