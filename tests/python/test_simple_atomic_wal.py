#!/usr/bin/env python3
"""
Simple Atomic WAL Test using direct HTTP requests
"""

import json
import requests
import numpy as np

def test_atomic_wal():
    """Test atomic WAL with direct HTTP requests"""
    
    print("🧪 Simple Atomic WAL Test")
    print("=" * 40)
    
    base_url = "http://localhost:5678"
    
    # Test 1: Health check
    print("\n💊 Health check...")
    try:
        response = requests.get(f"{base_url}/health")
        print(f"✅ Health: {response.status_code} - {response.text}")
    except Exception as e:
        print(f"❌ Health check failed: {e}")
        return False
    
    # Test 2: Collection creation (try different endpoints)
    collection_id = "simple_atomic_test"
    
    # Try standard REST collection creation
    print(f"\n📁 Creating collection: {collection_id}")
    
    collection_data = {
        "name": collection_id,
        "dimension": 384,
        "distance_metric": "cosine",
        "storage_engine": "viper"
    }
    
    # Try different possible endpoints
    endpoints_to_try = [
        "/collections",           # Standard
        "/api/v1/collections",   # Versioned API  
        "/api/collections",      # API prefix
    ]
    
    collection_created = False
    for endpoint in endpoints_to_try:
        try:
            print(f"   Trying: POST {endpoint}")
            response = requests.post(
                f"{base_url}{endpoint}",
                json=collection_data,
                headers={"Content-Type": "application/json"}
            )
            print(f"   Response: {response.status_code} - {response.text[:200]}")
            
            if response.status_code in [200, 201]:
                collection_created = True
                print(f"✅ Collection created via {endpoint}")
                break
                
        except Exception as e:
            print(f"   Error: {e}")
    
    if not collection_created:
        print("❌ Failed to create collection via any endpoint")
        
        # Let's see what endpoints are available
        print("\n🔍 Checking available endpoints...")
        try:
            # Try to get some info about available routes
            response = requests.get(f"{base_url}/")
            print(f"Root: {response.status_code} - {response.text[:200]}")
        except:
            pass
            
        return False
    
    # Test 3: Vector insertion (if collection was created)
    print(f"\n🔥 Testing vector insertion...")
    
    vector_data = {
        "id": "atomic_test_vector_1",
        "vector": np.random.random(384).astype(np.float32).tolist(),
        "metadata": {"test": "atomic", "timestamp": "2025-07-03"}
    }
    
    # Try different vector insertion endpoints
    vector_endpoints = [
        f"/collections/{collection_id}/vectors",
        f"/api/v1/collections/{collection_id}/vectors",
        f"/api/collections/{collection_id}/vectors",
    ]
    
    vector_inserted = False
    for endpoint in vector_endpoints:
        try:
            print(f"   Trying: POST {endpoint}")
            response = requests.post(
                f"{base_url}{endpoint}",
                json=vector_data,
                headers={"Content-Type": "application/json"}
            )
            print(f"   Response: {response.status_code} - {response.text[:200]}")
            
            if response.status_code in [200, 201]:
                vector_inserted = True
                print(f"✅ Vector inserted via {endpoint}")
                break
                
        except Exception as e:
            print(f"   Error: {e}")
    
    if vector_inserted:
        print("✅ Atomic WAL test basic operations successful!")
    else:
        print("⚠️ Vector insertion failed, but collection creation worked")
    
    return collection_created


def check_wal_logs():
    """Check for WAL-related log messages"""
    
    print(f"\n📋 Checking WAL logs...")
    
    try:
        with open('server_atomic_test.log', 'r') as f:
            logs = f.read()
        
        # Look for WAL-related messages
        wal_keywords = [
            "WAL",
            "memtable", 
            "disk write",
            "atomic write",
            "PerBatch",
            "Avro"
        ]
        
        found_logs = []
        for keyword in wal_keywords:
            if keyword in logs:
                found_logs.append(keyword)
        
        print(f"📊 Found WAL-related keywords: {found_logs}")
        
        # Look for specific atomic messages
        if "atomic write" in logs:
            print("✅ Found atomic write messages in logs")
        else:
            print("⚠️ No atomic write messages found")
        
    except Exception as e:
        print(f"❌ Error checking logs: {e}")


if __name__ == "__main__":
    success = test_atomic_wal()
    check_wal_logs()
    
    if success:
        print(f"\n🎉 Test completed!")
    else:
        print(f"\n💥 Test failed!")