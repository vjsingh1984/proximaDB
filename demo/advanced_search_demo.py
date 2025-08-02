#!/usr/bin/env python3
"""
ProximaDB Advanced Search Demo - Metadata Filters and Multi-Vector Text Search
"""

import time
import numpy as np
from proximadb import connect_rest, connect_grpc
from proximadb.models import CollectionConfig, DistanceMetric, VectorRecord, StorageEngine

def create_text_chunks(text, chunk_size=512, overlap=64):
    """
    Split large text into overlapping chunks for multi-vector representation.
    In production, you'd use a text embedding model here.
    """
    chunks = []
    for i in range(0, len(text), chunk_size - overlap):
        chunk = text[i:i + chunk_size]
        chunks.append(chunk)
    return chunks

def simulate_text_embedding(text, dimension=128):
    """
    Simulate text embedding. In production, use a real embedding model
    like sentence-transformers, OpenAI, or Cohere.
    """
    # Simple simulation: use text length and hash for deterministic vectors
    np.random.seed(hash(text) % 2**32)
    return np.random.rand(dimension).astype(np.float32).tolist()

def demo_metadata_filters(client, collection_name, protocol="gRPC"):
    """Demonstrate working metadata filter examples"""
    print(f"\n🔍 Metadata Filter Examples via {protocol}")
    print("=" * 60)
    
    query_vector = np.random.rand(128).astype(np.float32).tolist()
    
    # Example 1: Simple equality filter
    print("\n📌 Example 1: Simple Equality Filter")
    print("Filter: category = 'electronics'")
    
    results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=5,
        metadata_filter={"category": "electronics"},
        include_metadata=True
    )
    
    print(f"Found {len(results)} results:")
    for r in results[:3]:
        print(f"  - {r.id}: {r.metadata.get('title')} (${r.metadata.get('price')})")
    
    # Example 2: Multiple conditions (implicit AND)
    print("\n📌 Example 2: Multiple Conditions (Implicit AND)")
    print("Filter: category = 'electronics' AND price < 500")
    
    # Since comparison operators might not work, let's use a different approach
    results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=10,
        metadata_filter={"category": "electronics"},
        include_metadata=True
    )
    
    # Client-side filtering for price
    affordable = [r for r in results if r.metadata.get('price', 0) < 500]
    print(f"Found {len(affordable)} affordable electronics:")
    for r in affordable[:3]:
        print(f"  - {r.id}: {r.metadata.get('title')} (${r.metadata.get('price')})")
    
    # Example 3: Brand filtering
    print("\n📌 Example 3: Brand Filter")
    print("Filter: brand = 'Apple'")
    
    results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=5,
        metadata_filter={"brand": "Apple"},
        include_metadata=True
    )
    
    print(f"Found {len(results)} Apple products:")
    for r in results[:3]:
        print(f"  - {r.id}: {r.metadata.get('title')}")

def demo_multi_vector_search(client, collection_name):
    """Demonstrate multi-vector search for large text"""
    print("\n📚 Multi-Vector Text Search Demo")
    print("=" * 60)
    
    # Example: Large document about smartphones
    large_document = """
    The evolution of smartphones has transformed how we communicate and access information. 
    Starting with basic mobile phones in the 1980s, the industry has seen remarkable growth. 
    The introduction of the iPhone in 2007 marked a turning point, bringing touch screens 
    and app ecosystems to the mainstream. Android followed shortly after, creating a 
    competitive market that drove rapid innovation.
    
    Modern smartphones feature powerful processors, high-resolution cameras, and AI capabilities. 
    5G technology promises even faster connections and new possibilities for mobile computing. 
    Privacy and security have become major concerns as these devices store increasingly 
    sensitive personal information. The future may bring foldable screens, augmented reality, 
    and even more integration with our daily lives.
    """
    
    # Split into chunks
    chunks = create_text_chunks(large_document, chunk_size=200, overlap=50)
    print(f"\n📄 Document split into {len(chunks)} chunks")
    
    # Create vectors for each chunk
    doc_id = "doc_smartphone_history"
    chunk_records = []
    
    for i, chunk in enumerate(chunks):
        vector = simulate_text_embedding(chunk)
        metadata = {
            "document_id": doc_id,
            "chunk_index": i,
            "chunk_text": chunk[:100] + "...",  # Store preview
            "document_type": "article",
            "topic": "technology"
        }
        
        record = VectorRecord(
            id=f"{doc_id}_chunk_{i}",
            vector=vector,
            metadata=metadata
        )
        chunk_records.append(record)
    
    # Insert chunks
    print("📤 Inserting document chunks...")
    client.upsert_vectors(
        collection_id=collection_name,
        records=chunk_records
    )
    
    # Search with a query
    query_text = "iPhone innovation and impact on mobile industry"
    query_vector = simulate_text_embedding(query_text)
    
    print(f"\n🔍 Searching for: '{query_text}'")
    
    results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=3,
        metadata_filter={"document_id": doc_id},
        include_metadata=True
    )
    
    print(f"\nFound {len(results)} relevant chunks:")
    for r in results:
        print(f"\n  Chunk {r.metadata.get('chunk_index')}:")
        print(f"  Score: {r.score:.4f}")
        print(f"  Text: {r.metadata.get('chunk_text')}")
    
    # Aggregate results by document
    print("\n📊 Document-Level Results:")
    doc_scores = {}
    for r in results:
        doc_id = r.metadata.get('document_id')
        if doc_id not in doc_scores:
            doc_scores[doc_id] = []
        doc_scores[doc_id].append(r.score)
    
    for doc_id, scores in doc_scores.items():
        avg_score = sum(scores) / len(scores)
        max_score = max(scores)
        print(f"  Document: {doc_id}")
        print(f"  - Average relevance: {avg_score:.4f}")
        print(f"  - Best chunk score: {max_score:.4f}")
        print(f"  - Matching chunks: {len(scores)}")

