#!/usr/bin/env python3
"""
Advanced Search Example for ProximaDB Python SDK v1.0

This example demonstrates advanced search features:
- Complex metadata filtering
- SQL queries with vector similarity
- Hybrid search (vector + metadata)
- Search result caching
- Pagination and streaming
"""

import asyncio
import json
import time
import numpy as np
from datetime import datetime, timedelta
from typing import List, Dict, Any

from proximadb import ProximaDBClient, ClientConfig
from proximadb.models import (
    CollectionConfig,
    VectorRecord,
    SearchOptions,
    FilterCondition,
    FilterOperator,
    DistanceMetric,
    StorageEngine
)
from proximadb.streaming import SearchStream


def generate_product_embeddings(num_products: int) -> List[Dict[str, Any]]:
    """Generate synthetic product data with embeddings"""
    categories = ["Electronics", "Books", "Clothing", "Home & Garden", "Sports"]
    brands = ["TechCorp", "BookWorld", "FashionHub", "HomeStyle", "SportPro"]
    
    products = []
    for i in range(num_products):
        # Generate clean BERT embedding without artificial clustering
        category_idx = i % len(categories)
        
        product = {
            "id": f"product_{i:05d}",
            "embedding": base_embedding.tolist(),
            "metadata": {
                "name": f"Product {i}",
                "category": categories[category_idx],
                "brand": brands[category_idx],
                "price": round(np.random.uniform(10, 1000), 2),
                "rating": round(np.random.uniform(3.0, 5.0), 1),
                "reviews": int(np.random.randint(0, 5000)),
                "in_stock": bool(np.random.random() > 0.2),
                "created_at": (
                    datetime.now() - timedelta(days=int(np.random.randint(0, 365)))
                ).isoformat(),
                "tags": np.random.choice(
                    ["popular", "sale", "new", "featured", "limited"],
                    size=np.random.randint(0, 3),
                    replace=False
                ).tolist()
            }
        }
        products.append(product)
    
    return products


async def setup_collection(client: ProximaDBClient, collection_name: str) -> None:
    """Set up collection with product data"""
    print(f"📦 Setting up collection '{collection_name}'...")
    
    # Create collection optimized for search
    config = CollectionConfig(
        name=collection_name,
        dimension=384,  # Common embedding dimension
        distance_metric=DistanceMetric.COSINE,
        storage_engine=StorageEngine.VIPER,  # Columnar for metadata filtering
        metadata={
            "description": "Product catalog for advanced search demo",
            "index_type": "hnsw",
            "index_params": {"m": 16, "ef_construction": 200}
        }
    )
    
    try:
        await client.adelete_collection(collection_name)
    except:
        pass  # Collection might not exist
    
    collection = await client.acreate_collection(config)
    print(f"✅ Collection created: {collection.id}")
    
    # Insert product data
    print("📝 Inserting product data...")
    products = get_product_data_with_bert_embeddings(50)  # Reduced for better performance
    
    vectors = [
        VectorRecord(
            id=p["id"],
            vector=p["embedding"],
            metadata=p["metadata"]
        )
        for p in products
    ]
    
    # Batch insert
    batch_size = 100
    for i in range(0, len(vectors), batch_size):
        batch = vectors[i:i + batch_size]
        response = await client.ainsert_vectors(collection_name, batch)
        print(f"   Inserted batch {i//batch_size + 1}/{len(vectors)//batch_size + 1}")
    
    print("✅ Product data inserted successfully")
    return products


async def demo_basic_filtering(client: ProximaDBClient, collection_name: str, 
                              query_vector: List[float]) -> None:
    """Demonstrate basic metadata filtering"""
    print("\n🔍 Example 1: Basic Metadata Filtering")
    print("=" * 50)
    
    # Search for electronics under $500
    options = SearchOptions(
        top_k=5,
        filter_conditions=[
            FilterCondition(
                field="metadata.category",
                operator=FilterOperator.EQUALS,
                value="Electronics"
            ),
            FilterCondition(
                field="metadata.price",
                operator=FilterOperator.LESS_THAN,
                value=500
            )
        ],
        include_metadata=True
    )
    
    results = await client.asearch_vectors(
        collection_name,
        query_vector,
        options=options
    )
    
    print("📱 Electronics under $500:")
    for i, result in enumerate(results.results):
        print(f"{i+1}. {result.metadata['name']} - ${result.metadata['price']}")
        print(f"   Brand: {result.metadata['brand']}, Rating: ⭐ {result.metadata['rating']}")


