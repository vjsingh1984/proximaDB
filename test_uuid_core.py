#!/usr/bin/env python3
"""Core test to verify UUID-based collection operations work with ProximaDB Python SDK."""

from proximadb import ProximaDBClient

def test_uuid_core_operations():
    """Test core UUID operations that matter most."""
    print("Testing Core UUID Operations with ProximaDB Python SDK")
    print("=" * 55)
    
    # Test REST client
    client = ProximaDBClient("http://localhost:5678")
    print("✅ Connected to ProximaDB (REST)")
    
    # Create a collection
    import time
    collection_name = f"uuid-core-test-{int(time.time())}"
    collection = client.create_collection(collection_name, dimension=384)
    print(f"✅ Created collection: {collection_name}")
    
    # Get the collection UUID
    collection_info = client.get_collection(collection_name)
    uuid = collection_info.id
    print(f"✅ Retrieved UUID: {uuid}")
    
    # Test 1: Get collection by UUID (CRITICAL TEST)
    print(f"\n1️⃣ Testing get_collection with UUID...")
    try:
        collection_by_uuid = client.get_collection(uuid)
        if collection_by_uuid and collection_by_uuid.name == collection_name and collection_by_uuid.id == uuid:
            print(f"✅ Successfully retrieved collection by UUID")
            print(f"   Name: {collection_by_uuid.name}")
            print(f"   UUID: {collection_by_uuid.id}")
        else:
            print(f"❌ Failed to retrieve collection by UUID")
            return False
    except Exception as e:
        print(f"❌ Error retrieving by UUID: {e}")
        return False
    
    # Test 2: Delete collection by UUID (CRITICAL TEST)
    print(f"\n2️⃣ Testing delete_collection with UUID...")
    try:
        client.delete_collection(uuid)  # Using UUID instead of name
        print(f"✅ Successfully deleted collection by UUID")
    except Exception as e:
        print(f"❌ Error deleting collection by UUID: {e}")
        return False
    
    # Test 3: Verify deletion worked
    print(f"\n3️⃣ Verifying collection deletion...")
    try:
        client.get_collection(uuid)
        print(f"❌ Collection should have been deleted but still exists")
        return False
    except Exception:
        print(f"✅ Confirmed collection was deleted by UUID")
    
    # Test by name should also fail
    try:
        client.get_collection(collection_name)
        print(f"❌ Collection should have been deleted (name check failed)")
        return False
    except Exception:
        print(f"✅ Confirmed collection was deleted (name check passed)")
    
    print(f"\n🎉 All core UUID operations PASSED!")
    print(f"Key achievements:")
    print(f"  ✅ Collection retrieval by UUID works")
    print(f"  ✅ Collection deletion by UUID works")
    print(f"  ✅ Both REST and backend UUID resolution working")
    return True

if __name__ == "__main__":
    success = test_uuid_core_operations()
    exit(0 if success else 1)