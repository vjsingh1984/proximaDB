#!/usr/bin/env python3
"""
Simple performance tests that work with current API
"""
import requests
import json
import time
import numpy as np


def test_1k_performance():
    """Test 1K vector performance using direct HTTP"""
    print("🚀 1K Vector Performance Test")
    print("=" * 40)
    
    base_url = "http://localhost:5678"
    collection_id = f"perf_1k_{int(time.time())}"
    dimension = 384
    num_vectors = 1000
    
    # Create collection
    print(f"📁 Creating collection: {collection_id}")
    collection_data = {
        "name": collection_id,
        "dimension": dimension,
        "distance_metric": "cosine",
        "storage_engine": "viper"
    }
    
    response = requests.post(f"{base_url}/collections", json=collection_data)
    assert response.status_code == 200
    print(f"✅ Collection created")
    
    # Insert vectors in batches
    batch_size = 100
    start_time = time.time()
    total_inserted = 0
    
    for i in range(0, num_vectors, batch_size):
        batch_end = min(i + batch_size, num_vectors)
        
        for j in range(i, batch_end):
            vector_data = {
                "id": f"vector_{j:04d}",
                "vector": np.random.random(dimension).astype(np.float32).tolist(),
                "metadata": {"index": j, "batch": i // batch_size}
            }
            
            response = requests.post(
                f"{base_url}/collections/{collection_id}/vectors",
                json=vector_data
            )
            
            if response.status_code == 200:
                total_inserted += 1
        
        if i % 500 == 0:
            elapsed = time.time() - start_time
            rate = total_inserted / elapsed if elapsed > 0 else 0
            print(f"   Progress: {total_inserted}/{num_vectors} vectors ({rate:.1f}/s)")
    
    end_time = time.time()
    duration = end_time - start_time
    
    print(f"✅ Inserted {total_inserted} vectors in {duration:.2f}s")
    print(f"📊 Rate: {total_inserted/duration:.1f} vectors/second")
    
    # Cleanup
    requests.delete(f"{base_url}/collections/{collection_id}")
    
    assert total_inserted >= num_vectors * 0.9  # Allow some failures
    return total_inserted, duration


def test_5k_performance():
    """Test 5K vector performance using direct HTTP"""
    print("\n🚀 5K Vector Performance Test")
    print("=" * 40)
    
    base_url = "http://localhost:5678"
    collection_id = f"perf_5k_{int(time.time())}"
    dimension = 384
    num_vectors = 5000
    
    # Create collection
    print(f"📁 Creating collection: {collection_id}")
    collection_data = {
        "name": collection_id,
        "dimension": dimension,
        "distance_metric": "cosine",
        "storage_engine": "viper"
    }
    
    response = requests.post(f"{base_url}/collections", json=collection_data)
    assert response.status_code == 200
    print(f"✅ Collection created")
    
    # Insert vectors in batches
    batch_size = 200
    start_time = time.time()
    total_inserted = 0
    
    for i in range(0, num_vectors, batch_size):
        batch_end = min(i + batch_size, num_vectors)
        
        for j in range(i, batch_end):
            vector_data = {
                "id": f"vector_{j:04d}",
                "vector": np.random.random(dimension).astype(np.float32).tolist(),
                "metadata": {"index": j, "category": f"cat_{j % 20}"}
            }
            
            response = requests.post(
                f"{base_url}/collections/{collection_id}/vectors",
                json=vector_data
            )
            
            if response.status_code == 200:
                total_inserted += 1
        
        if i % 1000 == 0:
            elapsed = time.time() - start_time
            rate = total_inserted / elapsed if elapsed > 0 else 0
            print(f"   Progress: {total_inserted}/{num_vectors} vectors ({rate:.1f}/s)")
    
    end_time = time.time()
    duration = end_time - start_time
    
    print(f"✅ Inserted {total_inserted} vectors in {duration:.2f}s")
    print(f"📊 Rate: {total_inserted/duration:.1f} vectors/second")
    
    # Search performance test
    print(f"🔍 Testing search performance...")
    query_vector = np.random.random(dimension).astype(np.float32).tolist()
    
    search_times = []
    for i in range(5):
        search_start = time.time()
        search_response = requests.post(
            f"{base_url}/collections/{collection_id}/search",
            json={"vector": query_vector, "k": 10}
        )
        search_end = time.time()
        
        if search_response.status_code == 200:
            search_time = search_end - search_start
            search_times.append(search_time)
            search_data = search_response.json()
            results = search_data.get("data", [])
            print(f"   Search {i+1}: {search_time:.3f}s, found {len(results)} results")
    
    if search_times:
        avg_search_time = sum(search_times) / len(search_times)
        print(f"📊 Average search time: {avg_search_time:.3f}s")
    
    # Cleanup
    requests.delete(f"{base_url}/collections/{collection_id}")
    
    assert total_inserted >= num_vectors * 0.9  # Allow some failures
    return total_inserted, duration


def test_25k_performance():
    """Test 25K vector performance using direct HTTP"""
    print("\n🚀 25K Vector Performance Test")
    print("=" * 40)
    
    base_url = "http://localhost:5678"
    collection_id = f"perf_25k_{int(time.time())}"
    dimension = 384
    num_vectors = 25000
    
    # Create collection
    print(f"📁 Creating collection: {collection_id}")
    collection_data = {
        "name": collection_id,
        "dimension": dimension,
        "distance_metric": "cosine",
        "storage_engine": "viper"
    }
    
    response = requests.post(f"{base_url}/collections", json=collection_data)
    assert response.status_code == 200
    print(f"✅ Collection created")
    
    # Insert vectors in larger batches
    batch_size = 500
    start_time = time.time()
    total_inserted = 0
    
    for i in range(0, num_vectors, batch_size):
        batch_end = min(i + batch_size, num_vectors)
        
        for j in range(i, batch_end):
            vector_data = {
                "id": f"vector_{j:05d}",
                "vector": np.random.random(dimension).astype(np.float32).tolist(),
                "metadata": {
                    "index": j, 
                    "category": f"cat_{j % 50}",
                    "priority": j % 5
                }
            }
            
            response = requests.post(
                f"{base_url}/collections/{collection_id}/vectors",
                json=vector_data
            )
            
            if response.status_code == 200:
                total_inserted += 1
        
        if i % 5000 == 0:
            elapsed = time.time() - start_time
            rate = total_inserted / elapsed if elapsed > 0 else 0
            print(f"   Progress: {total_inserted:,}/{num_vectors:,} vectors ({rate:.1f}/s)")
    
    end_time = time.time()
    duration = end_time - start_time
    
    print(f"✅ Inserted {total_inserted:,} vectors in {duration:.2f}s ({duration/60:.1f}m)")
    print(f"📊 Rate: {total_inserted/duration:.1f} vectors/second")
    
    # Search scaling test
    print(f"🔍 Testing search scaling...")
    query_vector = np.random.random(dimension).astype(np.float32).tolist()
    
    search_configs = [10, 50, 100]
    for k in search_configs:
        search_start = time.time()
        search_response = requests.post(
            f"{base_url}/collections/{collection_id}/search",
            json={"vector": query_vector, "k": k}
        )
        search_end = time.time()
        
        if search_response.status_code == 200:
            search_time = search_end - search_start
            search_data = search_response.json()
            results = search_data.get("data", [])
            print(f"   Top-{k}: {search_time:.3f}s, found {len(results)} results")
    
    # Cleanup
    requests.delete(f"{base_url}/collections/{collection_id}")
    
    assert total_inserted >= num_vectors * 0.8  # Allow more failures for large scale
    return total_inserted, duration


if __name__ == "__main__":
    # Health check
    response = requests.get("http://localhost:5678/health")
    if response.status_code != 200:
        print("❌ Server not available")
        exit(1)
    
    print("✅ Server is healthy")
    
    try:
        # Run performance tests
        print("\n" + "="*60)
        print("PERFORMANCE TEST SUITE")
        print("="*60)
        
        total_start = time.time()
        
        # 1K test
        count_1k, time_1k = test_1k_performance()
        
        # 5K test
        count_5k, time_5k = test_5k_performance() 
        
        # 25K test (commented out for speed)
        # count_25k, time_25k = test_25k_performance()
        
        total_end = time.time()
        total_duration = total_end - total_start
        
        # Summary
        print("\n" + "="*60)
        print("PERFORMANCE SUMMARY")
        print("="*60)
        print(f"1K vectors:  {count_1k:,} inserted in {time_1k:.2f}s ({count_1k/time_1k:.1f}/s)")
        print(f"5K vectors:  {count_5k:,} inserted in {time_5k:.2f}s ({count_5k/time_5k:.1f}/s)")
        # print(f"25K vectors: {count_25k:,} inserted in {time_25k:.2f}s ({count_25k/time_25k:.1f}/s)")
        print(f"Total time:  {total_duration:.2f}s ({total_duration/60:.1f}m)")
        
        print("\n🎉 All performance tests completed successfully!")
        
    except Exception as e:
        print(f"\n❌ Performance test failed: {e}")
        import traceback
        traceback.print_exc()
        exit(1)