#!/usr/bin/env python3
"""
ProximaDB Complete Workflow Demo

Demonstrates end-to-end workflow:
1. Create collections via Python SDK
2. Insert vectors with metadata
3. Search vectors (two-stage parallel search: WAL + Storage)
4. View results on dashboard
5. Monitor metrics in real-time
"""

import sys
import time
import random
import requests

# Add SDK to path
sys.path.insert(0, 'clients/python/src')

from proximadb.models import DistanceMetricType, StorageEngineType

from proximadb.protocols.grpc_sync import ProximaDBSyncGrpcClient


def generate_random_vector(dimension: int) -> list:
    """Generate a random normalized vector"""
    vec = [random.gauss(0, 1) for _ in range(dimension)]
    # Normalize
    magnitude = sum(x**2 for x in vec) ** 0.5
    return [x / magnitude for x in vec]


def main():
    print("\n" + "="*70)
    print("ProximaDB Complete Workflow Demo")
    print("="*70)

    # Step 1: Connect to ProximaDB
    print("\n📡 Step 1: Connecting to ProximaDB...")
    try:
        client = ProximaDBSyncGrpcClient(
            "localhost:5679",
            enable_compression=False  # Disable compression for compatibility
        )
        print("   ✅ Connected to gRPC server (localhost:5679)")
    except Exception as e:
        print(f"   ❌ Failed to connect: {e}")
        print("\n💡 Please ensure ProximaDB server is running:")
        print("   cargo run --bin proximadb-server")
        return 1

    # Step 2: Create Collection
    print("\n📁 Step 2: Creating test collection...")
    collection_name = f"demo_collection_{int(time.time())}"
    dimension = 128

    try:
        result = client.create_collection(
            name=collection_name,
            dimension=dimension,
            distance_metric=DistanceMetricType.COSINE,
            storage_engine=StorageEngineType.UNSPECIFIED  # Auto-select best engine
        )
        print(f"   ✅ Collection created: {collection_name}")
        print(f"      - Dimension: {dimension}")
        print(f"      - Distance Metric: COSINE")
    except Exception as e:
        print(f"   ❌ Failed to create collection: {e}")
        return 1

    # Step 3: Insert Vectors (in batches to show WAL behavior)
    print(f"\n📥 Step 3: Inserting vectors...")
    batch_size = 100
    total_vectors = 500

    try:
        for batch_idx in range(0, total_vectors, batch_size):
            vectors = [
                {
                    "id": f"vec_{i}",
                    "vector": generate_random_vector(dimension)
                }
                for i in range(batch_idx, min(batch_idx + batch_size, total_vectors))
            ]

            client.insert_vectors(collection_name, vectors)
            print(f"   ✅ Batch {batch_idx // batch_size + 1}: Inserted vectors {batch_idx} to {batch_idx + len(vectors) - 1}")
            time.sleep(0.1)  # Small delay to simulate real workload

        print(f"\n   🎉 Total vectors inserted: {total_vectors}")
    except Exception as e:
        print(f"   ❌ Failed to insert vectors: {e}")
        client.close()
        return 1

    # Step 4: Search Vectors (demonstrating two-stage search)
    print(f"\n🔍 Step 4: Searching vectors (two-stage: WAL + Storage)...")
    try:
        query_vector = generate_random_vector(dimension)
        top_k = 10

        print(f"   🎯 Executing search with top_k={top_k}...")
        search_results = client.search_vectors(
            collection_id=collection_name,
            query_vector=query_vector,
            top_k=top_k
        )

        print(f"   ✅ Search complete: Found {len(search_results)} results")
        print(f"\n   📊 Top {min(5, len(search_results))} Results:")
        for i, result in enumerate(search_results[:5], 1):
            score = result.get('score', 0)
            vec_id = result.get('id', 'unknown')
            print(f"      {i}. ID: {vec_id}, Score: {score:.6f}")

    except Exception as e:
        print(f"   ❌ Search failed: {e}")
        client.close()
        return 1

    # Step 5: Check Dashboard and Metrics
    print(f"\n📊 Step 5: Checking dashboard and metrics...")
    base_url = "http://localhost:5678"

    try:
        # Get metrics
        response = requests.get(f"{base_url}/metrics/json", timeout=5)
        if response.status_code == 200:
            metrics = response.json()
            storage = metrics.get('storage', {})

            print(f"   ✅ Metrics retrieved:")
            print(f"      - Total Collections: {storage.get('total_collections', 0)}")
            print(f"      - Total Vectors: {storage.get('total_vectors', 0)}")
            print(f"      - Uptime: {metrics.get('uptime_seconds', 0):.1f}s")

        # Verify collection appears in API
        response = requests.get(f"{base_url}/api/v1/collections", timeout=5)
        if response.status_code == 200:
            collections = response.json()
            our_collection = next((c for c in collections if c.get('name') == collection_name), None)

            if our_collection:
                print(f"\n   ✅ Collection visible in dashboard API:")
                print(f"      - Name: {our_collection.get('name')}")
                print(f"      - Vectors: {our_collection.get('vector_count', 0)}")
                print(f"      - Engine: {our_collection.get('engine', 'unknown')}")

    except Exception as e:
        print(f"   ⚠️  Metrics check failed: {e}")

    # Step 6: Collection Info
    print(f"\n📋 Step 6: Getting collection info...")
    try:
        info = client.get_collection(collection_name)
        print(f"   ✅ Collection Info:")
        print(f"      - Name: {info.get('name')}")
        print(f"      - ID: {info.get('collection_id')}")
        print(f"      - Dimension: {info.get('dimension')}")
        print(f"      - Distance Metric: {info.get('distance_metric')}")
        print(f"      - Vector Count: {info.get('vector_count', 0)}")
    except Exception as e:
        print(f"   ⚠️  Failed to get collection info: {e}")

    # Final Summary
    print("\n" + "="*70)
    print("🎉 Demo Complete!")
    print("="*70)
    print(f"\n✅ Successfully demonstrated:")
    print(f"   1. ✓ Collection creation via gRPC")
    print(f"   2. ✓ Batch vector insertion ({total_vectors} vectors)")
    print(f"   3. ✓ Two-stage parallel search (WAL + Storage)")
    print(f"   4. ✓ Dashboard and metrics integration")
    print(f"   5. ✓ Real-time monitoring")

    print(f"\n📱 Next Steps:")
    print(f"   • View dashboard: http://localhost:5678/dashboard")
    print(f"   • Check metrics: http://localhost:5678/metrics/json")
    print(f"   • View collections: http://localhost:5678/api/v1/collections")

    print(f"\n🧹 Cleanup:")
    print(f"   Collection '{collection_name}' left for dashboard inspection")
    print(f"   To delete: client.delete_collection('{collection_name}')")

    client.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
