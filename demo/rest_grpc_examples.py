#!/usr/bin/env python3
"""
ProximaDB REST and gRPC Examples - Metadata Filters and Multi-Vector Search
"""

import json
import time
import numpy as np
import requests
from proximadb import connect_rest, connect_grpc
from proximadb.models import CollectionConfig, DistanceMetric, VectorRecord, StorageEngine

def example_grpc_metadata_filter():
    """gRPC example with metadata filters"""
    print("\n🔷 gRPC Example - Metadata Filtering")
    print("=" * 60)
    
    # Connect via gRPC
    client = connect_grpc(url="http://localhost:5679")
    
    # Create collection
    collection_name = "grpc_filter_example"
    try:
        client.create_collection(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,
            filterable_metadata_fields=["category", "brand", "price_range"]
        )
        print("✅ Collection created")
    except:
        print("📁 Using existing collection")
    
    # Insert sample data
    products = [
        {"id": "laptop_1", "name": "MacBook Pro", "category": "laptop", "brand": "Apple", "price_range": "high"},
        {"id": "laptop_2", "name": "ThinkPad X1", "category": "laptop", "brand": "Lenovo", "price_range": "high"},
        {"id": "laptop_3", "name": "Chromebook", "category": "laptop", "brand": "Google", "price_range": "low"},
        {"id": "phone_1", "name": "iPhone 14", "category": "phone", "brand": "Apple", "price_range": "high"},
        {"id": "phone_2", "name": "Pixel 7", "category": "phone", "brand": "Google", "price_range": "medium"},
    ]
    
    records = []
    for product in products:
        vector = np.random.rand(128).astype(np.float32).tolist()
        record = VectorRecord(
            id=product["id"],
            vector=vector,
            metadata={k: v for k, v in product.items() if k != "id"}
        )
        records.append(record)
    
    client.upsert_vectors(collection_id=collection_name, records=records)
    print(f"✅ Inserted {len(records)} products")
    
    # Example searches
    query_vector = np.random.rand(128).astype(np.float32).tolist()
    
    # 1. Simple filter
    print("\n1️⃣ Simple Filter: category = 'laptop'")
    results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=5,
        metadata_filter={"category": "laptop"},
        include_metadata=True
    )
    print(f"Found {len(results)} laptops")
    
    # 2. Brand filter  
    print("\n2️⃣ Brand Filter: brand = 'Apple'")
    results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=5,
        metadata_filter={"brand": "Apple"},
        include_metadata=True
    )
    print(f"Found {len(results)} Apple products")
    
    return collection_name

def example_rest_metadata_filter():
    """REST API example with metadata filters"""
    print("\n🔶 REST API Example - Metadata Filtering")
    print("=" * 60)
    
    base_url = "http://localhost:5678"
    collection_name = "rest_filter_example"
    
    # Create collection via REST
    create_payload = {
        "name": collection_name,
        "dimension": 128,
        "distance_metric": "cosine",
        "storage_engine": "viper",
        "filterable_metadata_fields": ["category", "brand", "price_range"]
    }
    
    try:
        response = requests.post(f"{base_url}/api/v1/collections", json=create_payload)
        if response.status_code == 201:
            print("✅ Collection created via REST")
        else:
            print("📁 Using existing collection")
    except:
        print("📁 Using existing collection")
    
    # Insert data via REST
    products = [
        {"id": "laptop_1", "name": "MacBook Pro", "category": "laptop", "brand": "Apple", "price_range": "high"},
        {"id": "phone_1", "name": "iPhone 14", "category": "phone", "brand": "Apple", "price_range": "high"},
    ]
    
    for product in products:
        vector = np.random.rand(128).astype(np.float32).tolist()
        insert_payload = {
            "vectors": [{
                "id": product["id"],
                "vector": vector,
                "metadata": {k: v for k, v in product.items() if k != "id"}
            }]
        }
        
        response = requests.post(
            f"{base_url}/api/v1/collections/{collection_name}/vectors",
            json=insert_payload
        )
    
    print(f"✅ Inserted {len(products)} products via REST")
    
    # Search with filter
    query_vector = np.random.rand(128).astype(np.float32).tolist()
    
    # Example 1: Simple filter
    print("\n1️⃣ REST Search with Filter")
    search_payload = {
        "vector": query_vector,
        "top_k": 5,
        "metadata_filter": {"category": "laptop"},
        "include_metadata": True
    }
    
    response = requests.post(
        f"{base_url}/api/v1/collections/{collection_name}/search",
        json=search_payload
    )
    
    if response.status_code == 200:
        results = response.json()
        print(f"Found {len(results.get('results', []))} results")
        print(f"Response: {json.dumps(results, indent=2)[:200]}...")
    
    return collection_name

