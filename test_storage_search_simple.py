#!/usr/bin/env python3
"""
Simple test for storage-aware search optimizations
"""
import requests
import json
import numpy as np
import time

def test_rest_api():
    """Test storage-aware search via REST API"""
    base_url = "http://localhost:5678"
    
    print("🚀 Testing storage-aware search optimizations via REST API")
    print("=" * 60)
    
    # Test 1: Health check
    print("\n1. Health Check...")
    try:
        response = requests.get(f"{base_url}/health")
        print(f"✅ Health: {response.status_code} - {response.json()}")
    except Exception as e:
        print(f"❌ Health check failed: {e}")
        return
    
    # Test 2: Create VIPER collection
    print("\n2. Creating VIPER collection...")
    try:
        collection_data = {
            "name": "test_viper_search",
            "dimension": 384,
            "distance_metric": "cosine",
            "storage_engine": "viper",
            "indexing_algorithm": "hnsw"
        }
        response = requests.post(f"{base_url}/collections", json=collection_data)
        print(f"✅ VIPER Collection: {response.status_code}")
        if response.status_code >= 400:
            print(f"   Response: {response.text}")
    except Exception as e:
        print(f"❌ VIPER collection creation failed: {e}")
    
    # Test 3: Create LSM collection
    print("\n3. Creating LSM collection...")
    try:
        collection_data = {
            "name": "test_lsm_search",
            "dimension": 384,
            "distance_metric": "cosine",
            "storage_engine": "lsm",
            "indexing_algorithm": "hnsw"
        }
        response = requests.post(f"{base_url}/collections", json=collection_data)
        print(f"✅ LSM Collection: {response.status_code}")
        if response.status_code >= 400:
            print(f"   Response: {response.text}")
    except Exception as e:
        print(f"❌ LSM collection creation failed: {e}")
    
    # Test 4: Insert test vectors into VIPER collection
    print("\n4. Inserting vectors into VIPER collection...")
    try:
        vectors = []
        for i in range(10):
            vector = np.random.normal(0, 1, 384).astype(np.float32)
            vector = vector / np.linalg.norm(vector)
            vectors.append({
                "id": f"viper_vec_{i:03d}",
                "vector": vector.tolist(),
                "metadata": {
                    "category": f"cat_{i % 3}",
                    "cluster": i % 2,
                    "value": float(i * 0.5)
                }
            })
        
        # Insert vectors
        response = requests.post(
            f"{base_url}/collections/test_viper_search/vectors/batch",
            json=vectors
        )
        print(f"✅ VIPER vectors: {response.status_code}")
        if response.status_code >= 400:
            print(f"   Response: {response.text}")
    except Exception as e:
        print(f"❌ VIPER vector insertion failed: {e}")
    
    # Test 5: Insert test vectors into LSM collection
    print("\n5. Inserting vectors into LSM collection...")
    try:
        vectors = []
        for i in range(10):
            vector = np.random.normal(0, 1, 384).astype(np.float32)
            vector = vector / np.linalg.norm(vector)
            vectors.append({
                "id": f"lsm_vec_{i:03d}",
                "vector": vector.tolist(),
                "metadata": {
                    "category": f"cat_{i % 3}",
                    "cluster": i % 2,
                    "value": float(i * 0.5)
                }
            })
        
        # Insert vectors
        response = requests.post(
            f"{base_url}/collections/test_lsm_search/vectors/batch",
            json=vectors
        )
        print(f"✅ LSM vectors: {response.status_code}")
        if response.status_code >= 400:
            print(f"   Response: {response.text}")
    except Exception as e:
        print(f"❌ LSM vector insertion failed: {e}")
    
    # Test 6: Storage-aware search on VIPER collection
    print("\n6. Testing VIPER storage-aware search...")
    try:
        query_vector = np.random.normal(0, 1, 384).astype(np.float32)
        query_vector = query_vector / np.linalg.norm(query_vector)
        
        search_data = {
            "vector": query_vector.tolist(),
            "k": 5,
            "include_metadata": True,
            "search_hints": {
                "storage_aware": True,
                "quantization_level": "FP32",
                "predicate_pushdown": True,
                "use_clustering": True
            }
        }
        
        start_time = time.time()
        response = requests.post(
            f"{base_url}/collections/test_viper_search/search",
            json=search_data
        )
        search_time = (time.time() - start_time) * 1000
        
        print(f"✅ VIPER search: {response.status_code} in {search_time:.2f}ms")
        if response.status_code == 200:
            results = response.json()
            print(f"   Found {len(results.get('results', []))} results")
        else:
            print(f"   Response: {response.text}")
    except Exception as e:
        print(f"❌ VIPER search failed: {e}")
    
    # Test 7: Storage-aware search on LSM collection
    print("\n7. Testing LSM storage-aware search...")
    try:
        query_vector = np.random.normal(0, 1, 384).astype(np.float32)
        query_vector = query_vector / np.linalg.norm(query_vector)
        
        search_data = {
            "vector": query_vector.tolist(),
            "k": 5,
            "include_metadata": True,
            "search_hints": {
                "storage_aware": True,
                "use_bloom_filters": True,
                "level_aware_search": True
            }
        }
        
        start_time = time.time()
        response = requests.post(
            f"{base_url}/collections/test_lsm_search/search",
            json=search_data
        )
        search_time = (time.time() - start_time) * 1000
        
        print(f"✅ LSM search: {response.status_code} in {search_time:.2f}ms")
        if response.status_code == 200:
            results = response.json()
            print(f"   Found {len(results.get('results', []))} results")
        else:
            print(f"   Response: {response.text}")
    except Exception as e:
        print(f"❌ LSM search failed: {e}")
    
    # Test 8: Quantization test on VIPER
    print("\n8. Testing VIPER quantization levels...")
    try:
        query_vector = np.random.normal(0, 1, 384).astype(np.float32)
        query_vector = query_vector / np.linalg.norm(query_vector)
        
        quantization_levels = ["FP32", "PQ8", "PQ4", "Binary"]
        
        for quant_level in quantization_levels:
            search_data = {
                "vector": query_vector.tolist(),
                "k": 5,
                "search_hints": {
                    "storage_aware": True,
                    "quantization_level": quant_level
                }
            }
            
            start_time = time.time()
            response = requests.post(
                f"{base_url}/collections/test_viper_search/search",
                json=search_data
            )
            search_time = (time.time() - start_time) * 1000
            
            if response.status_code == 200:
                results = response.json()
                print(f"   {quant_level:6}: {search_time:6.2f}ms, {len(results.get('results', []))} results")
            else:
                print(f"   {quant_level:6}: Failed - {response.status_code}")
    except Exception as e:
        print(f"❌ Quantization test failed: {e}")
    
    # Test 9: Cleanup
    print("\n9. Cleanup...")
    try:
        requests.delete(f"{base_url}/collections/test_viper_search")
        requests.delete(f"{base_url}/collections/test_lsm_search")
        print("✅ Cleanup completed")
    except Exception as e:
        print(f"❌ Cleanup failed: {e}")
    
    print("\n" + "=" * 60)
    print("🎯 Storage-aware search optimization test completed!")

if __name__ == "__main__":
    test_rest_api()