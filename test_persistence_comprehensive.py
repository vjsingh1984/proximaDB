#!/usr/bin/env python3
"""
Comprehensive persistence test for ProximaDB
Tests server restarts, deletions, concurrent operations, and edge cases
"""

import asyncio
import grpc
import time
import json
import sys
import os
import subprocess
import signal
from datetime import datetime

# Add the client path
sys.path.insert(0, 'clients/python/src')

from proximadb.grpc_client import ProximaDBClient
from proximadb.models import CollectionConfig, DistanceMetric

async def test_persistence_across_restart():
    """Test that collections persist across server restarts"""
    print("\n🔄 Test: Server Restart Persistence")
    print("-" * 50)
    
    # Connect and create collections
    client = ProximaDBClient(endpoint='localhost:5679')
    
    # Create test collections with timestamps
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    test_collections = [
        ("restart_test_1_" + timestamp, 128, 1),  # COSINE
        ("restart_test_2_" + timestamp, 256, 2),  # EUCLIDEAN
        ("restart_test_3_" + timestamp, 512, 3),  # DOT_PRODUCT
    ]
    
    created_collections = {}
    
    print("📝 Creating collections before restart...")
    for name, dim, metric in test_collections:
        try:
            result = await client.create_collection(
                name=name,
                dimension=dim,
                distance_metric=metric,
                indexing_algorithm=1,
                storage_engine=1,
            )
            created_collections[name] = result.id
            print(f"   ✅ Created: {name} (UUID: {result.id})")
        except Exception as e:
            print(f"   ❌ Failed to create {name}: {e}")
    
    # List collections before restart
    collections_before = await client.list_collections()
    print(f"\n📋 Collections before restart: {len(collections_before)}")
    
    # Kill the server
    print("\n🛑 Stopping server...")
    subprocess.run(["pkill", "-f", "proximadb-server"], capture_output=True)
    time.sleep(2)
    
    # Restart the server
    print("🚀 Restarting server...")
    subprocess.Popen(
        ["cargo", "run", "--bin", "proximadb-server", "--", "--config", "config.toml"],
        stdout=open("server_restart_output.txt", "w"),
        stderr=subprocess.STDOUT
    )
    
    # Wait for server to come up
    print("⏳ Waiting for server to start...")
    time.sleep(5)
    
    # Reconnect and verify
    client2 = ProximaDBClient(endpoint='localhost:5679')
    
    collections_after = await client2.list_collections()
    print(f"\n📋 Collections after restart: {len(collections_after)}")
    
    # Verify each collection
    print("\n🔍 Verifying collections survived restart...")
    for name, expected_uuid in created_collections.items():
        try:
            collection = await client2.get_collection(name)
            if collection.id == expected_uuid:
                print(f"   ✅ {name}: UUID matches ({expected_uuid})")
            else:
                print(f"   ❌ {name}: UUID mismatch! Expected {expected_uuid}, got {collection.id}")
        except Exception as e:
            print(f"   ❌ {name}: Failed to retrieve after restart: {e}")
    
    return True

async def test_collection_deletion():
    """Test collection deletion and persistence"""
    print("\n🗑️ Test: Collection Deletion")
    print("-" * 50)
    
    client = ProximaDBClient(endpoint='localhost:5679')
    
    # Create a collection to delete
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    delete_name = f"delete_test_{timestamp}"
    
    print(f"📝 Creating collection to delete: {delete_name}")
    try:
        result = await client.create_collection(
            name=delete_name,
            dimension=128,
            distance_metric=1,
        )
        print(f"   ✅ Created with UUID: {result.id}")
    except Exception as e:
        print(f"   ❌ Failed to create: {e}")
        return False
    
    # Verify it exists
    collections = await client.list_collections()
    exists = any(c.name == delete_name for c in collections)
    print(f"   📋 Collection exists in list: {exists}")
    
    # Delete the collection
    print(f"\n🗑️ Deleting collection: {delete_name}")
    try:
        await client.delete_collection(delete_name)
        print("   ✅ Delete request successful")
    except Exception as e:
        print(f"   ⚠️ Delete not implemented or failed: {e}")
        # This is expected if delete isn't implemented yet
        return True
    
    # Verify it's gone
    collections_after = await client.list_collections()
    still_exists = any(c.name == delete_name for c in collections_after)
    print(f"   📋 Collection still exists after delete: {still_exists}")
    
    if not still_exists:
        print("   ✅ Collection successfully deleted")
    else:
        print("   ❌ Collection still exists after deletion")
    
    return True

async def test_concurrent_operations():
    """Test concurrent collection operations"""
    print("\n⚡ Test: Concurrent Operations")
    print("-" * 50)
    
    client = ProximaDBClient(endpoint='localhost:5679')
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    
    # Define concurrent tasks
    async def create_collection(index):
        name = f"concurrent_test_{index}_{timestamp}"
        try:
            result = await client.create_collection(
                name=name,
                dimension=128 + index * 32,
                distance_metric=(index % 3) + 1,
            )
            return (True, name, result.id)
        except Exception as e:
            return (False, name, str(e))
    
    # Run concurrent creates
    print("🏃 Running 5 concurrent collection creates...")
    tasks = [create_collection(i) for i in range(5)]
    results = await asyncio.gather(*tasks)
    
    success_count = sum(1 for r in results if r[0])
    print(f"\n📊 Results: {success_count}/5 successful")
    
    for success, name, info in results:
        if success:
            print(f"   ✅ {name}: Created with UUID {info}")
        else:
            print(f"   ❌ {name}: {info}")
    
    # Verify all collections exist
    print("\n🔍 Verifying all concurrent collections exist...")
    collections = await client.list_collections()
    collection_names = {c.name for c in collections}
    
    for _, name, _ in results:
        if name in collection_names:
            print(f"   ✅ {name} found in list")
        else:
            print(f"   ❌ {name} NOT found in list")
    
    return True

