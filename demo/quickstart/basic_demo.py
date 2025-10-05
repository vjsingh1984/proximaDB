#!/usr/bin/env python3
"""
ProximaDB Basic Demo - Updated for new gRPC/REST API

This script demonstrates basic functionality of ProximaDB including:
- Collection creation
- Vector insertion
- Vector search
- Collection management
"""

import sys
import time
import random
import logging
from pathlib import Path

# Add SDK to Python path
sdk_path = str(Path(__file__).parent.parent.parent / "clients" / "python" / "src")
if sdk_path not in sys.path:
    sys.path.insert(0, sdk_path)

try:
    from proximadb.protocols.grpc_sync import ProximaDBSyncGrpcClient
except ImportError as e:
    print(f"❌ Failed to import ProximaDB client: {e}")
    print("Please ensure the Python SDK is installed:")
    print("cd clients/python && pip install -e .")
    print(f"SDK path: {sdk_path}")
    sys.exit(1)

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(message)s')
logger = logging.getLogger(__name__)


def generate_random_vector(dimension: int) -> list:
    """Generate a random normalized vector"""
    vec = [random.gauss(0, 1) for _ in range(dimension)]
    magnitude = sum(x**2 for x in vec) ** 0.5
    return [x / magnitude for x in vec]


def main():
    print("\n" + "="*70)
    print("ProximaDB Basic Demo - Feature Showcase")
    print("="*70)

    # Configuration
    collection_name = f"basic_demo_{int(time.time())}"
    dimension = 128
    num_vectors = 100
    search_top_k = 10

    # Step 1: Connect to ProximaDB
    print("\n📡 Step 1: Connecting to ProximaDB...")
    try:
        client = ProximaDBSyncGrpcClient(
            "localhost:5679",
            enable_compression=False
        )
        logger.info("   ✅ Connected to gRPC server (localhost:5679)")
    except Exception as e:
        logger.error(f"   ❌ Failed to connect: {e}")
        print("\n💡 Please ensure ProximaDB server is running:")
        print("   cargo run --bin proximadb-server")
        return 1

    # Step 2: Create Collection
    print(f"\n📁 Step 2: Creating collection '{collection_name}'...")
    try:
        result = client.create_collection(
            name=collection_name,
            dimension=dimension,
            distance_metric=1,  # 1 = cosine
            storage_engine=0    # 0 = auto-select
        )
        logger.info(f"   ✅ Collection created successfully")
        logger.info(f"      - Name: {collection_name}")
        logger.info(f"      - Dimension: {dimension}")
        logger.info(f"      - Distance: cosine")
    except Exception as e:
        logger.error(f"   ❌ Failed to create collection: {e}")
        client.close()
        return 1

    # Step 3: Generate and Insert Vectors
    print(f"\n📥 Step 3: Inserting {num_vectors} vectors...")
    try:
        start_time = time.time()

        # Insert in batches
        batch_size = 50
        for batch_start in range(0, num_vectors, batch_size):
            batch_end = min(batch_start + batch_size, num_vectors)
            vectors = [
                {
                    "id": f"vec_{i}",
                    "vector": generate_random_vector(dimension),
                    "metadata": {"index": i, "batch": batch_start // batch_size}
                }
                for i in range(batch_start, batch_end)
            ]

            client.insert_vectors(collection_name, vectors)
            logger.info(f"   ✅ Inserted batch {batch_start // batch_size + 1}: vectors {batch_start}-{batch_end-1}")

        duration = time.time() - start_time
        vectors_per_sec = num_vectors / duration

        logger.info(f"\n   🎉 Total: {num_vectors} vectors in {duration:.2f}s")
        logger.info(f"   ⚡ Throughput: {vectors_per_sec:.0f} vectors/second")

    except Exception as e:
        logger.error(f"   ❌ Failed to insert vectors: {e}")
        client.close()
        return 1

    # Step 4: Vector Search
    print(f"\n🔍 Step 4: Searching for top {search_top_k} similar vectors...")
    try:
        query_vector = generate_random_vector(dimension)

        start_time = time.time()
        results = client.search_vectors(
            collection_id=collection_name,
            query_vector=query_vector,
            top_k=search_top_k
        )
        search_time = time.time() - start_time

        logger.info(f"   ✅ Search completed in {search_time*1000:.2f}ms")
        logger.info(f"   📊 Found {len(results)} results\n")

        # Display top 5 results
        print("   🎯 Top 5 Results:")
        for i, result in enumerate(results[:5], 1):
            score = result.score if hasattr(result, 'score') else 0.0
            vec_id = result.id if hasattr(result, 'id') else 'unknown'
            print(f"      {i}. ID: {vec_id}, Similarity Score: {score:.6f}")

    except Exception as e:
        logger.error(f"   ❌ Search failed: {e}")
        client.close()
        return 1

    # Step 5: Collection Info
    print(f"\n📋 Step 5: Getting collection information...")
    try:
        info = client.get_collection(collection_name)
        logger.info(f"   ✅ Collection Info:")
        logger.info(f"      - Name: {info.get('name', collection_name)}")
        logger.info(f"      - Dimension: {info.get('dimension', dimension)}")
        logger.info(f"      - Vector Count: {info.get('vector_count', 0)}")
        logger.info(f"      - Distance Metric: {info.get('distance_metric', 'cosine')}")
    except Exception as e:
        logger.warning(f"   ⚠️  Failed to get collection info: {e}")

    # Step 6: Performance Benchmark
    print(f"\n⚡ Step 6: Running quick performance benchmark...")
    try:
        iterations = 10
        latencies = []

        for _ in range(iterations):
            query_vector = generate_random_vector(dimension)
            start_time = time.time()
            results = client.search_vectors(
                collection_id=collection_name,
                query_vector=query_vector,
                top_k=search_top_k
            )
            latency = (time.time() - start_time) * 1000
            latencies.append(latency)

        avg_latency = sum(latencies) / len(latencies)
        p95_latency = sorted(latencies)[int(len(latencies) * 0.95)]
        throughput = 1000 / avg_latency  # queries per second

        logger.info(f"   📊 Benchmark Results ({iterations} iterations):")
        logger.info(f"      - Average Latency: {avg_latency:.2f}ms")
        logger.info(f"      - P95 Latency: {p95_latency:.2f}ms")
        logger.info(f"      - Throughput: {throughput:.0f} queries/second")

    except Exception as e:
        logger.warning(f"   ⚠️  Benchmark failed: {e}")

    # Summary
    print("\n" + "="*70)
    print("🎉 Demo Complete!")
    print("="*70)
    print(f"\n✅ Successfully demonstrated:")
    print(f"   1. ✓ Connection to ProximaDB (gRPC)")
    print(f"   2. ✓ Collection creation")
    print(f"   3. ✓ Batch vector insertion ({num_vectors} vectors)")
    print(f"   4. ✓ Vector similarity search")
    print(f"   5. ✓ Collection management")
    print(f"   6. ✓ Performance benchmarking")

    print(f"\n📱 Next Steps:")
    print(f"   • View dashboard: http://localhost:5678/dashboard")
    print(f"   • Check metrics: http://localhost:5678/metrics/json")
    print(f"   • Explore collection: {collection_name}")

    print(f"\n🧹 Cleanup:")
    print(f"   Collection '{collection_name}' left for inspection")
    print(f"   To delete: client.delete_collection('{collection_name}')")
    print("="*70 + "\n")

    client.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
