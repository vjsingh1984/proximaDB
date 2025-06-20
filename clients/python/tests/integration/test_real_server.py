#!/usr/bin/env python3
"""
Test script to verify ProximaDB server is working with real HTTP calls
"""

import requests
import json
import time
import numpy as np

def test_real_server():
    """Test the real ProximaDB server using HTTP REST API"""
    print("🔍 Testing real ProximaDB server via REST API...")
    
    base_url = "http://localhost:5678"
    
    # Test 1: Health check
    print("1️⃣ Testing health endpoint...")
    response = requests.get(f"{base_url}/health")
    print(f"   Status: {response.status_code}")
    print(f"   Response: {response.json()}")
    
    # Test 2: Create collection
    print("2️⃣ Creating collection via REST API...")
    collection_data = {
        "name": "test_real_collection",
        "dimension": 128,
        "distance_metric": "cosine",
        "indexing_algorithm": "hnsw",
        "storage_layout": "viper",  # Use VIPER storage by default
        "filterable_metadata_fields": ["category", "priority"]
    }
    
    response = requests.post(f"{base_url}/api/v1/collections", json=collection_data)
    print(f"   Status: {response.status_code}")
    if response.status_code == 200:
        collection = response.json()
        print(f"   Created collection: {collection.get('name')} (ID: {collection.get('id')})")
        collection_id = collection.get('id')
    else:
        print(f"   Error: {response.text}")
        return
    
    # Test 3: Insert vectors
    print("3️⃣ Inserting vectors via REST API...")
    vectors_data = {
        "vectors": [
            {
                "id": "real_vec_001",
                "vector": np.random.random(128).tolist(),
                "metadata": {"category": "test", "priority": 1}
            },
            {
                "id": "real_vec_002", 
                "vector": np.random.random(128).tolist(),
                "metadata": {"category": "real", "priority": 2}
            }
        ],
        "upsert": False
    }
    
    start_time = time.time()
    response = requests.post(f"{base_url}/api/v1/collections/{collection_id}/vectors", json=vectors_data)
    insert_time = time.time() - start_time
    
    print(f"   Status: {response.status_code}")
    if response.status_code == 200:
        result = response.json()
        print(f"   Inserted: {result.get('count', 0)} vectors")
        print(f"   Time: {insert_time:.3f}s")
        print(f"   Rate: {result.get('count', 0)/insert_time:.0f} vectors/sec")
    else:
        print(f"   Error: {response.text}")
        return
    
    # Test 4: Search vectors
    print("4️⃣ Searching vectors via REST API...")
    search_data = {
        "query": np.random.random(128).tolist(),
        "k": 5,
        "params": {
            "include_metadata": True,
            "include_vectors": False
        }
    }
    
    response = requests.post(f"{base_url}/api/v1/collections/{collection_id}/search", json=search_data)
    print(f"   Status: {response.status_code}")
    if response.status_code == 200:
        result = response.json()
        results = result.get('results', [])
        print(f"   Found: {len(results)} results")
        if results:
            print(f"   Top result: ID={results[0].get('id')}, Score={results[0].get('score'):.4f}")
    else:
        print(f"   Error: {response.text}")
    
    # Test 5: Get collection info
    print("5️⃣ Getting collection info...")
    response = requests.get(f"{base_url}/api/v1/collections/{collection_id}")
    if response.status_code == 200:
        collection_info = response.json()
        print(f"   Collection: {collection_info.get('name')}")
        print(f"   Vector count: {collection_info.get('vector_count', 0)}")
        print(f"   Dimension: {collection_info.get('dimension')}")
    
    # Test 6: Cleanup
    print("6️⃣ Cleaning up...")
    response = requests.delete(f"{base_url}/api/v1/collections/{collection_id}")
    print(f"   Cleanup status: {response.status_code}")
    
    print("✅ Real server test completed!")

if __name__ == "__main__":
    test_real_server()