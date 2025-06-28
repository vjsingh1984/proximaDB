#!/usr/bin/env python3
"""Simple test to verify UUID-based operations work with ProximaDB Python SDK."""

from proximadb import ProximaDBClient
import numpy as np

def test_uuid_operations_simple():
    """Test basic UUID operations."""
    print("Testing UUID operations with ProximaDB Python SDK")
    print("=" * 50)
    
    # Test REST client
    client = ProximaDBClient("http://localhost:5678")
    print("✅ Connected to ProximaDB (REST)")
    
    # Create a collection
    import time
    collection_name = f"uuid-simple-test-{int(time.time())}"
    collection = client.create_collection(collection_name, dimension=384)
    print(f"✅ Created collection: {collection_name}")
    
    # Get the collection UUID
    collection_info = client.get_collection(collection_name)
    uuid = collection_info.id
    print(f"✅ Retrieved UUID: {uuid}")
    
    # Test 1: Get collection by UUID
    print(f"\n1️⃣ Testing get_collection with UUID...")
    try:
        collection_by_uuid = client.get_collection(uuid)
        if collection_by_uuid and collection_by_uuid.name == collection_name:
            print(f"✅ Successfully retrieved collection by UUID")
        else:
            print(f"❌ Failed to retrieve collection by UUID")
            return False
    except Exception as e:
        print(f"❌ Error retrieving by UUID: {e}")
        return False
    
    # Test 2: Insert vector using collection UUID
    print(f"\n2️⃣ Testing vector insertion with UUID...")
    try:
        vector_id = client.insert_vector(
            uuid,  # Using UUID instead of name
            vector_id=f"vec-uuid-test",
            vector=[0.1] * 384,  # Simple vector to avoid random issues
            metadata={"test": "uuid-based insert"}
        )
        if vector_id == "vec-uuid-test":
            print(f"✅ Successfully inserted vector using collection UUID")
        else:
            print(f"❌ Failed to insert vector using UUID")
            return False
    except Exception as e:
        print(f"❌ Error inserting vector with UUID: {e}")
        import traceback
        traceback.print_exc()
        return False
    
    # Test 3: Delete collection by UUID
    print(f"\n3️⃣ Testing delete_collection with UUID...")
    try:
        client.delete_collection(uuid)  # Using UUID instead of name
        print(f"✅ Successfully deleted collection by UUID")
    except Exception as e:
        print(f"❌ Error deleting collection by UUID: {e}")
        return False
    
    # Verify deletion
    try:
        client.get_collection(uuid)
        print(f"❌ Collection should have been deleted")
        return False
    except Exception:
        print(f"✅ Confirmed collection deletion")
    
    print(f"\n✅ All basic UUID operations passed!")
    return True

if __name__ == "__main__":
    success = test_uuid_operations_simple()
    exit(0 if success else 1)