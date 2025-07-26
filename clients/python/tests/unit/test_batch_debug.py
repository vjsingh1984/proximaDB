#!/usr/bin/env python3
"""
Debug batch processing - simple test to examine server logs
"""

# To run this script, set PYTHONPATH to include the src directory:
# PYTHONPATH=/home/vsingh/code/proximaDB/clients/python/src python tests/unit/test_batch_debug.py

import time
import uuid
import numpy as np
from pathlib import Path
import logging

from proximadb import ProximaDBClient, Protocol

# Enable debug logging
logging.basicConfig(level=logging.DEBUG)

def main():
    print("🔍 Simple Batch Debug Test")
    print("=" * 40)
    
    # Initialize client
    client = ProximaDBClient("localhost:5679", enable_debug_logging=True)
    collection_id = f"debug_batch_{uuid.uuid4().hex[:8]}"
    
    try:
        # Create collection
        print(f"Creating collection: {collection_id}")
        result = client.create_collection(
            name=collection_id,
            dimension=384,
            distance_metric=1,  # COSINE
            storage_engine=1    # VIPER
        )
        print("✅ Collection created")
        
        # Test batch of 3 vectors
        print("\n🔥 Testing batch of 3 vectors...")
        vectors = []
        for i in range(3):
            vectors.append({
                "id": f"test_vec_{i}",
                "vector": np.random.normal(0, 0.1, 384).tolist(),
                "metadata": {"index": i, "batch": "debug_test"}
            })
        
        print(f"Sending batch with {len(vectors)} vectors...")
        start_time = time.time()
        
        result = client.insert_vectors(collection_id, vectors)
        
        end_time = time.time()
        
        print(f"✅ Insert result:")
        print(f"   Count: {result.count}")
        print(f"   Failed: {result.failed_count}")
        print(f"   Duration: {end_time - start_time:.3f}s")
        
        # Debug: Let's inspect the actual gRPC response
        print(f"\n🔍 Debug: Let's check what the server actually responded...")
        
        # Make the call again with manual response inspection
        print("Making raw gRPC call to inspect response...")
        import json
        from proximadb import proximadb_pb2 as pb2
        
        # Create modern proto request
        proto_vectors = client._create_proto_vector_batch(vectors, collection_id)
        request = pb2.VectorBatchRequest(
            collection_id=collection_id,
            vectors=proto_vectors
        )
        
        # Make raw call using modern VectorBatch endpoint
        raw_response = client._call_with_timeout(client.stub.VectorBatch, request)
        print(f"   Raw response success: {raw_response.success}")
        print(f"   Raw response vector_ids count: {len(raw_response.vector_ids)}")
        print(f"   Raw response vector_ids: {raw_response.vector_ids}")
        if raw_response.metrics:
            print(f"   Raw response metrics total_processed: {raw_response.metrics.total_processed}")
            print(f"   Raw response metrics successful_count: {raw_response.metrics.successful_count}")
        
        # Wait a moment for server logs to flush
        time.sleep(1)
        
        # Test search to verify
        print("\n🔍 Testing search...")
        query_vector = np.random.normal(0, 0.1, 384).tolist()
        
        search_results = client.search_vectors(
            collection_id=collection_id,
            query_vectors=[query_vector],
            top_k=5,
            include_metadata=True
        )
        
        print(f"   Search found {len(search_results)} results")
        for i, result in enumerate(search_results):
            metadata = result.metadata if hasattr(result, 'metadata') and result.metadata else {}
            print(f"      {i+1}. ID: {result.id}, Score: {result.score:.4f}")
        
    except Exception as e:
        print(f"❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
    finally:
        # Cleanup
        try:
            client.delete_collection(collection_id)
            print("✅ Cleanup completed")
        except:
            pass

if __name__ == "__main__":
    main()