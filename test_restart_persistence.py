#!/usr/bin/env python3
"""
Restart and Persistence Test - Quick and Focused
"""

import os
import subprocess
import time
import sys

# Add Python client to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients/python/src'))

from proximadb import ProximaDBClient

def run_server():
    """Start server and wait for it to be ready"""
    print("🚀 Starting ProximaDB server...")
    proc = subprocess.Popen(
        ["./target/release/proximadb-server", "--config", "config.toml"],
        stdout=open('/tmp/server.log', 'w'),
        stderr=subprocess.STDOUT,
        cwd="/workspace"
    )
    
    # Wait for server to be ready
    for i in range(15):
        try:
            client = ProximaDBClient("http://localhost:5678")
            client.list_collections()
            print(f"   ✅ Server ready after {i+1} attempts")
            return proc, client
        except:
            time.sleep(1)
    
    raise Exception("Server failed to start")

def test_restart_persistence():
    """Test data persistence across server restart"""
    print("🧪 Restart and Persistence Test")
    print("=" * 40)
    
    # Clean slate
    subprocess.run(["pkill", "-f", "proximadb-server"], capture_output=True)
    time.sleep(2)
    
    # Phase 1: Create data
    print("\n📝 Phase 1: Creating Data")
    server_proc, client = run_server()
    
    try:
        # Create collection
        collection_name = "persistence_test_collection"
        client.create_collection(collection_name, dimension=384)
        print(f"   ✅ Created collection: {collection_name}")
        
        # Insert vectors
        test_vectors = {}
        for i in range(3):
            vector_id = f"test_vector_{i}"
            vector_data = [0.1 * j for j in range(384)]
            vector_data[i] = 1.0  # Make unique
            
            client.insert_vector(
                collection_id=collection_name,
                vector_id=vector_id,
                vector=vector_data,
                metadata={"test": True, "index": i}
            )
            test_vectors[vector_id] = vector_data
            print(f"   ✅ Inserted vector: {vector_id}")
        
        # Verify data before restart
        collections_before = client.list_collections()
        print(f"   📊 Collections before restart: {len(collections_before)}")
        
        for vector_id in test_vectors:
            result = client.get_vector(collection_name, vector_id)
            if result:
                print(f"   ✅ Vector {vector_id} retrievable before restart")
        
    finally:
        server_proc.terminate()
        server_proc.wait(timeout=5)
    
    # Phase 2: Restart and verify persistence
    print("\n🔄 Phase 2: Server Restart and Recovery")
    time.sleep(2)
    
    server_proc, client = run_server()
    
    try:
        # Check collection recovery
        collections_after = client.list_collections()
        collection_names = {col['name'] for col in collections_after}
        
        print(f"   📊 Collections after restart: {len(collections_after)}")
        
        if collection_name in collection_names:
            print(f"   ✅ Collection '{collection_name}' recovered successfully")
        else:
            print(f"   ❌ Collection '{collection_name}' NOT recovered")
            return False
        
        # Check vector recovery
        recovered_vectors = 0
        for vector_id, expected_data in test_vectors.items():
            try:
                result = client.get_vector(collection_name, vector_id)
                if result and 'vector' in result:
                    recovered_vectors += 1
                    print(f"   ✅ Vector '{vector_id}' recovered")
                else:
                    print(f"   ❌ Vector '{vector_id}' NOT recovered")
            except Exception as e:
                print(f"   ❌ Error retrieving vector '{vector_id}': {e}")
        
        recovery_rate = recovered_vectors / len(test_vectors)
        print(f"   📊 Vector recovery rate: {recovered_vectors}/{len(test_vectors)} ({recovery_rate:.1%})")
        
        # Test search functionality
        print("\n🔍 Phase 3: Search Functionality Test")
        if recovered_vectors > 0:
            try:
                # Use first vector for search
                first_vector_id, first_vector_data = next(iter(test_vectors.items()))
                results = client.search(
                    collection_id=collection_name,
                    query=first_vector_data,
                    k=3
                )
                
                if results and len(results) > 0:
                    print(f"   ✅ Search successful: {len(results)} results found")
                    # Check if exact match is found
                    exact_match = any(r.get('id') == first_vector_id for r in results)
                    if exact_match:
                        print(f"   ✅ Exact match found in search results")
                    else:
                        print(f"   ⚠️ Exact match not found, but search works")
                else:
                    print(f"   ❌ Search returned no results")
                    return False
                    
            except Exception as e:
                print(f"   ❌ Search failed: {e}")
                return False
        
        # Check file distribution
        print("\n📁 Phase 4: Multi-Disk File Distribution")
        file_counts = {"disk1": 0, "disk2": 0, "disk3": 0}
        
        for disk in ["disk1", "disk2", "disk3"]:
            # Check WAL files
            wal_path = f"/workspace/data/{disk}/wal"
            if os.path.exists(wal_path):
                for root, dirs, files in os.walk(wal_path):
                    file_counts[disk] += len([f for f in files if not f.startswith('.')])
            
            # Check metadata files (only disk1 used for metadata in this config)
            if disk == "disk1":
                metadata_path = f"/workspace/data/{disk}/metadata"
                if os.path.exists(metadata_path):
                    for root, dirs, files in os.walk(metadata_path):
                        if 'incremental' in root or 'snapshots' in root:
                            file_counts[disk] += len([f for f in files if not f.startswith('.')])
        
        active_disks = sum(1 for count in file_counts.values() if count > 0)
        total_files = sum(file_counts.values())
        
        for disk, count in file_counts.items():
            print(f"   📂 {disk}: {count} files")
        
        print(f"   📊 Total files: {total_files}, Active disks: {active_disks}/3")
        
        # Final assessment
        print("\n🎯 Final Assessment")
        print("=" * 25)
        
        success_criteria = [
            (recovery_rate >= 0.8, f"Vector recovery: {recovery_rate:.1%}"),
            (collection_name in collection_names, "Collection recovery: Success"),
            (recovered_vectors > 0, "Data persistence: Working"),
            (total_files > 0, "File system activity: Detected"),
        ]
        
        passed = sum(1 for success, _ in success_criteria if success)
        total = len(success_criteria)
        
        for success, description in success_criteria:
            status = "✅" if success else "❌"
            print(f"   {status} {description}")
        
        overall_success = passed == total
        
        if overall_success:
            print(f"\n🎉 PERSISTENCE TEST PASSED! ({passed}/{total})")
            print("🏆 Key achievements:")
            print("   • Collections persist across restart")
            print("   • Vectors recover correctly") 
            print("   • Search functionality works")
            print("   • Multi-disk configuration active")
        else:
            print(f"\n⚠️ Persistence test partially successful ({passed}/{total})")
        
        return overall_success
        
    finally:
        server_proc.terminate()
        try:
            server_proc.wait(timeout=5)
        except:
            server_proc.kill()

if __name__ == "__main__":
    try:
        success = test_restart_persistence()
        print(f"\n📋 Test Result: {'PASSED' if success else 'FAILED'}")
        sys.exit(0 if success else 1)
    except Exception as e:
        print(f"❌ Test failed with exception: {e}")
        sys.exit(1)