async def demo_complex_filtering(client: ProximaDBClient, collection_name: str,
                                query_vector: List[float]) -> None:
    """Demonstrate complex compound filters"""
    print("\n🔍 Example 2: Complex Compound Filtering")
    print("=" * 50)
    
    # High-rated products that are either on sale or new, in stock
    options = SearchOptions(
        top_k=10,
        filter_conditions=[
            FilterCondition(
                field="metadata.rating",
                operator=FilterOperator.GREATER_THAN_OR_EQUAL,
                value=4.5
            ),
            FilterCondition(
                field="metadata.in_stock",
                operator=FilterOperator.EQUALS,
                value=True
            ),
            FilterCondition(
                field="metadata.reviews",
                operator=FilterOperator.GREATER_THAN,
                value=100
            )
        ],
        include_metadata=True
    )
    
    results = await client.asearch_vectors(
        collection_name,
        query_vector,
        options=options
    )
    
    print("⭐ High-rated products with 100+ reviews in stock:")
    for i, result in enumerate(results.results[:5]):
        print(f"{i+1}. {result.metadata['name']} (Score: {result.score:.3f})")
        print(f"   Rating: ⭐ {result.metadata['rating']} from {result.metadata['reviews']} reviews")
        print(f"   Price: ${result.metadata['price']}, Tags: {result.metadata.get('tags', [])}")


async def demo_sql_search(client: ProximaDBClient, collection_name: str,
                         query_vector: List[float]) -> None:
    """Demonstrate SQL-based vector search"""
    print("\n🔍 Example 3: SQL-based Vector Search")
    print("=" * 50)
    
    # SQL query with vector similarity
    sql_query = f"""
    SELECT id, metadata.name, metadata.category, metadata.price, metadata.rating
    FROM {collection_name}
    WHERE metadata.in_stock = true
      AND metadata.category IN ('Electronics', 'Books')
      AND metadata.price BETWEEN 50 AND 500
      AND metadata.rating >= 4.0
    ORDER BY VECTOR_SIMILARITY(vector, :query_vector, 'cosine')
    LIMIT 10
    """
    
    try:
        results = await client.aexecute_sql(
            sql_query,
            parameters={"query_vector": query_vector}
        )
        
        print("📊 SQL Query Results:")
        print("Category      | Product Name           | Price   | Rating | Score")
        print("-" * 70)
        
        for row in results.rows:
            print(f"{row['category']:12} | {row['name']:20} | ${row['price']:6.2f} | "
                  f"⭐ {row['rating']} | {row.get('_score', 0):.3f}")
    except Exception as e:
        print(f"⚠️  SQL search not available: {e}")
        print("   Make sure ProximaDB server supports SQL queries")


async def demo_hybrid_search(client: ProximaDBClient, collection_name: str) -> None:
    """Demonstrate hybrid search combining multiple signals"""
    print("\n🔍 Example 4: Hybrid Search (Text + Vector + Filters)")
    print("=" * 50)
    
    # Simulate a text query converted to embedding
    # In real usage, you'd use a text embedding model
    text_query = "high-end gaming laptop"
    
    # Create honest query embedding from real text (no artificial bias)
    query_text = "gaming laptop with high performance graphics"
    print(f"🔍 Searching for: '{query_text}'")
    query_embedding = generate_query_embedding(query_text)
    # No artificial bias - let BERT semantic similarity work naturally
    
    # Multi-stage search
    print(f"🔎 Searching for: '{text_query}'")
    
    # Stage 1: Broad vector search
    broad_results = await client.asearch_vectors(
        collection_name,
        query_embedding.tolist(),
        top_k=50  # Get more candidates
    )
    
    # Stage 2: Refined search with strict filters
    refined_options = SearchOptions(
        top_k=5,
        filter_conditions=[
            FilterCondition(
                field="metadata.category",
                operator=FilterOperator.EQUALS,
                value="Electronics"
            ),
            FilterCondition(
                field="metadata.price",
                operator=FilterOperator.GREATER_THAN,
                value=500  # High-end
            ),
            FilterCondition(
                field="metadata.rating",
                operator=FilterOperator.GREATER_THAN_OR_EQUAL,
                value=4.0
            )
        ],
        include_metadata=True
    )
    
    refined_results = await client.asearch_vectors(
        collection_name,
        query_embedding.tolist(),
        options=refined_options
    )
    
    print("\n💎 Top high-end electronics matches:")
    for i, result in enumerate(refined_results.results):
        print(f"{i+1}. {result.metadata['name']} - ${result.metadata['price']}")
        print(f"   Similarity: {result.score:.3f}, Rating: ⭐ {result.metadata['rating']}")


