#!/usr/bin/env python3
"""
Test ProximaDB Python SDK
"""
import sys
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb import ProximaDBClient
import numpy as np

def test_python_sdk():
    print("🐍 Testing ProximaDB Python SDK")
    print("=" * 50)
    
    # Create client
    print("📡 Creating ProximaDB client...")
    client = ProximaDBClient("http://localhost:5678")  # REST endpoint
    
    # Test health check
    print("\n🏥 Testing health check...")
    try:
        # The client should auto-detect REST protocol
        collections = client.list_collections()
        print("✅ Connection successful!")
        print(f"   Collections found: {len(collections)}")
    except Exception as e:
        print(f"❌ Connection failed: {e}")
        return
    
    # Create collection
    print("\n📦 Creating collection via SDK...")
    try:
        client.create_collection(
            name="sdk_test_collection",
            dimension=384,
            distance_metric="cosine",
            storage_engine="viper"
        )
        print("✅ Collection created successfully")
    except Exception as e:
        print(f"❌ Collection creation failed: {e}")
    
    # List collections
    print("\n📋 Listing collections...")
    try:
        collections = client.list_collections()
        print(f"✅ Found {len(collections)} collections")
        for col in collections:
            print(f"   - {col}")
    except Exception as e:
        print(f"❌ Listing failed: {e}")
    
    # Insert vectors
    print("\n🔢 Inserting vectors...")
    try:
        vectors = []
        for i in range(3):
            vector = np.random.rand(384).tolist()
            vectors.append({
                "id": f"sdk_vector_{i}",
                "vector": vector,
                "metadata": {"category": "sdk_test", "index": i}
            })
        
        # Insert vectors one by one
        for vec in vectors:
            client.insert_vector(
                collection_id="sdk_test_collection",
                vector_id=vec["id"],
                vector=vec["vector"],
                metadata=vec["metadata"]
            )
        print("✅ Vectors inserted successfully")
    except Exception as e:
        print(f"❌ Vector insertion failed: {e}")
    
    # Search vectors
    print("\n🔍 Searching vectors...")
    try:
        query_vector = np.random.rand(384).tolist()
        results = client.search(
            collection_id="sdk_test_collection",
            query=query_vector,
            k=2
        )
        print(f"✅ Search completed, found {len(results)} results")
        for i, result in enumerate(results):
            print(f"   {i+1}. ID: {result.get('id', 'N/A')}, Score: {result.get('score', 'N/A')}")
    except Exception as e:
        print(f"❌ Search failed: {e}")
    
    # Delete collection
    print("\n🗑️ Cleaning up...")
    try:
        client.delete_collection("sdk_test_collection")
        print("✅ Collection deleted")
    except Exception as e:
        print(f"❌ Deletion failed: {e}")
    
    print("\n" + "=" * 50)
    print("🎉 SDK test complete!")

if __name__ == "__main__":
    test_python_sdk()