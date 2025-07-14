#!/usr/bin/env python3
"""
Test REST client only
"""

import numpy as np
from proximadb import ProximaDBClient, Protocol, VectorRecord

def test_rest():
    """Test REST client operations"""
    
    print("Testing REST Client")
    print("=" * 50)
    
    # Use REST explicitly
    client = ProximaDBClient(url="http://localhost:5678", protocol=Protocol.REST)
    print(f"✅ Connected using: {client.active_protocol}")
    
    # Test health
    try:
        health = client.health()
        print(f"✅ Server health: {health.status}")
    except Exception as e:
        print(f"❌ Health check failed: {e}")
    
    # Create collection
    try:
        collection = client.create_collection(
            name="test_rest_unified",
            dimension=128,
            distance_metric="cosine",
            storage_engine="viper"
        )
        print(f"✅ Collection created: {collection.config.name}")
    except Exception as e:
        print(f"❌ Create collection failed: {e}")
        return
    
    # Insert using legacy method for simplicity
    try:
        vectors = [np.random.rand(128).tolist() for _ in range(5)]
        ids = [f"vec_{i}" for i in range(5)]
        metadata = [{"index": i} for i in range(5)]
        
        result = client.insert("test_rest_unified", vectors, ids, metadata)
        print(f"✅ Inserted vectors: {result.metrics.successful_count}")
    except Exception as e:
        print(f"❌ Insert failed: {e}")
    
    # Search
    try:
        query = np.random.rand(128).tolist()
        results = client.search("test_rest_unified", query, top_k=3)
        print(f"✅ Found {len(results)} results")
    except Exception as e:
        print(f"❌ Search failed: {e}")
    
    # Cleanup
    try:
        client.delete_collection("test_rest_unified")
        print("✅ Collection deleted")
    except Exception as e:
        print(f"❌ Delete failed: {e}")

if __name__ == "__main__":
    test_rest()