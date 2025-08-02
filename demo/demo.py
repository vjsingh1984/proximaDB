#!/usr/bin/env python3
"""
ProximaDB Simple Demo - Showcases core functionality with gRPC for high performance
"""

import time
import numpy as np
from proximadb import connect_grpc
from proximadb.models import CollectionConfig, DistanceMetric, VectorRecord

def main():
    print("🚀 ProximaDB Simple Demo - Using gRPC for High Performance")
    print("=" * 60)
    
    # Connect to ProximaDB using gRPC
    print("\n1️⃣ Connecting to ProximaDB via gRPC...")
    client = connect_grpc(url="http://localhost:5679")
    
    # Check server health
    health = client.health()
    print(f"✅ Server status: {health.status}")
    
    # Create a collection
    print("\n2️⃣ Creating collection...")
    collection_name = "demo_collection_grpc"
    
    try:
        collection = client.create_collection(
            name=collection_name,
            dimension=384,  # Common embedding dimension
            distance_metric=DistanceMetric.COSINE,
            filterable_metadata_fields=["category", "price", "brand"]
        )
        print(f"✅ Collection created: {collection_name}")
    except Exception as e:
        if "already exists" in str(e):
            print(f"📁 Collection already exists: {collection_name}")
        else:
            raise
    
    # Generate demo vectors
    print("\n3️⃣ Generating demo vectors...")
    num_vectors = 1000
    dimension = 384
    
    # Create vector records with metadata
    records = []
    categories = ["electronics", "books", "clothing", "home", "sports"]
    brands = ["BrandA", "BrandB", "BrandC", "BrandD", "BrandE"]
    
    for i in range(num_vectors):
        vector = np.random.rand(dimension).astype(np.float32).tolist()
        metadata = {
            "category": categories[i % len(categories)],
            "price": float(np.random.randint(10, 1000)),
            "brand": brands[i % len(brands)],
            "item_id": f"item_{i:04d}"
        }
        
        record = VectorRecord(
            id=f"vec_{i:04d}",
            vector=vector,
            metadata=metadata
        )
        records.append(record)
    
    print(f"✅ Generated {num_vectors} vectors with metadata")
    
    # Batch upsert vectors - showcasing gRPC's efficiency with large batches
    print("\n4️⃣ Upserting vectors in batches (gRPC optimized)...")
    batch_size = 100  # gRPC can handle larger batches efficiently
    total_time = 0
    
    for i in range(0, len(records), batch_size):
        batch = records[i:i+batch_size]
        start_time = time.time()
        
        result = client.upsert_vectors(
            collection_id=collection_name,
            records=batch
        )
        
        batch_time = time.time() - start_time
        total_time += batch_time
        
        if i % 500 == 0:
            print(f"  📊 Processed {i+batch_size} vectors...")
    
    vectors_per_second = num_vectors / total_time
    print(f"✅ Upserted {num_vectors} vectors in {total_time:.2f}s ({vectors_per_second:.0f} vectors/sec)")
    
    # Perform vector search
    print("\n5️⃣ Performing vector similarity search...")
    query_vector = np.random.rand(dimension).astype(np.float32).tolist()
    
    start_time = time.time()
    results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=10,
        include_metadata=True
    )
    search_time = time.time() - start_time
    
    print(f"✅ Search completed in {search_time*1000:.2f}ms")
    print(f"📊 Top 5 results:")
    for i, result in enumerate(results[:5]):
        metadata = result.metadata
        print(f"  {i+1}. ID: {result.id}, Score: {result.score:.4f}")
        print(f"     Category: {metadata.get('category')}, Price: ${metadata.get('price')}")
    
    # Search with metadata filter
    print("\n6️⃣ Search with metadata filtering...")
    start_time = time.time()
    filtered_results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=10,
        metadata_filter={"category": "electronics"},
        include_metadata=True
    )
    filter_time = time.time() - start_time
    
    print(f"✅ Filtered search completed in {filter_time*1000:.2f}ms")
    print(f"📊 Found {len(filtered_results)} electronics items")
    
    # Performance comparison
    print("\n7️⃣ Performance Summary:")
    print(f"  • gRPC Connection: ✅")
    print(f"  • Insert throughput: {vectors_per_second:.0f} vectors/sec")
    print(f"  • Search latency: {search_time*1000:.2f}ms")
    print(f"  • Filtered search: {filter_time*1000:.2f}ms")
    
    # Cleanup (optional)
    print("\n8️⃣ Demo complete! Collection retained for further testing.")
    print(f"   To delete: client.delete_collection('{collection_name}')")

if __name__ == "__main__":
    main()