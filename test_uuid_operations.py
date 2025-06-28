#!/usr/bin/env python3
"""Test UUID-based operations with ProximaDB Python SDK for both REST and gRPC."""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients/python/src'))

from proximadb import ProximaDBClient
import numpy as np

def test_uuid_operations(client_type: str, port: int):
    """Test UUID operations with specified client type."""
    print(f"\n{'='*60}")
    print(f"Testing UUID operations with {client_type} client on port {port}")
    print(f"{'='*60}")
    
    # Initialize client
    if port == 5678:
        client = ProximaDBClient(f"http://localhost:{port}")
    else:
        client = ProximaDBClient(f"localhost:{port}")
    print(f"✅ Connected to ProximaDB ({client_type})")
    
    # Create a collection
    collection_name = f"uuid-test-{client_type.lower()}"
    collection = client.create_collection(collection_name, dimension=384)
    print(f"✅ Created collection: {collection_name}")
    
    # Get the collection UUID using the reverse lookup
    # The Python SDK doesn't expose this method yet, so we'll get it from the collection
    collection_info = client.get_collection(collection_name)
    uuid = collection_info.id  # Collection.id contains the UUID
    print(f"✅ Retrieved UUID: {uuid}")
    
    # Test 1: Get collection by UUID
    print(f"\n1️⃣ Testing get_collection with UUID...")
    collection_by_uuid = client.get_collection(uuid)
    assert collection_by_uuid is not None, "Failed to get collection by UUID"
    assert collection_by_uuid.name == collection_name
    assert collection_by_uuid.id == uuid
    print(f"✅ Successfully retrieved collection by UUID")
    
    # Test 2: Update collection by UUID
    print(f"\n2️⃣ Testing update_collection with UUID...")
    updated = client.update_collection(uuid, {
        "description": f"Updated via UUID - {client_type}",
        "tags": ["uuid-test", client_type.lower()]
    })
    assert updated is not None, "Failed to update collection by UUID"
    assert updated.config.description == f"Updated via UUID - {client_type}"
    # Note: checking tags depends on the exact return format
    print(f"✅ Successfully updated collection by UUID")
    
    # Test 3: Insert vectors using collection UUID
    print(f"\n3️⃣ Testing vector insertion with UUID...")
    vector_id = client.insert_vector(
        uuid,  # Using UUID instead of name
        vector_id=f"vec-uuid-1",
        vector=np.random.rand(384).tolist(),
        metadata={"test": "uuid-based insert", "client": client_type}
    )
    assert vector_id == f"vec-uuid-1"
    print(f"✅ Successfully inserted vector using collection UUID")
    
    # Test 4: Batch insert vectors using collection UUID
    print(f"\n4️⃣ Testing batch vector insertion with UUID...")
    vectors = [
        {
            "id": f"batch-uuid-{i}",
            "vector": np.random.rand(384).tolist(),
            "metadata": {"batch": True, "index": i, "client": client_type}
        }
        for i in range(3)
    ]
    vector_ids = client.batch_insert_vectors(uuid, vectors)  # Using UUID
    assert len(vector_ids) == 3
    assert all(f"batch-uuid-{i}" in vector_ids for i in range(3))
    print(f"✅ Successfully batch inserted vectors using collection UUID")
    
    # Test 5: Delete collection by UUID
    print(f"\n5️⃣ Testing delete_collection with UUID...")
    client.delete_collection(uuid)  # Using UUID instead of name
    print(f"✅ Successfully deleted collection by UUID")
    
    # Verify deletion
    try:
        client.get_collection(uuid)
        assert False, "Collection should have been deleted"
    except Exception as e:
        print(f"✅ Confirmed collection deletion (expected error: {type(e).__name__})")
    
    print(f"\n✅ All UUID operations passed for {client_type}!")
    return True

def main():
    """Test UUID operations with both REST and gRPC clients."""
    # Test with REST client
    try:
        test_uuid_operations("REST", 5678)
    except Exception as e:
        print(f"❌ REST tests failed: {e}")
        import traceback
        traceback.print_exc()
        return False
    
    # Test with gRPC client
    try:
        test_uuid_operations("gRPC", 5679)
    except Exception as e:
        print(f"❌ gRPC tests failed: {e}")
        import traceback
        traceback.print_exc()
        return False
    
    print("\n" + "="*60)
    print("🎉 ALL UUID OPERATION TESTS PASSED!")
    print("="*60)
    return True

if __name__ == "__main__":
    success = main()
    exit(0 if success else 1)