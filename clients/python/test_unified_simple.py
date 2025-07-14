#!/usr/bin/env python3
"""
Simple test of unified client
"""

import numpy as np
from proximadb import ProximaDBClient, Protocol, VectorRecord

def test_unified():
    """Test unified client basic operations"""
    
    print("Testing Unified Client")
    print("=" * 50)
    
    # Test 1: REST client
    print("\n1. Testing REST client...")
    try:
        rest_client = ProximaDBClient(url="http://localhost:5678", protocol=Protocol.REST)
        health = rest_client.health()
        print(f"✅ REST client connected: {health.status}")
    except Exception as e:
        print(f"❌ REST client error: {e}")
    
    # Test 2: Auto client (should select REST since gRPC might not be available)
    print("\n2. Testing AUTO client...")
    try:
        auto_client = ProximaDBClient(url="localhost", protocol=Protocol.AUTO)
        print(f"✅ AUTO client connected using: {auto_client.active_protocol}")
        
        # Create a collection
        collection = auto_client.create_collection(
            name="test_unified",
            dimension=128,
            distance_metric="cosine"
        )
        print(f"✅ Collection created: {collection.config.name}")
        
        # Insert vectors
        records = [
            VectorRecord(
                id=f"vec_{i}",
                vector=np.random.rand(128).tolist(),
                metadata={"index": i}
            )
            for i in range(5)
        ]
        
        result = auto_client.insert_vectors("test_unified", records)
        print(f"✅ Inserted {result.metrics.successful_count} vectors")
        
        # Search
        query = np.random.rand(128).tolist()
        results = auto_client.search_single("test_unified", query, top_k=3)
        print(f"✅ Found {len(results)} results")
        
        # Cleanup
        auto_client.delete_collection("test_unified")
        print("✅ Collection deleted")
        
    except Exception as e:
        print(f"❌ AUTO client error: {e}")
        import traceback
        traceback.print_exc()

if __name__ == "__main__":
    test_unified()