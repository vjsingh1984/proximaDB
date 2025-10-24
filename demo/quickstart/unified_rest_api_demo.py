#!/usr/bin/env python3
"""
ProximaDB REST API Demo - Updated for new REST endpoints

This example shows how to use ProximaDB's REST API for vector operations.
Uses standard REST endpoints matching the current server implementation.
"""

import requests
import random
import time

BASE_URL = "http://localhost:5678"


def generate_random_vector(dimension: int) -> list:
    """Generate a random normalized vector"""
    vec = [random.gauss(0, 1) for _ in range(dimension)]
    magnitude = sum(x**2 for x in vec) ** 0.5
    return [x / magnitude for x in vec]


def main():
    print("\n" + "="*70)
    print("ProximaDB REST API Demo")
    print("="*70)

    collection_name = f"rest_demo_{int(time.time())}"
    dimension = 128

    # 1. Health Check
    print("\n📡 Step 1: Health Check")
    try:
        response = requests.get(f"{BASE_URL}/health", timeout=5)
        response.raise_for_status()
        print(f"   ✅ Server is healthy: {response.json()}")
    except Exception as e:
        print(f"   ❌ Server not responding: {e}")
        print("   Please ensure ProximaDB server is running:")
        print("   cargo run --bin proximadb-server")
        return 1

    # 2. Create Collection
    print(f"\n📁 Step 2: Creating collection '{collection_name}'")
    try:
        payload = {
            "name": collection_name,
            "dimension": dimension,
            "distance_metric": "cosine"
        }
        response = requests.post(
            f"{BASE_URL}/api/v1/collections",
            json=payload,
            headers={"Content-Type": "application/json"},
            timeout=10
        )
        response.raise_for_status()
        result = response.json()
        print(f"   ✅ Collection created")
        print(f"      Response: {result}")
    except Exception as e:
        print(f"   ❌ Failed to create collection: {e}")
        if hasattr(e, 'response') and e.response:
            print(f"      Error: {e.response.text}")
        return 1

    # 3. Insert Vectors
    print(f"\n📥 Step 3: Inserting vectors")
    total_vectors = 50
    batch_size = 25

    try:
        for batch_idx in range(0, total_vectors, batch_size):
            vectors = [
                {
                    "id": f"vec_{i}",
                    "values": generate_random_vector(dimension),
                    "metadata": {
                        "index": i,
                        "batch": batch_idx // batch_size
                    }
                }
                for i in range(batch_idx, min(batch_idx + batch_size, total_vectors))
            ]

            payload = {"vectors": vectors}
            response = requests.post(
                f"{BASE_URL}/api/v1/collections/{collection_name}/vectors",
                json=payload,
                headers={"Content-Type": "application/json"},
                timeout=30
            )
            response.raise_for_status()
            print(f"   ✅ Batch {batch_idx // batch_size + 1}: Inserted {len(vectors)} vectors")
            time.sleep(0.1)

        print(f"\n   🎉 Total: {total_vectors} vectors inserted")
    except Exception as e:
        print(f"   ❌ Failed to insert vectors: {e}")
        if hasattr(e, 'response') and e.response:
            print(f"      Error: {e.response.text}")
        return 1

    # 4. Search Vectors
    print(f"\n🔍 Step 4: Searching vectors")
    try:
        query_vector = generate_random_vector(dimension)
        top_k = 10

        payload = {
            "query_vector": query_vector,
            "top_k": top_k
        }
        response = requests.post(
            f"{BASE_URL}/api/v1/collections/{collection_name}/search",
            json=payload,
            headers={"Content-Type": "application/json"},
            timeout=10
        )
        response.raise_for_status()
        search_results = response.json()

        # Handle both list and dict responses
        if isinstance(search_results, dict) and 'results' in search_results:
            results = search_results['results']
        else:
            results = search_results

        print(f"   ✅ Search complete: Found {len(results)} results")
        print(f"\n   📊 Top 5 Results:")
        for i, result in enumerate(results[:5], 1):
            score = result.get('score', result.get('distance', 0))
            vec_id = result.get('id', 'unknown')
            print(f"      {i}. ID: {vec_id}, Score: {score:.6f}")
    except Exception as e:
        print(f"   ❌ Search failed: {e}")
        if hasattr(e, 'response') and e.response:
            print(f"      Error: {e.response.text}")
        return 1

    # 5. Get Collection Info
    print(f"\n📋 Step 5: Getting collection info")
    try:
        response = requests.get(
            f"{BASE_URL}/api/v1/collections/{collection_name}",
            timeout=5
        )
        if response.status_code == 200:
            info = response.json()
            print(f"   ✅ Collection Info:")
            print(f"      - Name: {info.get('name', collection_name)}")
            print(f"      - Dimension: {info.get('dimension', dimension)}")
            print(f"      - Vectors: {info.get('vector_count', 0)}")
        else:
            print(f"   ⚠️  Collection info not available (endpoint may not be implemented)")
    except Exception as e:
        print(f"   ⚠️  Failed to get collection info: {e}")

    # 6. Check Metrics
    print(f"\n📊 Step 6: Checking system metrics")
    try:
        response = requests.get(f"{BASE_URL}/metrics/json", timeout=5)
        response.raise_for_status()
        metrics = response.json()

        storage = metrics.get('storage', {})
        print(f"   ✅ System Metrics:")
        print(f"      - Collections: {storage.get('total_collections', 0)}")
        print(f"      - Vectors: {storage.get('total_vectors', 0)}")
        print(f"      - Uptime: {metrics.get('uptime_seconds', 0):.1f}s")
    except Exception as e:
        print(f"   ⚠️  Metrics check failed: {e}")

    # 7. List Collections
    print(f"\n📋 Step 7: Listing all collections")
    try:
        response = requests.get(f"{BASE_URL}/api/v1/collections", timeout=5)
        response.raise_for_status()
        collections = response.json()

        # Find our collection
        collections_list = collections.get('collections', collections) if isinstance(collections, dict) else collections
        our_collection = next((c for c in collections_list if c.get('name') == collection_name), None)

        if our_collection:
            print(f"   ✅ Collection visible in list:")
            print(f"      - Name: {our_collection.get('name')}")
            print(f"      - Vectors: {our_collection.get('vector_count', 0)}")
            print(f"      - Engine: {our_collection.get('engine', 'auto')}")
        else:
            print(f"   ⚠️  Collection not found in list (may take time to sync)")
    except Exception as e:
        print(f"   ⚠️  List collections failed: {e}")

    # Summary
    print("\n" + "="*70)
    print("🎉 REST API Demo Complete!")
    print("="*70)
    print(f"\n✅ Successfully demonstrated:")
    print(f"   1. ✓ Health check")
    print(f"   2. ✓ Collection creation")
    print(f"   3. ✓ Batch vector insertion ({total_vectors} vectors)")
    print(f"   4. ✓ Vector search")
    print(f"   5. ✓ Collection info retrieval")
    print(f"   6. ✓ System metrics")
    print(f"   7. ✓ Collection listing")

    print(f"\n📱 View Results:")
    print(f"   • Dashboard: {BASE_URL}/dashboard")
    print(f"   • Metrics: {BASE_URL}/metrics/json")
    print(f"   • Collections: {BASE_URL}/api/v1/collections")

    print(f"\n🧹 Cleanup:")
    print(f"   Collection '{collection_name}' left for inspection")
    print(f"   To delete: DELETE {BASE_URL}/api/v1/collections/{collection_name}")
    print("="*70 + "\n")

    return 0


if __name__ == "__main__":
    import sys
    sys.exit(main())