def example_multi_vector_text_search():
    """Example of handling large text with multiple vectors"""
    print("\n📚 Multi-Vector Text Search Example")
    print("=" * 60)
    
    client = connect_grpc(url="http://localhost:5679")
    collection_name = "text_search_example"
    
    # Create collection
    try:
        client.create_collection(
            name=collection_name,
            dimension=384,  # Common for sentence embeddings
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,
            filterable_metadata_fields=["doc_id", "chunk_idx", "doc_type"]
        )
        print("✅ Collection created")
    except:
        print("📁 Using existing collection")
    
    # Example: Large document about AI
    document = """
    Artificial Intelligence has revolutionized numerous industries. Machine learning 
    algorithms can now process vast amounts of data to identify patterns. Deep learning, 
    a subset of machine learning, uses neural networks to achieve remarkable results 
    in computer vision and natural language processing. Recent advances in transformer 
    architectures have led to breakthroughs in language models like GPT and BERT.
    """
    
    # Split into sentences (simplified chunking)
    sentences = [s.strip() for s in document.split('.') if s.strip()]
    print(f"\n📄 Document split into {len(sentences)} chunks")
    
    # Create vectors for each chunk
    doc_id = "ai_overview_001"
    records = []
    
    for idx, sentence in enumerate(sentences):
        # In production, use a real embedding model
        vector = np.random.rand(384).astype(np.float32).tolist()
        
        record = VectorRecord(
            id=f"{doc_id}_chunk_{idx}",
            vector=vector,
            metadata={
                "doc_id": doc_id,
                "chunk_idx": idx,
                "text": sentence,
                "doc_type": "article",
                "char_count": len(sentence)
            }
        )
        records.append(record)
    
    client.upsert_vectors(collection_id=collection_name, records=records)
    print(f"✅ Inserted {len(records)} text chunks")
    
    # Search for relevant chunks
    query = "deep learning and neural networks"
    query_vector = np.random.rand(384).astype(np.float32).tolist()
    
    print(f"\n🔍 Searching for: '{query}'")
    
    # Search all chunks from this document
    results = client.search(
        collection_id=collection_name,
        vector=query_vector,
        top_k=3,
        metadata_filter={"doc_id": doc_id},
        include_metadata=True
    )
    
    print(f"\nTop {len(results)} relevant chunks:")
    for r in results:
        print(f"  - Chunk {r.metadata.get('chunk_idx')}: {r.metadata.get('text')[:60]}...")
        print(f"    Score: {r.score:.4f}")
    
    return collection_name

def main():
    print("🚀 ProximaDB REST and gRPC Examples")
    print("=" * 60)
    
    # Run examples
    grpc_collection = example_grpc_metadata_filter()
    rest_collection = example_rest_metadata_filter()
    text_collection = example_multi_vector_text_search()
    
    # Summary and best practices
    print("\n📋 Summary and Best Practices")
    print("=" * 60)
    
    print("\n✅ **Working Features:**")
    print("- Basic equality filters (field = value)")
    print("- Single field filtering")
    print("- Multi-vector storage for documents")
    
    print("\n⚠️  **Current Limitations:**")
    print("- Complex AND/OR operators may not work as expected")
    print("- Comparison operators (>, <, >=, <=) limited support")
    print("- IN operator syntax varies")
    
    print("\n💡 **Multi-Vector Text Search Strategy:**")
    print("1. Split large text into chunks (sentences, paragraphs, or fixed size)")
    print("2. Create embeddings for each chunk")
    print("3. Store with doc_id metadata to group chunks")
    print("4. Search and aggregate results by document")
    print("5. Use chunk_idx to maintain order")
    
    print("\n🔧 **Implementation Tips:**")
    print("- Use real embedding models (sentence-transformers, OpenAI, etc.)")
    print("- Overlap chunks for better context (e.g., 10-20% overlap)")
    print("- Store chunk metadata for result presentation")
    print("- Consider hybrid retrieval: initial vector search + reranking")
    
    print(f"\n✅ Demo complete! Collections created:")
    print(f"  - {grpc_collection}")
    print(f"  - {rest_collection}")
    print(f"  - {text_collection}")

if __name__ == "__main__":
    main()