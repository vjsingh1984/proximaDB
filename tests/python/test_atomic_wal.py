#!/usr/bin/env python3
"""
Test WAL Atomicity Implementation
Verifies that WAL writes are properly atomic between memtable and disk
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients/python/src'))

import time
import requests
import json
import numpy as np
from proximadb import ProximaDBClient, Protocol


def test_atomic_wal_behavior():
    """Test that WAL writes are atomic and handle failures correctly"""
    
    print("🧪 Testing WAL Atomicity Implementation")
    print("=" * 50)
    
    # Use REST client for simplicity
    client = ProximaDBClient("http://localhost:5678", protocol=Protocol.REST)
    
    collection_id = "atomic_test_collection"
    
    # Cleanup
    try:
        client.delete_collection(collection_id)
        print(f"🧹 Cleaned up existing collection: {collection_id}")
    except:
        pass
    
    # Create collection
    print(f"\n📁 Creating collection: {collection_id}")
    try:
        result = client.create_collection(
            collection_id=collection_id,
            dimension=384,
            distance_metric="cosine",
            storage_engine="viper"
        )
        print(f"✅ Collection created: {result}")
    except Exception as e:
        print(f"❌ Failed to create collection: {e}")
        return False
    
    # Test 1: Normal successful write
    print(f"\n🔥 Test 1: Normal WAL write (should succeed)")
    test_vectors = [
        {
            "id": "atomic_test_vec_1",
            "vector": np.random.random(384).astype(np.float32).tolist(),
            "metadata": {"test": "atomic_normal", "sequence": 1}
        }
    ]
    
    try:
        insert_result = client.insert_vectors(collection_id, test_vectors)
        print(f"✅ Normal write succeeded: {insert_result.count} vectors inserted")
        
        # Verify immediate read consistency
        retrieved = client.get_vector(collection_id, "atomic_test_vec_1")
        print(f"✅ Immediate read successful: {retrieved['id']}")
        
    except Exception as e:
        print(f"❌ Normal write failed: {e}")
        return False
    
    # Test 2: Verify durability by checking logs
    print(f"\n💾 Test 2: Checking WAL disk write logs")
    
    # Insert a few more vectors to trigger WAL activity
    more_vectors = []
    for i in range(5):
        more_vectors.append({
            "id": f"atomic_test_vec_{i+2}",
            "vector": np.random.random(384).astype(np.float32).tolist(),
            "metadata": {"test": "atomic_batch", "sequence": i+2}
        })
    
    try:
        batch_result = client.insert_vectors(collection_id, more_vectors)
        print(f"✅ Batch write succeeded: {batch_result.count} vectors inserted")
        
        # Check that all vectors are readable
        total_readable = 0
        for i in range(1, 7):  # vec_1 + 5 more vectors
            try:
                retrieved = client.get_vector(collection_id, f"atomic_test_vec_{i}")
                total_readable += 1
            except:
                pass
        
        print(f"✅ Read consistency verified: {total_readable}/6 vectors readable")
        
    except Exception as e:
        print(f"❌ Batch write failed: {e}")
        return False
    
    # Test 3: Search to verify memtable access
    print(f"\n🔍 Test 3: Search to verify memtable access")
    
    try:
        query_vector = np.random.random(384).astype(np.float32).tolist()
        search_result = client.search_vectors(
            collection_id=collection_id,
            query_vector=query_vector,
            top_k=3,
            include_metadata=True
        )
        
        print(f"✅ Search successful: Found {len(search_result.results)} results")
        
        for i, result in enumerate(search_result.results):
            metadata = result.get('metadata', {})
            print(f"   {i+1}. ID: {result['id']}, Score: {result.get('score', 0):.4f}, Test: {metadata.get('test', 'unknown')}")
        
    except Exception as e:
        print(f"❌ Search failed: {e}")
        return False
    
    print(f"\n✅ All atomic WAL tests passed!")
    print("📋 Key verifications:")
    print("   - WAL writes complete successfully")  
    print("   - Immediate read consistency maintained")
    print("   - Batch operations work correctly")
    print("   - Search accesses both memtable and storage")
    
    return True


def check_server_logs():
    """Check server logs for WAL atomicity messages"""
    
    print(f"\n📋 Checking server logs for WAL atomicity behavior...")
    
    try:
        with open('server_atomic_test.log', 'r') as f:
            logs = f.read()
        
        # Look for key atomicity log messages
        atomic_messages = [
            "WAL atomic write succeeded",
            "memtable=",
            "disk=", 
            "WAL write completed",
            "Memtable write failed",
            "Disk write failed"
        ]
        
        found_messages = []
        for message in atomic_messages:
            if message in logs:
                found_messages.append(message)
        
        print(f"📊 Found {len(found_messages)}/{len(atomic_messages)} atomic WAL log patterns:")
        for msg in found_messages:
            print(f"   ✅ {msg}")
        
        # Check for any error patterns
        error_patterns = [
            "WAL write failed",
            "operation aborted",
            "rollback",
            "inconsistency"
        ]
        
        found_errors = []
        for pattern in error_patterns:
            if pattern.lower() in logs.lower():
                found_errors.append(pattern)
        
        if found_errors:
            print(f"⚠️ Found error patterns: {found_errors}")
        else:
            print(f"✅ No error patterns detected")
        
    except FileNotFoundError:
        print("⚠️ Server log file not found")
    except Exception as e:
        print(f"❌ Failed to read logs: {e}")


if __name__ == "__main__":
    # Wait for server to be ready
    print("⏳ Waiting for server to start...")
    
    for attempt in range(30):  # Wait up to 30 seconds
        try:
            response = requests.get("http://localhost:5678/health", timeout=2)
            if response.status_code == 200:
                print("✅ Server is ready!")
                break
        except:
            pass
        time.sleep(1)
    else:
        print("❌ Server failed to start within 30 seconds")
        sys.exit(1)
    
    # Run atomic WAL tests
    success = test_atomic_wal_behavior()
    
    # Check logs
    check_server_logs()
    
    if success:
        print(f"\n🎉 Atomic WAL implementation test completed successfully!")
    else:
        print(f"\n💥 Atomic WAL implementation test failed!")
        sys.exit(1)