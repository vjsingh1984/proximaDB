#!/usr/bin/env python3
"""
Debug script to test actual gRPC batch processing
"""

import sys
import os
import time
import json
from pathlib import Path

# Add SDK to path
workspace_root = Path(__file__).parent  # /workspace/
sdk_path = workspace_root / "clients/python/src"
sys.path.insert(0, str(sdk_path))

try:
    from proximadb.grpc_client import ProximaDBClient
    print("✅ ProximaDB SDK imported successfully")
except ImportError as e:
    print(f"❌ Failed to import ProximaDB SDK: {e}")
    sys.exit(1)

def test_grpc_batch_insert():
    """Test gRPC batch insertion with debugging"""
    print("🧪 Testing gRPC Batch Insertion")
    print("=" * 50)
    
    # Initialize client
    client = ProximaDBClient(
        endpoint="localhost:5679",
        enable_debug_logging=True
    )
    
    collection_id = "debug_batch_test"
    
    try:
        # Create collection
        print(f"🏗️ Creating collection: {collection_id}")
        try:
            client.create_collection(
                name=collection_id,
                dimension=4,
                distance_metric=1,  # COSINE
                storage_engine=1    # VIPER
            )
            print("✅ Collection created")
        except Exception as e:
            if "already exists" in str(e).lower():
                print("⚠️ Collection already exists, continuing...")
            else:
                raise
        
        # Test different batch sizes
        batch_sizes = [1, 2, 5, 10]
        
        for batch_size in batch_sizes:
            print(f"\n🔧 Testing batch size: {batch_size}")
            
            # Create test vectors
            vectors = []
            for i in range(batch_size):
                vectors.append({
                    "id": f"batch_{batch_size}_vec_{i}",
                    "vector": [float(j + i * 0.1) for j in range(4)],
                    "metadata": {"batch_size": str(batch_size), "index": str(i)}
                })
            
            print(f"   Created {len(vectors)} test vectors")
            
            # Insert vectors
            start_time = time.time()
            result = client.insert_vectors(collection_id, vectors)
            end_time = time.time()
            
            print(f"   📊 Results:")
            print(f"      Requested: {batch_size} vectors")
            print(f"      Inserted: {result.count} vectors")
            print(f"      Failed: {result.failed_count} vectors")
            print(f"      Duration: {(end_time - start_time)*1000:.1f}ms")
            
            # Check if all vectors were inserted
            if result.count == batch_size:
                print(f"   ✅ Batch size {batch_size}: All vectors inserted correctly")
            else:
                print(f"   ❌ Batch size {batch_size}: Only {result.count}/{batch_size} vectors inserted")
        
        # Test search to verify vectors are accessible
        print(f"\n🔍 Testing search to verify vectors...")
        query_vector = [0.1, 0.2, 0.3, 0.4]
        results = client.search_vectors(
            collection_id=collection_id,
            query_vectors=[query_vector],
            top_k=20,
            include_metadata=True
        )
        
        print(f"   Found {len(results)} vectors in search")
        
        # Group by batch size to verify
        batch_counts = {}
        for result in results:
            if hasattr(result, 'metadata') and result.metadata:
                batch_size = result.metadata.get('batch_size', 'unknown')
                batch_counts[batch_size] = batch_counts.get(batch_size, 0) + 1
        
        print(f"   Vectors by batch size: {batch_counts}")
        
    except Exception as e:
        print(f"❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
    finally:
        # Cleanup
        try:
            client.delete_collection(collection_id)
            print(f"🧹 Cleaned up collection: {collection_id}")
        except:
            pass

if __name__ == "__main__":
    test_grpc_batch_insert()