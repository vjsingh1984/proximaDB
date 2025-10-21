#!/usr/bin/env python3
"""
ProximaDB SDK-Driven Compression Example

This example demonstrates how to use SDK-driven compression configuration
to control compression settings for collections from the client side.

Copyright 2025 ProximaDB
"""

# import asyncio
import time
from typing import List
import numpy as np

from proximadb import ProximaDBClient
from proximadb.models import (
    CollectionConfig,
    CompressionConfig,
    CompressionAlgorithm,
    CompressionLevel,
    DistanceMetric,
    StorageEngine,
    VectorRecord,
    SearchOptimization,
)


def generate_random_vectors(num_vectors: int, dimension: int) -> List[List[float]]:
    """Generate random vectors for testing"""
    return np.random.rand(num_vectors, dimension).tolist()


def create_compressed_collections(client: ProximaDBClient):
    """Create collections with different compression configurations"""
    
    # 1. SST collection with ZSTD compression
    sst_config = CollectionConfig(
        name="compressed_sst_collection",
        dimension=1536,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.SST,
        compression_config=CompressionConfig(
            sst_compression_algorithm=CompressionAlgorithm.ZSTD,
            sst_compression_level=6,  # Balanced compression
            sst_block_size=32768,  # 32KB blocks
            adaptive_compression=True,
            compression_threshold_kb=50,
        ),
    )
    
    print("Creating SST collection with ZSTD compression...")
    sst_collection = client.create_collection(sst_config.name, sst_config)
    print(f"✅ Created: {sst_collection.id}")
    
    # 2. VIPER collection with LZ4 compression and dual columns
    viper_config = CollectionConfig(
        name="compressed_viper_collection",
        dimension=1536,
        distance_metric=DistanceMetric.EUCLIDEAN,
        storage_engine=StorageEngine.VIPER,
        compression_config=CompressionConfig(
            viper_compression_algorithm=CompressionAlgorithm.LZ4,
            viper_compression_level=1,  # Fast compression
            viper_enable_dual_columns=True,  # FP32 + quantized columns
            adaptive_compression=False,
        ),
    )
    
    print("Creating VIPER collection with LZ4 compression and dual columns...")
    viper_collection = client.create_collection(viper_config.name, viper_config)
    print(f"✅ Created: {viper_collection.id}")
    
    # 3. Mixed collection with adaptive compression
    mixed_config = CollectionConfig(
        name="adaptive_compression_collection",
        dimension=768,
        distance_metric=DistanceMetric.DOT_PRODUCT,
        storage_engine=StorageEngine.SST,
        compression_config=CompressionConfig(
            sst_compression_algorithm=CompressionAlgorithm.SNAPPY,
            adaptive_compression=True,
            compression_threshold_kb=100,  # Only compress files > 100KB
        ),
    )
    
    print("Creating collection with adaptive compression...")
    mixed_collection = client.create_collection(mixed_config.name, mixed_config)
    print(f"✅ Created: {mixed_collection.id}")
    
    return sst_collection, viper_collection, mixed_collection


