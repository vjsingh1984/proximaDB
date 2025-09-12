#!/usr/bin/env python3
"""List collections and test persistence"""
import os
import sys

sys.path.insert(0, 'src')
os.environ['PROXIMADB_URL'] = 'http://localhost:5678'
os.environ['PROXIMADB_GRPC_URL'] = 'http://localhost:5679'

from proximadb import ProximaDBClient

def test_persistence():
    """Check what collections persisted"""
    
    print("=" * 80)
    print("Collection Persistence Test")
    print("=" * 80)
    
    # Test with both REST and gRPC
    for protocol in ["rest", "grpc"]:
        client = ProximaDBClient(protocol=protocol)
        
        print(f"\n📋 Listing collections via {protocol.upper()}...")
        
        try:
            collections = client.list_collections()
            print(f"   Found {len(collections)} collections")
            
            if collections:
                # Show first 5 collections
                for i, collection in enumerate(collections[:5]):
                    print(f"\n   Collection {i+1}:")
                    print(f"     - ID: {collection.id}")
                    print(f"     - Name: {collection.config.name}")
                    print(f"     - Dimension: {collection.config.dimension}")
                    print(f"     - Distance Metric: {collection.config.distance_metric.name}")
                    print(f"     - Storage Engine: {collection.config.storage_engine.name}")
                    print(f"     - Created: {collection.created_at}")
                    
                    # Check stats
                    if collection.stats:
                        print(f"     - Vector Count: {collection.stats.vector_count}")
                
                if len(collections) > 5:
                    print(f"\n   ... and {len(collections) - 5} more collections")
                    
        except Exception as e:
            print(f"   ❌ Error: {e}")

if __name__ == "__main__":
    test_persistence()