async def demo_search_caching(client: ProximaDBClient, collection_name: str,
                             query_vector: List[float]) -> None:
    """Demonstrate search result caching"""
    print("\n🔍 Example 5: Search Result Caching")
    print("=" * 50)
    
    # Enable caching (if using ResilientProximaDBClient)
    print("⏱️  Testing search performance with caching...")
    
    search_options = SearchOptions(
        top_k=10,
        filter_conditions=[
            FilterCondition(
                field="metadata.category",
                operator=FilterOperator.EQUALS,
                value="Electronics"
            )
        ]
    )
    
    # First search (cache miss)
    start = time.time()
    results1 = await client.asearch_vectors(
        collection_name,
        query_vector,
        options=search_options
    )
    time1 = time.time() - start
    print(f"❄️  Cold search time: {time1*1000:.2f}ms")
    
    # Immediate second search (potential cache hit)
    start = time.time()
    results2 = await client.asearch_vectors(
        collection_name,
        query_vector,
        options=search_options
    )
    time2 = time.time() - start
    print(f"🔥 Warm search time: {time2*1000:.2f}ms")
    
    if time2 < time1 * 0.5:
        print(f"✅ Cache hit! {(1 - time2/time1)*100:.1f}% faster")
    else:
        print("ℹ️  No significant caching detected (enable with ResilientProximaDBClient)")


async def demo_streaming_search(client: ProximaDBClient, collection_name: str,
                               query_vector: List[float]) -> None:
    """Demonstrate streaming search for large result sets"""
    print("\n🔍 Example 6: Streaming Search Results")
    print("=" * 50)
    
    print("📡 Streaming search results with pagination...")
    
    # Create search stream
    search_stream = SearchStream(
        client,
        collection_name=collection_name,
        query_vector=query_vector,
        page_size=20,      # Results per page
        max_results=100    # Total results to retrieve
    )
    
    # Process results as they stream in
    total_processed = 0
    categories_count = {}
    
    async for result in search_stream:
        category = result.metadata.get("category", "Unknown")
        categories_count[category] = categories_count.get(category, 0) + 1
        total_processed += 1
        
        # Show progress every 20 results
        if total_processed % 20 == 0:
            print(f"   Processed {total_processed} results...")
    
    print(f"\n✅ Streamed {total_processed} total results")
    print("📊 Category distribution:")
    for category, count in sorted(categories_count.items(), key=lambda x: x[1], reverse=True):
        print(f"   - {category}: {count} products")


async def demo_optimization_hints(client: ProximaDBClient, collection_name: str,
                                 query_vector: List[float]) -> None:
    """Demonstrate search optimization hints"""
    print("\n🔍 Example 7: Search Optimization Hints")
    print("=" * 50)
    
    from proximadb.models import OptimizationHints
    
    # Search with optimization hints
    options = SearchOptions(
        top_k=10,
        filter_conditions=[
            FilterCondition(
                field="metadata.category",
                operator=FilterOperator.IN,
                value=["Electronics", "Books"]
            )
        ],
        optimization_hints=OptimizationHints(
            use_approximate_search=True,     # Trade accuracy for speed
            predicate_pushdown=True,        # Push filters to storage
            parallel_execution=True,        # Use parallel processing
            cache_results=True,             # Cache for repeated queries
            ef_search=200                   # HNSW parameter for recall/speed
        ),
        include_metadata=True
    )
    
    start = time.time()
    results = await client.asearch_vectors(
        collection_name,
        query_vector,
        options=options
    )
    elapsed = time.time() - start
    
    print(f"⚡ Optimized search completed in {elapsed*1000:.2f}ms")
    print(f"Found {len(results.results)} results with optimization hints")
    
    # Show query execution stats if available
    if hasattr(results, 'query_stats'):
        print("\n📊 Query execution statistics:")
        stats = results.query_stats
        print(f"   - Vectors scanned: {stats.get('vectors_scanned', 'N/A')}")
        print(f"   - Filter efficiency: {stats.get('filter_efficiency', 'N/A')}")
        print(f"   - Cache hit: {stats.get('cache_hit', False)}")


async def main():
    # Initialize client
    print("🚀 Advanced Search Example for ProximaDB")
    print("=" * 50)
    
    client = ProximaDBClient(
        ClientConfig(
            url="http://localhost:5678",
            timeout=60.0  # Longer timeout for complex queries
        )
    )
    
    collection_name = "product_catalog"
    
    try:
        # Set up collection with data
        products = await setup_collection(client, collection_name)
        
        # Use first product's embedding as query
        query_vector = products[0]["embedding"]
        
        # Run all demos
        await demo_basic_filtering(client, collection_name, query_vector)
        await demo_complex_filtering(client, collection_name, query_vector)
        await demo_sql_search(client, collection_name, query_vector)
        await demo_hybrid_search(client, collection_name)
        await demo_search_caching(client, collection_name, query_vector)
        await demo_streaming_search(client, collection_name, query_vector)
        await demo_optimization_hints(client, collection_name, query_vector)
        
        print("\n✅ All advanced search examples completed!")
        
    finally:
        # Cleanup
        print("\n🧹 Cleaning up...")
        try:
            await client.adelete_collection(collection_name)
            print("✅ Demo collection deleted")
        except Exception as e:
            print(f"⚠️  Cleanup failed: {e}")


if __name__ == "__main__":
    asyncio.run(main())