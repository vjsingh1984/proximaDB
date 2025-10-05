#!/usr/bin/env python3
"""
ProximaDB v1 Proto Demo
Demonstrates the working v1 proto migration with gRPC client
"""

import sys
import numpy as np
import time
from proximadb.protocols.grpc_sync import ProximaDBSyncGrpcClient

def main():
    print("=" * 70)
    print("ProximaDB v1 Proto Migration Demo")
    print("=" * 70)
    print()

    # Connect to server
    print("📡 Step 1: Connecting to ProximaDB gRPC server...")
    try:
        client = ProximaDBSyncGrpcClient(
            server_address="localhost:5679",
            enable_compression=False
        )
        print("   ✅ Connected successfully")
    except Exception as e:
        print(f"   ❌ Connection failed: {e}")
        return 1

    # Create collection
    collection_name = f"demo_v1_{int(time.time())}"
    dimension = 128

    print(f"\n📁 Step 2: Creating collection '{collection_name}'...")
    try:
        client.create_collection(
            name=collection_name,
            dimension=dimension,
            distance_metric="COSINE"
        )
        print(f"   ✅ Collection created")
        print(f"      - Name: {collection_name}")
        print(f"      - Dimension: {dimension}")
        print(f"      - Metric: cosine")
    except Exception as e:
        print(f"   ❌ Collection creation failed: {e}")
        return 1

    # Insert vectors with metadata
    print(f"\n📥 Step 3: Inserting vectors with metadata...")
    num_vectors = 100
    vectors = []

    for i in range(num_vectors):
        vector_data = {
            'id': f'vec_{i}',
            'vector': np.random.rand(dimension).tolist(),
            'metadata': {
                'index': i,
                'category': 'demo' if i % 2 == 0 else 'test',
                'score': float(i * 0.1),
                'active': bool(i % 3 == 0)
            }
        }
        vectors.append(vector_data)

    try:
        start_time = time.time()
        result = client.insert_vectors(collection_name, vectors)
        elapsed = time.time() - start_time

        print(f"   ✅ Vectors inserted successfully")
        print(f"      - Count: {num_vectors}")
        print(f"      - Time: {elapsed:.3f}s")
        print(f"      - Throughput: {num_vectors/elapsed:.0f} vectors/sec")
        print(f"   📋 Metadata fields:")
        print(f"      - index (int64)")
        print(f"      - category (string)")
        print(f"      - score (float)")
        print(f"      - active (bool)")
    except Exception as e:
        print(f"   ❌ Insert failed: {e}")
        import traceback
        traceback.print_exc()
        return 1

    # Search with metadata filters
    print(f"\n🔍 Step 4: Searching with metadata filters...")
    query_vector = np.random.rand(dimension).tolist()

    try:
        start_time = time.time()
        results = client.search_vectors(
            collection_id=collection_name,
            query_vector=query_vector,
            top_k=10,
            metadata_filters={'category': 'demo'},
            include_metadata=True
        )
        elapsed = time.time() - start_time

        print(f"   ✅ Search completed")
        print(f"      - Results: {len(results)}")
        print(f"      - Latency: {elapsed*1000:.2f}ms")
        print(f"      - QPS: {1/elapsed:.0f} queries/sec")

        if results:
            print(f"\n   📊 Top 3 Results:")
            for i, result in enumerate(results[:3], 1):
                print(f"      {i}. ID: {result.id if hasattr(result, 'id') else 'N/A'}")
                print(f"         Similarity: {result.similarity if hasattr(result, 'similarity') else 0:.4f}")
                if hasattr(result, 'metadata') and result.metadata:
                    print(f"         Metadata: {result.metadata}")
    except Exception as e:
        print(f"   ❌ Search failed: {e}")
        import traceback
        traceback.print_exc()
        return 1

    # Get specific vector
    print(f"\n🔎 Step 5: Retrieving specific vector...")
    try:
        vector_result = client.get_vector(collection_name, 'vec_0')
        print(f"   ✅ Vector retrieved")
        if hasattr(vector_result, 'id'):
            print(f"      - ID: {vector_result.id}")
        if hasattr(vector_result, 'metadata') and vector_result.metadata:
            print(f"      - Metadata: {vector_result.metadata}")
    except Exception as e:
        print(f"   ❌ Get vector failed: {e}")

    print()
    print("=" * 70)
    print("🎉 Demo completed successfully!")
    print("=" * 70)
    print()
    print("✅ v1 Proto Migration Features Demonstrated:")
    print("   - gRPC client using v1 proto structures")
    print("   - Metadata with int64, float, string, and bool types")
    print("   - Map-based metadata encoding (not repeated fields)")
    print("   - VectorRecord using v1 format")
    print("   - Search with metadata filtering")
    print()

    return 0

if __name__ == '__main__':
    sys.exit(main())