def insert_and_search_compressed(client: ProximaDBClient, collection_name: str):
    """Insert vectors and perform compression-aware searches"""
    
    print(f"\n📝 Working with collection: {collection_name}")
    
    # Generate test data
    num_vectors = 1000
    dimension = 1536
    vectors = generate_random_vectors(num_vectors, dimension)
    
    # Insert vectors
    print(f"Inserting {num_vectors} vectors...")
    start_time = time.time()
    
    records = [
        VectorRecord(
            id=f"vec_{i}",
            vector=vectors[i],
            metadata={
                "category": f"cat_{i % 10}",
                "timestamp": int(time.time()),
                "compressed": True,
            }
        )
        for i in range(num_vectors)
    ]
    
    insert_response = client.insert_vectors(collection_name, records)
    insert_time = time.time() - start_time
    print(f"✅ Inserted {num_vectors} vectors in {insert_time:.2f}s")
    
    # Search with compression-aware optimization
    query_vector = generate_random_vectors(1, dimension)[0]
    
    # 1. Regular search
    print("\n🔍 Regular search...")
    start_time = time.time()
    regular_results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=10,
    )
    regular_time = time.time() - start_time
    print(f"Found {len(regular_results)} results in {regular_time:.3f}s")
    
    # 2. Compression-aware search with decompression cache
    print("\n🔍 Compression-aware search with cache...")
    search_optimization = SearchOptimization(
        enable_two_stage=True,
        use_decompression_cache=True,
        prefer_compressed_search=True,
        decompression_budget_ms=100,
        compression_aware_routing=True,
    )
    
    start_time = time.time()
    optimized_results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=10,
        search_optimization=search_optimization,
    )
    optimized_time = time.time() - start_time
    print(f"Found {len(optimized_results)} results in {optimized_time:.3f}s")
    
    # 3. Second search (should hit cache)
    print("\n🔍 Second search (cache hit expected)...")
    start_time = time.time()
    cached_results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=10,
        search_optimization=search_optimization,
    )
    cached_time = time.time() - start_time
    print(f"Found {len(cached_results)} results in {cached_time:.3f}s")
    
    # Compare performance
    print("\n📊 Performance Comparison:")
    print(f"  Regular search: {regular_time:.3f}s")
    print(f"  Optimized search: {optimized_time:.3f}s")
    print(f"  Cached search: {cached_time:.3f}s")
    if cached_time < regular_time:
        speedup = regular_time / cached_time
        print(f"  🚀 Cache speedup: {speedup:.1f}x")


def demonstrate_adaptive_compression(client: ProximaDBClient):
    """Demonstrate adaptive compression based on data characteristics"""
    
    print("\n🎯 Demonstrating Adaptive Compression")
    
    # Create collection with adaptive compression
    config = CollectionConfig(
        name="adaptive_demo_collection",
        dimension=512,
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.SST,
        compression_config=CompressionConfig(
            sst_compression_algorithm=CompressionAlgorithm.ZSTD,
            sst_compression_level=3,  # Fast compression
            adaptive_compression=True,
            compression_threshold_kb=10,  # Low threshold for demo
        ),
    )
    
    collection = client.create_collection(config)
    print(f"✅ Created adaptive collection: {collection.id}")
    
    # Insert sparse vectors (should compress well)
    print("\nInserting sparse vectors (high compressibility)...")
    sparse_vectors = []
    for i in range(100):
        vec = np.zeros(512)
        vec[np.random.choice(512, 10)] = np.random.randn(10)  # Only 10 non-zero values
        sparse_vectors.append(vec.tolist())
    
    sparse_records = [
        VectorRecord(id=f"sparse_{i}", vector=v, metadata={"type": "sparse"})
        for i, v in enumerate(sparse_vectors)
    ]
    client.insert_vectors(collection.name, sparse_records)
    
    # Insert dense vectors (lower compressibility)
    print("Inserting dense vectors (low compressibility)...")
    dense_vectors = generate_random_vectors(100, 512)
    dense_records = [
        VectorRecord(id=f"dense_{i}", vector=v, metadata={"type": "dense"})
        for i, v in enumerate(dense_vectors)
    ]
    client.insert_vectors(collection.name, dense_records)
    
    print("✅ Adaptive compression will optimize based on data characteristics")


def main():
    """Main example function"""
    print("=" * 60)
    print("ProximaDB SDK-Driven Compression Example")
    print("=" * 60)
    
    # Initialize client
    client = ProximaDBClient(
        url="http://localhost:5678",
        grpc_url="http://localhost:5679",
    )
    
    try:
        # Create collections with different compression configs
        sst_col, viper_col, mixed_col = create_compressed_collections(client)
        
        # Test compression-aware operations
        insert_and_search_compressed(client, sst_col.name)
        insert_and_search_compressed(client, viper_col.name)
        
        # Demonstrate adaptive compression
        demonstrate_adaptive_compression(client)
        
        print("\n✅ Compression example completed successfully!")
        
    except Exception as e:
        print(f"❌ Error: {e}")
    finally:
        # Cleanup (optional)
        print("\n🧹 Cleaning up...")
        try:
            client.delete_collection("compressed_sst_collection")
            client.delete_collection("compressed_viper_collection")
            client.delete_collection("adaptive_compression_collection")
            client.delete_collection("adaptive_demo_collection")
        except:
            pass


if __name__ == "__main__":
    main()