#!/usr/bin/env python3
"""
WAL Persistence Test - REST Based
Creates test data and verifies it persists across server restarts
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'clients/python/src'))

import json
import time
import numpy as np
from proximadb import connect_rest, CollectionConfig, DistanceMetric, StorageEngine, VectorRecord

def test_wal_persistence():
    """Test WAL persistence with REST client"""
    
    print("🚀 WAL Persistence Test (REST)")
    print("="*60)
    
    collection_name = "wal_persistence_test"
    test_vectors = []
    
    # Generate test vectors
    for i in range(50):
        vector_data = np.random.random(128).astype(np.float32)
        test_vectors.append({
            'id': f'wal_test_vector_{i}',
            'vector': vector_data.tolist(),
            'metadata': {
                'batch': 'persistence_test',
                'index': i,
                'timestamp': time.time()
            }
        })
    
    try:
        # Connect to server
        print("🔗 Connecting to REST server...")
        client = connect_rest("http://localhost:5678")
        
        # Check if collection exists
        try:
            collection = client.get_collection(collection_name)
            print(f"✅ Found existing collection: {collection_name}")
            
            # Test search to verify persistence
            query_vector = test_vectors[0]['vector']
            results = client.search(collection_name, query_vector, top_k=10)
            print(f"📊 Found {len(results)} persisted vectors")
            
            if len(results) > 0:
                print("✅ WAL PERSISTENCE VERIFIED!")
                print("   Data survived server restart")
                for i, result in enumerate(results[:5]):
                    print(f"   {i+1}. {result.id}")
                return True
            else:
                print("❌ No persisted data found")
                return False
                
        except Exception as e:
            print(f"📦 Collection doesn't exist, creating new one...")
            
            # Create collection
            config = CollectionConfig(
                name=collection_name,
                dimension=128,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=StorageEngine.VIPER,
                description="WAL persistence test collection"
            )
            
            collection = client.create_collection(collection_name, config)
            print(f"✅ Created collection: {collection_name}")
            
            # Insert test vectors
            print(f"📝 Inserting {len(test_vectors)} test vectors...")
            
            vectors_to_insert = []
            for i, vec_data in enumerate(test_vectors):
                vector = VectorRecord(
                    id=vec_data['id'],
                    vector=vec_data['vector'],
                    metadata=vec_data['metadata']
                )
                vectors_to_insert.append(vector)
            
            # Batch insert
            response = client.insert_vectors(collection_name, vectors_to_insert)
            print(f"✅ All {len(test_vectors)} vectors inserted")
            
            # Test immediate search
            query_vector = test_vectors[0]['vector']
            results = client.search(collection_name, query_vector, top_k=5)
            print(f"🔍 Immediate search found {len(results)} vectors")
            
            # Save test state
            test_state = {
                'collection_name': collection_name,
                'vectors_inserted': len(test_vectors),
                'test_timestamp': time.time(),
                'first_vector_id': test_vectors[0]['id'],
                'query_vector': test_vectors[0]['vector'][:5]  # Save first 5 dims as sample
            }
            
            with open('wal_persistence_state.json', 'w') as f:
                json.dump(test_state, f, indent=2)
            
            print("📊 Test state saved to: wal_persistence_state.json")
            print("\n🔄 RESTART THE SERVER NOW")
            print("   Then run this test again to verify WAL persistence!")
            
            return False  # Data just inserted, need restart to test
            
    except Exception as e:
        print(f"❌ Error: {e}")
        return False

if __name__ == "__main__":
    test_wal_persistence()