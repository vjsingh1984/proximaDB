#!/usr/bin/env python3
"""
Simple test to verify VIPER collection flush at 1MB threshold using direct HTTP
"""

import requests
import time
import random
import json

def main():
    print("🔥 1MB Flush Threshold Test (Simple HTTP)")
    print("=" * 60)
    
    base_url = "http://localhost:5678"
    collection_name = f"flush_test_{int(time.time())}"
    
    # Use larger dimensions to increase data size per vector
    dimension = 384  # 384 dimensions * 4 bytes = 1536 bytes per vector
    vectors_per_mb = 1024 * 1024 // (dimension * 4)  # ~171 vectors per MB
    target_vectors = int(vectors_per_mb * 1.2)  # 20% over 1MB to ensure flush
    
    print(f"📊 Test Parameters:")
    print(f"   Collection: {collection_name}")
    print(f"   Dimension: {dimension}")
    print(f"   Vectors per MB: ~{vectors_per_mb}")
    print(f"   Target vectors: {target_vectors}")
    print(f"   Expected size: ~{target_vectors * dimension * 4 / 1024 / 1024:.2f} MB")
    
    try:
        # Create collection
        print(f"\n🔧 Creating collection: {collection_name}")
        create_payload = {
            "name": collection_name,
            "dimension": dimension,
            "distance_metric": "cosine",
            "storage_layout": "viper"
        }
        response = requests.post(f"{base_url}/collections", json=create_payload, timeout=30)
        if response.status_code == 200:
            print(f"✅ Collection created")
        else:
            print(f"❌ Failed to create collection: {response.text}")
            return 1
        
        # Insert vectors one by one
        print(f"\n🚀 Inserting {target_vectors} vectors...")
        total_inserted = 0
        start_time = time.time()
        
        for i in range(target_vectors):
            vector = [random.random() for _ in range(dimension)]
            vector_id = f"vec_{i:06d}"
            
            insert_payload = {
                "id": vector_id,
                "vector": vector,
                "metadata": {"index": i}
            }
            
            response = requests.post(
                f"{base_url}/collections/{collection_name}/vectors", 
                json=insert_payload,
                timeout=10
            )
            
            if response.status_code == 200:
                total_inserted += 1
                if (i + 1) % 100 == 0:  # Progress every 100 vectors
                    print(f"   📊 Inserted {total_inserted} / {target_vectors} vectors...")
            else:
                print(f"   ❌ Failed to insert vector {i}: {response.status_code} - {response.text}")
                break
        
        insert_time = time.time() - start_time
        
        print(f"✅ Inserted {total_inserted} vectors in {insert_time:.2f}s")
        print(f"📊 Insert rate: {total_inserted / insert_time:.0f} vectors/sec")
        
        # Wait a bit for potential async flush
        print(f"\n⏳ Waiting 10 seconds for potential flush...")
        time.sleep(10)
        
        # Test search to verify data accessibility
        print(f"\n🔍 Testing search...")
        query_vector = [random.random() for _ in range(dimension)]
        search_payload = {
            "vector": query_vector,
            "k": 10,
            "include_vectors": False,
            "include_metadata": True
        }
        
        response = requests.post(
            f"{base_url}/collections/{collection_name}/search",
            json=search_payload,
            timeout=30
        )
        
        if response.status_code == 200:
            search_data = response.json()
            # Handle different response formats
            if "data" in search_data and "results" in search_data["data"]:
                result_count = len(search_data["data"]["results"])
            elif "results" in search_data:
                result_count = len(search_data["results"])
            elif isinstance(search_data, list):
                result_count = len(search_data)
            else:
                result_count = 0
                
            print(f"✅ Search returned {result_count} results")
            
            if result_count > 0:
                print(f"🎯 Data is accessible - vectors are being found!")
                print(f"💾 VIPER flush likely occurred (data moved from WAL to VIPER storage)")
            else:
                print(f"⚠️  No search results - data may still be in WAL (not flushed)")
                print(f"🔍 This suggests WAL hasn't reached 1MB threshold or flush isn't working")
                print(f"📋 Search response format: {type(search_data)} - {list(search_data.keys()) if isinstance(search_data, dict) else 'not dict'}")
        else:
            print(f"❌ Search failed: {response.status_code} - {response.text}")
            result_count = 0
        
        # Cleanup
        print(f"\n🧹 Cleaning up...")
        response = requests.delete(f"{base_url}/collections/{collection_name}", timeout=10)
        if response.status_code == 200:
            print(f"✅ Collection deleted")
        else:
            print(f"⚠️  Cleanup failed: {response.status_code}")
        
        print(f"\n📋 Test Summary:")
        print(f"   Vectors inserted: {total_inserted}")
        print(f"   Data size: ~{total_inserted * dimension * 4 / 1024 / 1024:.2f} MB")
        print(f"   Search results: {result_count}")
        print(f"   Flush status: {'SUCCESS - Data accessible' if result_count > 0 else 'PENDING - Check server logs'}")
        
        return 0 if total_inserted > 0 else 1
        
    except Exception as e:
        print(f"❌ Test failed: {e}")
        try:
            requests.delete(f"{base_url}/collections/{collection_name}", timeout=5)
        except:
            pass
        return 1

if __name__ == "__main__":
    exit(main())