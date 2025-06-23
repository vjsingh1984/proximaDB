#!/usr/bin/env python3
"""
Simple End-to-End Test for ProximaDB
Tests basic connectivity and operations without heavy dependencies
"""

import requests
import json
import numpy as np
import time
import sys

SERVER_URL = "http://localhost:5678"

def test_health_check():
    """Test health endpoint"""
    print("🏥 Testing health check...")
    response = requests.get(f"{SERVER_URL}/health")
    if response.status_code == 200:
        print("✅ Health check passed")
        print(f"   Response: {response.json()}")
        return True
    else:
        print(f"❌ Health check failed: {response.status_code}")
        return False

def test_create_collection():
    """Test collection creation"""
    print("📦 Testing collection creation...")
    
    collection_data = {
        "name": "test_collection",
        "dimension": 384,
        "distance_metric": "COSINE",
        "storage_engine": "VIPER",
        "indexing_algorithm": "HNSW",
        "filterable_metadata_fields": ["category"],
        "indexing_config": {}
    }
    
    response = requests.post(
        f"{SERVER_URL}/collections",
        json=collection_data,
        headers={"Content-Type": "application/json"}
    )
    
    if response.status_code in [200, 201]:
        print("✅ Collection created successfully")
        result = response.json()
        collection_id = result.get('data', result.get('id', 'test_collection'))
        print(f"   Collection ID: {collection_id}")
        return collection_id
    else:
        print(f"❌ Collection creation failed: {response.status_code}")
        print(f"   Response: {response.text}")
        return None

def test_list_collections():
    """Test listing collections"""
    print("📋 Testing collection listing...")
    
    response = requests.get(f"{SERVER_URL}/collections")
    
    if response.status_code == 200:
        print("✅ Collections listed successfully")
        result = response.json()
        print(f"   Found {len(result.get('collections', []))} collections")
        return True
    else:
        print(f"❌ Collection listing failed: {response.status_code}")
        return False

def test_vector_operations(collection_id):
    """Test vector insert, search operations"""
    if not collection_id:
        print("⏭️ Skipping vector operations - no collection ID")
        return False
        
    print("🔢 Testing vector operations...")
    
    # Create simple test vectors
    vectors = []
    for i in range(5):
        vector = np.random.rand(384).tolist()
        vectors.append({
            "id": f"test_vector_{i}",
            "vector": vector,
            "metadata": {"category": "test", "index": i}
        })
    
    # Test vector insertion (single vector at a time)
    insert_data = vectors[0]  # Send just the first vector
    
    response = requests.post(
        f"{SERVER_URL}/collections/{collection_id}/vectors",
        json=insert_data,
        headers={"Content-Type": "application/json"}
    )
    
    if response.status_code in [200, 201]:
        print("✅ Vector insertion successful")
        result = response.json()
        print(f"   Inserted vectors: {result.get('vector_ids', [])}")
    else:
        print(f"❌ Vector insertion failed: {response.status_code}")
        print(f"   Response: {response.text}")
        return False
    
    # Test vector search
    query_vector = np.random.rand(384).tolist()
    search_data = {
        "vector": query_vector,
        "top_k": 3,
        "include_vectors": False,
        "include_metadata": True
    }
    
    response = requests.post(
        f"{SERVER_URL}/collections/{collection_id}/search",
        json=search_data,
        headers={"Content-Type": "application/json"}
    )
    
    if response.status_code == 200:
        print("✅ Vector search successful")
        result = response.json()
        print(f"   Found {len(result.get('results', []))} results")
        return True
    else:
        print(f"❌ Vector search failed: {response.status_code}")
        print(f"   Response: {response.text}")
        return False

def main():
    """Run all tests"""
    print("🚀 Starting ProximaDB End-to-End Test")
    print("=" * 50)
    
    # Give server time to start
    print("⏱️ Waiting for server startup...")
    time.sleep(2)
    
    results = []
    
    # Test 1: Health check
    results.append(test_health_check())
    
    # Test 2: Create collection
    collection_id = test_create_collection()
    results.append(collection_id is not None)
    
    # Test 3: List collections
    results.append(test_list_collections())
    
    # Test 4: Vector operations
    results.append(test_vector_operations(collection_id))
    
    # Summary
    print("\n" + "=" * 50)
    print("📊 Test Summary:")
    passed = sum(results)
    total = len(results)
    print(f"   ✅ Passed: {passed}/{total}")
    print(f"   ❌ Failed: {total - passed}/{total}")
    
    if passed == total:
        print("🎉 All tests passed!")
        return 0
    else:
        print("💥 Some tests failed!")
        return 1

if __name__ == "__main__":
    sys.exit(main())