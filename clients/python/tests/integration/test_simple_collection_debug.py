#!/usr/bin/env python3
"""
Simple debug test for collection creation
"""

import asyncio
import sys
import uuid

# Add client path
sys.path.insert(0, 'clients/python/src')

from proximadb.grpc_client import ProximaDBClient

async def debug_collection():
    """Debug collection creation"""
    
    print("🔍 Debug Collection Creation")
    print("=" * 40)
    
    # Connect to server
    print("1. Connecting to ProximaDB...")
    client = ProximaDBClient("localhost:5679")
    
    # List existing collections
    print("2. Listing existing collections...")
    existing = await client.list_collections()
    print(f"   Found {len(existing)} existing collections:")
    for col in existing:
        name = col.config.name if col.config else col.name
        print(f"     - {name} (UUID: {col.id})")
    
    # Create new collection
    collection_name = f"debug_test_{uuid.uuid4().hex[:8]}"
    print(f"3. Creating collection: {collection_name}")
    
    try:
        # Try to create collection and capture the exact result
        result = await client.create_collection(
            name=collection_name,
            dimension=128,
            distance_metric=1,  # COSINE
            indexing_algorithm=1  # HNSW
        )
        
        print(f"   Type of result: {type(result)}")
        print(f"   Result: {result}")
        
        if result:
            print(f"   Result has config: {hasattr(result, 'config')}")
            if hasattr(result, 'config'):
                print(f"   Config: {result.config}")
                print(f"   Config has name: {hasattr(result.config, 'name')}")
                if hasattr(result.config, 'name'):
                    print(f"   Collection name: {result.config.name}")
            print(f"   Result has id: {hasattr(result, 'id')}")
            if hasattr(result, 'id'):
                print(f"   Collection ID: {result.id}")
        else:
            print("   ❌ Result is None!")
            
    except Exception as e:
        print(f"   ❌ Creation failed: {e}")
        import traceback
        traceback.print_exc()

if __name__ == "__main__":
    asyncio.run(debug_collection())