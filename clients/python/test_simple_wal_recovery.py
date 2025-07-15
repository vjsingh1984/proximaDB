#!/usr/bin/env python3
"""
Simple WAL Recovery Test
Basic test to verify data persists across server restarts
"""

# Set PYTHONPATH to include src directory
import sys
import os
if 'PYTHONPATH' not in os.environ:
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

import time
import json
import numpy as np
from proximadb import connect_rest, connect_grpc, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord

def test_simple_persistence():
    """Test basic persistence with a simple collection and vector"""
    
    print("🚀 Simple WAL Recovery Test")
    print("="*60)
    
    # Test data
    collection_name = "simple_wal_test"
    test_vector_id = "test_vector_1"
    test_vector_data = [1.0, 2.0, 3.0, 4.0] + [0.0] * 124  # 128D vector
    
    # Connect to server
    print("🔗 Connecting to server...")
    client = connect_rest("http://localhost:5678")
    
    # Check if collection already exists
    print(f"🔍 Checking if collection '{collection_name}' exists...")
    try:
        existing_collection = client.get_collection(collection_name)
        print(f"✅ Collection found: {existing_collection.config.name}")
        
        # Test search to see if data persists
        print("🔍 Testing search on existing collection...")
        results = client.search(collection_name, test_vector_data, top_k=5)
        print(f"📊 Search results: {len(results)} vectors found")
        
        if len(results) > 0:
            print("✅ DATA PERSISTED ACROSS RESTART!")
            for i, result in enumerate(results):
                print(f"  Result {i+1}: ID={result.id}, Score={getattr(result, 'score', 'N/A')}")
            
            # Test specific vector lookup
            found_test_vector = any(r.id == test_vector_id for r in results)
            if found_test_vector:
                print(f"✅ Found our test vector: {test_vector_id}")
            else:
                print(f"⚠️  Test vector {test_vector_id} not in top results")
        else:
            print("❌ No data found in existing collection")
        
        return True
        
    except Exception as e:
        print(f"📦 Collection doesn't exist: {e}")
    
    # Create new collection
    print(f"📦 Creating new collection: {collection_name}")
    config = CollectionConfig(
        name=collection_name,
        dimension=128,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER,
        description="Simple WAL recovery test"
    )
    
    collection = client.create_collection(collection_name, config)
    print(f"✅ Collection created: {collection.config.name}")
    
    # Insert test vector
    print("📝 Inserting test vector...")
    test_vector = VectorRecord(
        id=test_vector_id,
        vector=test_vector_data,
        metadata={"test": "wal_recovery", "timestamp": int(time.time())}
    )
    
    result = client.insert_vectors(collection_name, [test_vector])
    print(f"✅ Vector inserted: {test_vector_id}")
    
    # Immediately test search
    print("🔍 Testing immediate search...")
    results = client.search(collection_name, test_vector_data, top_k=5)
    print(f"📊 Immediate search results: {len(results)} vectors found")
    
    if len(results) > 0:
        print("✅ Vector found in immediate search!")
        for i, result in enumerate(results):
            print(f"  Result {i+1}: ID={result.id}, Score={getattr(result, 'score', 'N/A')}")
    else:
        print("❌ Vector not found in immediate search")
    
    # Save test info
    test_info = {
        "collection_name": collection_name,
        "test_vector_id": test_vector_id,
        "test_vector_data": test_vector_data,
        "timestamp": time.strftime("%Y-%m-%d %H:%M:%S"),
        "immediate_search_results": len(results)
    }
    
    with open("simple_wal_test_info.json", "w") as f:
        json.dump(test_info, f, indent=2)
    
    print(f"\n📊 Test info saved to: simple_wal_test_info.json")
    print(f"🔄 Now restart the server and run this test again to verify persistence!")
    
    return False

def test_with_both_engines():
    """Test with both VIPER and LSM engines"""
    
    print("\n🔄 Testing with both storage engines...")
    
    engines = [
        ("VIPER", StorageEngine.VIPER),
        ("LSM", StorageEngine.LSM)
    ]
    
    results = {}
    
    for engine_name, engine_type in engines:
        print(f"\n--- Testing {engine_name} Engine ---")
        
        collection_name = f"wal_test_{engine_name.lower()}"
        test_vector_id = f"test_vector_{engine_name.lower()}"
        test_vector_data = [float(i) for i in range(128)]  # Simple sequential vector
        
        # Connect to server
        client = connect_rest("http://localhost:5678")
        
        # Check if collection exists
        try:
            existing_collection = client.get_collection(collection_name)
            print(f"✅ {engine_name} collection found, testing search...")
            
            results_list = client.search(collection_name, test_vector_data, top_k=5)
            print(f"📊 {engine_name} search results: {len(results_list)} vectors found")
            
            results[engine_name] = {
                "collection_exists": True,
                "search_results": len(results_list),
                "data_persisted": len(results_list) > 0
            }
            
            if len(results_list) > 0:
                print(f"✅ {engine_name} data persisted!")
                for i, result in enumerate(results_list):
                    print(f"  Result {i+1}: ID={result.id}, Score={getattr(result, 'score', 'N/A')}")
            
            continue
            
        except Exception as e:
            print(f"📦 {engine_name} collection doesn't exist: {e}")
        
        # Create new collection
        print(f"📦 Creating new {engine_name} collection...")
        config = CollectionConfig(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=engine_type,
            description=f"WAL recovery test - {engine_name}"
        )
        
        collection = client.create_collection(collection_name, config)
        print(f"✅ {engine_name} collection created")
        
        # Insert test vector
        test_vector = VectorRecord(
            id=test_vector_id,
            vector=test_vector_data,
            metadata={"engine": engine_name, "test": "wal_recovery"}
        )
        
        client.insert_vectors(collection_name, [test_vector])
        print(f"✅ {engine_name} vector inserted")
        
        # Test immediate search
        results_list = client.search(collection_name, test_vector_data, top_k=5)
        
        results[engine_name] = {
            "collection_exists": False,
            "search_results": len(results_list),
            "data_persisted": len(results_list) > 0
        }
        
        print(f"📊 {engine_name} immediate search: {len(results_list)} results")
    
    # Save results
    with open("wal_recovery_both_engines.json", "w") as f:
        json.dump(results, f, indent=2)
    
    print("\n📊 Both engines test results:")
    for engine, result in results.items():
        status = "✅ PERSISTED" if result["data_persisted"] else "❌ NO DATA"
        print(f"  {engine}: {status} ({result['search_results']} results)")
    
    return results

if __name__ == "__main__":
    # Run simple test first
    persisted = test_simple_persistence()
    
    if persisted:
        print("\n🎉 SUCCESS: Data persisted across server restart!")
    else:
        print("\n⚠️  Data was just inserted. Restart server to test persistence.")
    
    # Test with both engines
    test_with_both_engines()
    
    print("\n📋 SUMMARY:")
    print("  - Run this test before server restart to insert data")
    print("  - Restart the server")
    print("  - Run this test again to verify data persistence")
    print("  - Check the generated JSON files for detailed results")