def demo_hybrid_search(client, collection_name):
    """Demonstrate hybrid search combining vector similarity and metadata"""
    print("\n🔄 Hybrid Search Demo (Vector + Metadata)")
    print("=" * 60)
    
    # Create query for "affordable Apple laptops"
    query_text = "MacBook Pro laptop computer"
    query_vector = simulate_text_embedding(query_text)
    
    # Step 1: Vector search for relevant products
    print("\n1️⃣ Vector search for 'MacBook Pro laptop computer'")
    all_results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=20,
        include_metadata=True
    )
    
    print(f"Found {len(all_results)} semantically similar products")
    
    # Step 2: Filter by metadata
    print("\n2️⃣ Filtering by category and brand")
    filtered_results = [
        r for r in all_results 
        if r.metadata.get('category') == 'electronics' 
        and r.metadata.get('brand') == 'Apple'
    ]
    
    print(f"After filtering: {len(filtered_results)} Apple electronics")
    
    # Step 3: Further filter by price
    print("\n3️⃣ Finding affordable options (< $1500)")
    affordable = [
        r for r in filtered_results
        if r.metadata.get('price', float('inf')) < 1500
    ]
    
    print(f"Found {len(affordable)} affordable Apple electronics:")
    for r in affordable[:5]:
        print(f"  - {r.metadata.get('title')}: ${r.metadata.get('price')} (score: {r.score:.4f})")

def main():
    print("🚀 ProximaDB Advanced Search Demo")
    print("=" * 60)
    
    # Connect to ProximaDB
    print("\n📡 Connecting to ProximaDB...")
    grpc_client = connect_grpc(url="http://localhost:5679")
    rest_client = connect_rest(url="http://localhost:5678")
    
    # Create collection
    collection_name = "advanced_search_demo"
    dimension = 128
    
    print(f"\n📦 Creating collection '{collection_name}'...")
    try:
        collection = grpc_client.create_collection(
            name=collection_name,
            dimension=dimension,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,
            filterable_metadata_fields=["category", "brand", "document_id", "price"]
        )
        print("✅ Collection created")
    except Exception as e:
        if "already exists" in str(e):
            print("📁 Using existing collection")
        else:
            raise
    
    # Insert sample products
    print("\n📝 Inserting sample products...")
    products = [
        {"title": "iPhone 14 Pro", "category": "electronics", "brand": "Apple", "price": 999},
        {"title": "MacBook Air M2", "category": "electronics", "brand": "Apple", "price": 1199},
        {"title": "iPad Pro 12.9", "category": "electronics", "brand": "Apple", "price": 1099},
        {"title": "AirPods Pro", "category": "electronics", "brand": "Apple", "price": 249},
        {"title": "Apple Watch Series 8", "category": "electronics", "brand": "Apple", "price": 399},
        {"title": "Samsung Galaxy S23", "category": "electronics", "brand": "Samsung", "price": 899},
        {"title": "Dell XPS 13", "category": "electronics", "brand": "Dell", "price": 1299},
        {"title": "Sony WH-1000XM5", "category": "electronics", "brand": "Sony", "price": 399},
        {"title": "The Design of Everyday Things", "category": "books", "brand": "Norman", "price": 18},
        {"title": "Clean Code", "category": "books", "brand": "Martin", "price": 35},
    ]
    
    records = []
    for i, product in enumerate(products):
        # Create embedding based on title
        vector = simulate_text_embedding(product["title"])
        
        record = VectorRecord(
            id=f"product_{i:03d}",
            vector=vector,
            metadata=product
        )
        records.append(record)
    
    grpc_client.upsert_vectors(
        collection_id=collection_name,
        records=records
    )
    print(f"✅ Inserted {len(records)} products")
    
    # Wait for indexing
    time.sleep(1)
    
    # Run demos
    demo_metadata_filters(grpc_client, collection_name, "gRPC")
    demo_metadata_filters(rest_client, collection_name, "REST")
    demo_multi_vector_search(grpc_client, collection_name)
    demo_hybrid_search(grpc_client, collection_name)
    
    # Best practices summary
    print("\n📋 Best Practices for Advanced Search")
    print("=" * 60)
    print("\n1️⃣ **Metadata Filtering:**")
    print("   - Use simple equality filters (field = value)")
    print("   - Complex operators (AND/OR) may need client-side processing")
    print("   - Index frequently filtered fields with filterable_metadata_fields")
    
    print("\n2️⃣ **Large Text Search:**")
    print("   - Split documents into overlapping chunks (512-1024 tokens)")
    print("   - Store document_id in metadata to group chunks")
    print("   - Use chunk_index to maintain order")
    print("   - Aggregate scores across chunks for document ranking")
    
    print("\n3️⃣ **Hybrid Search:**")
    print("   - Combine vector similarity with metadata filters")
    print("   - Use larger top_k then filter client-side if needed")
    print("   - Consider two-stage retrieval: broad vector search → precise filtering")
    
    print("\n4️⃣ **Performance Tips:**")
    print("   - Batch insert chunks from same document")
    print("   - Use gRPC for better performance")
    print("   - Cache frequently used query embeddings")
    print("   - Consider using LIMIT with OFFSET for pagination")
    
    print(f"\n✅ Demo complete! Collection '{collection_name}' retained for testing.")

if __name__ == "__main__":
    main()