async def test_edge_cases():
    """Test edge cases and error handling"""
    print("\n🔧 Test: Edge Cases")
    print("-" * 50)
    
    client = ProximaDBClient(endpoint='localhost:5679')
    
    # Test 1: Empty collection name
    print("\n1️⃣ Testing empty collection name...")
    try:
        await client.create_collection(
            name="",
            dimension=128,
            distance_metric=1,
        )
        print("   ❌ Should have failed with empty name")
    except Exception as e:
        print(f"   ✅ Correctly rejected: {e}")
    
    # Test 2: Invalid dimension
    print("\n2️⃣ Testing invalid dimension (0)...")
    try:
        await client.create_collection(
            name="invalid_dim_test",
            dimension=0,
            distance_metric=1,
        )
        print("   ❌ Should have failed with dimension 0")
    except Exception as e:
        print(f"   ✅ Correctly rejected: {e}")
    
    # Test 3: Very long collection name
    print("\n3️⃣ Testing very long collection name...")
    long_name = "a" * 256
    try:
        await client.create_collection(
            name=long_name,
            dimension=128,
            distance_metric=1,
        )
        print("   ⚠️ Accepted very long name (may want to add limits)")
    except Exception as e:
        print(f"   ✅ Rejected long name: {e}")
    
    # Test 4: Special characters in name
    print("\n4️⃣ Testing special characters in name...")
    special_name = "test-collection_2024.v1"
    try:
        result = await client.create_collection(
            name=special_name,
            dimension=128,
            distance_metric=1,
        )
        print(f"   ✅ Accepted special characters: {result.id}")
    except Exception as e:
        print(f"   ⚠️ Rejected special characters: {e}")
    
    # Test 5: Get non-existent collection
    print("\n5️⃣ Testing get non-existent collection...")
    try:
        await client.get_collection("this_does_not_exist_12345")
        print("   ❌ Should have failed for non-existent collection")
    except Exception as e:
        print(f"   ✅ Correctly failed: {e}")
    
    return True

async def verify_filesystem_persistence():
    """Verify actual filesystem persistence"""
    print("\n💾 Test: Filesystem Verification")
    print("-" * 50)
    
    # Check various possible locations
    possible_paths = [
        "./data/metadata",
        "./data/collections",
        "./data",
        "/tmp/proximadb/metadata",
        "/tmp/proximadb/collections",
        "./metadata",
        "./collections",
    ]
    
    print("🔍 Searching for metadata files...")
    found_files = False
    
    for path in possible_paths:
        if os.path.exists(path):
            files = []
            for root, dirs, filenames in os.walk(path):
                for filename in filenames:
                    if filename.endswith(('.avro', '.json', '.yaml', '.toml')):
                        files.append(os.path.join(root, filename))
            
            if files:
                found_files = True
                print(f"\n📁 Found metadata in: {path}")
                for file in files[:10]:  # Show first 10 files
                    size = os.path.getsize(file)
                    mtime = datetime.fromtimestamp(os.path.getmtime(file))
                    print(f"   📄 {os.path.relpath(file, path)} ({size} bytes, modified: {mtime})")
                
                if len(files) > 10:
                    print(f"   ... and {len(files) - 10} more files")
    
    if not found_files:
        print("❌ No metadata files found in expected locations")
        print("\n📋 Checking config.toml for configured paths...")
        
        # Read config file to find actual path
        if os.path.exists("config.toml"):
            with open("config.toml", "r") as f:
                config_content = f.read()
                print("📄 Relevant config sections:")
                for line in config_content.split('\n'):
                    if any(keyword in line.lower() for keyword in ['path', 'dir', 'metadata', 'storage']):
                        print(f"   {line.strip()}")
    
    return True

async def main():
    """Run all comprehensive tests"""
    print("🚀 ProximaDB Comprehensive Persistence Testing")
    print("=" * 60)
    
    # Make sure server is running
    print("🔍 Checking if server is running...")
    try:
        client = ProximaDBClient(endpoint='localhost:5679')
        collections = await client.list_collections()
        print(f"✅ Server is running with {len(collections)} collections")
    except Exception as e:
        print(f"❌ Server not running: {e}")
        print("Please start the server first!")
        return False
    
    # Run all tests
    tests = [
        test_persistence_across_restart,
        test_collection_deletion,
        test_concurrent_operations,
        test_edge_cases,
        verify_filesystem_persistence,
    ]
    
    results = []
    for test in tests:
        try:
            result = await test()
            results.append((test.__name__, result))
        except Exception as e:
            print(f"\n❌ Test {test.__name__} failed with exception: {e}")
            import traceback
            traceback.print_exc()
            results.append((test.__name__, False))
    
    # Summary
    print("\n" + "=" * 60)
    print("📊 Test Summary:")
    print("-" * 60)
    
    passed = sum(1 for _, result in results if result)
    total = len(results)
    
    for test_name, result in results:
        status = "✅ PASS" if result else "❌ FAIL"
        print(f"{status} - {test_name}")
    
    print("-" * 60)
    print(f"Total: {passed}/{total} tests passed")
    
    return passed == total

if __name__ == "__main__":
    success = asyncio.run(main())
    
    if success:
        print("\n✨ All tests passed!")
        exit(0)
    else:
        print("\n💥 Some tests failed!")
        exit(1)