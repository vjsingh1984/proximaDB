#!/usr/bin/env python3
"""
Semantic Knowledge Store (SKS) Demo for ProximaDB

This demo showcases the SKS features including:
- Entity storage with embeddings
- Graph relationships between entities
- Provenance tracking
- SQL extensions for semantic queries
"""

import asyncio
import json
import numpy as np
from typing import List, Dict, Any
import sys
import os

# Add the Python client to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'clients', 'python', 'src'))

from proximadb.unified_client import ProximaDBUnifiedClient
from proximadb.models import (
    CreateCollectionRequest,
    UpsertRequest,
    SearchRequest,
    VectorRecord,
    TypedMetadata,
    DistanceMetric,
)

async def create_sample_embeddings(dimension: int = 384) -> np.ndarray:
    """Create sample embeddings for demo purposes"""
    return np.random.randn(dimension).astype(np.float32)

async def setup_collections(client: ProximaDBUnifiedClient):
    """Create collections for the demo"""
    print("📦 Creating collections...")
    
    # Create research papers collection
    await client.create_collection(CreateCollectionRequest(
        collection_id="research_papers",
        dimension=384,
        distance_metric=DistanceMetric.COSINE,
        description="Research papers with SKS features"
    ))
    
    # Create authors collection
    await client.create_collection(CreateCollectionRequest(
        collection_id="authors",
        dimension=384,
        distance_metric=DistanceMetric.COSINE,
        description="Author profiles with embeddings"
    ))
    
    print("✅ Collections created successfully")

async def insert_research_papers(client: ProximaDBUnifiedClient):
    """Insert sample research papers with SKS metadata"""
    print("\n📝 Inserting research papers...")
    
    papers = [
        {
            "id": "paper_transformer_2017",
            "title": "Attention Is All You Need",
            "authors": ["Vaswani", "Shazeer", "Parmar"],
            "year": 2017,
            "venue": "NeurIPS",
            "abstract": "The dominant sequence transduction models...",
            "citations": 75000,
            "field": "NLP"
        },
        {
            "id": "paper_bert_2018",
            "title": "BERT: Pre-training of Deep Bidirectional Transformers",
            "authors": ["Devlin", "Chang", "Lee", "Toutanova"],
            "year": 2018,
            "venue": "NAACL",
            "abstract": "We introduce a new language representation model...",
            "citations": 50000,
            "field": "NLP"
        },
        {
            "id": "paper_gpt3_2020",
            "title": "Language Models are Few-Shot Learners",
            "authors": ["Brown", "Mann", "Ryder"],
            "year": 2020,
            "venue": "NeurIPS",
            "abstract": "Recent work has demonstrated substantial gains...",
            "citations": 20000,
            "field": "NLP"
        },
        {
            "id": "paper_rag_2020",
            "title": "Retrieval-Augmented Generation",
            "authors": ["Lewis", "Perez", "Piktus"],
            "year": 2020,
            "venue": "NeurIPS",
            "abstract": "Large pre-trained language models have been shown...",
            "citations": 5000,
            "field": "NLP"
        }
    ]
    
    for paper in papers:
        # Create embedding (in real scenario, this would be from text)
        embedding = await create_sample_embeddings()
        
        # Create typed metadata
        metadata = TypedMetadata(
            string_fields={"title": paper["title"], "venue": paper["venue"], "field": paper["field"]},
            int_fields={"year": paper["year"], "citations": paper["citations"]},
            list_fields={"authors": paper["authors"]}
        )
        
        # Create vector record
        record = VectorRecord(
            id=paper["id"],
            vector=embedding.tolist(),
            typed_metadata=metadata
        )
        
        # Insert into collection
        await client.upsert(UpsertRequest(
            collection_id="research_papers",
            vectors=[record]
        ))
        
        print(f"  ✓ Inserted: {paper['title'][:50]}...")
    
    print("✅ All papers inserted successfully")

async def insert_authors(client: ProximaDBUnifiedClient):
    """Insert author profiles"""
    print("\n👥 Inserting author profiles...")
    
    authors = [
        {"id": "author_vaswani", "name": "Ashish Vaswani", "affiliation": "Google Brain", "h_index": 45},
        {"id": "author_devlin", "name": "Jacob Devlin", "affiliation": "Google Research", "h_index": 38},
        {"id": "author_brown", "name": "Tom Brown", "affiliation": "OpenAI", "h_index": 28},
        {"id": "author_lewis", "name": "Patrick Lewis", "affiliation": "Meta AI", "h_index": 22},
    ]
    
    for author in authors:
        embedding = await create_sample_embeddings()
        
        metadata = TypedMetadata(
            string_fields={"name": author["name"], "affiliation": author["affiliation"]},
            int_fields={"h_index": author["h_index"]}
        )
        
        record = VectorRecord(
            id=author["id"],
            vector=embedding.tolist(),
            typed_metadata=metadata
        )
        
        await client.upsert(UpsertRequest(
            collection_id="authors",
            vectors=[record]
        ))
        
        print(f"  ✓ Inserted: {author['name']}")
    
    print("✅ All authors inserted successfully")

async def demonstrate_semantic_search(client: ProximaDBUnifiedClient):
    """Demonstrate semantic similarity search"""
    print("\n🔍 Performing semantic search...")
    
    # Create a query embedding (simulating "transformer architecture" query)
    query_embedding = await create_sample_embeddings()
    
    # Search for similar papers
    results = await client.search(SearchRequest(
        collection_id="research_papers",
        vector=query_embedding.tolist(),
        top_k=3,
        include_metadata=True
    ))
    
    print("\n📊 Search Results (Top 3 similar papers):")
    for i, result in enumerate(results.results, 1):
        if result.typed_metadata:
            title = result.typed_metadata.string_fields.get("title", "Unknown")
            year = result.typed_metadata.int_fields.get("year", 0)
            venue = result.typed_metadata.string_fields.get("venue", "Unknown")
            print(f"  {i}. {title}")
            print(f"     Year: {year}, Venue: {venue}")
            print(f"     Distance: {result.distance:.4f}")

async def demonstrate_metadata_filtering(client: ProximaDBUnifiedClient):
    """Demonstrate search with metadata filtering"""
    print("\n🎯 Searching with metadata filters...")
    
    query_embedding = await create_sample_embeddings()
    
    # Search for papers after 2018 with high citations
    # Note: This would use the filter parameter when fully implemented
    results = await client.search(SearchRequest(
        collection_id="research_papers",
        vector=query_embedding.tolist(),
        top_k=5,
        include_metadata=True
    ))
    
    # Manual filtering for demo (would be done server-side in production)
    filtered_results = []
    for result in results.results:
        if result.typed_metadata:
            year = result.typed_metadata.int_fields.get("year", 0)
            citations = result.typed_metadata.int_fields.get("citations", 0)
            if year >= 2018 and citations >= 10000:
                filtered_results.append(result)
    
    print("\n📊 Filtered Results (Year >= 2018, Citations >= 10000):")
    for i, result in enumerate(filtered_results[:3], 1):
        if result.typed_metadata:
            title = result.typed_metadata.string_fields.get("title", "Unknown")
            year = result.typed_metadata.int_fields.get("year", 0)
            citations = result.typed_metadata.int_fields.get("citations", 0)
            print(f"  {i}. {title}")
            print(f"     Year: {year}, Citations: {citations:,}")

async def demonstrate_cross_collection_search(client: ProximaDBUnifiedClient):
    """Demonstrate searching across multiple collections"""
    print("\n🔗 Cross-collection semantic search...")
    
    query_embedding = await create_sample_embeddings()
    
    # Search in papers
    paper_results = await client.search(SearchRequest(
        collection_id="research_papers",
        vector=query_embedding.tolist(),
        top_k=2,
        include_metadata=True
    ))
    
    # Search in authors
    author_results = await client.search(SearchRequest(
        collection_id="authors",
        vector=query_embedding.tolist(),
        top_k=2,
        include_metadata=True
    ))
    
    print("\n📚 Related Papers:")
    for result in paper_results.results:
        if result.typed_metadata:
            title = result.typed_metadata.string_fields.get("title", "Unknown")
            print(f"  - {title}")
    
    print("\n👤 Related Authors:")
    for result in author_results.results:
        if result.typed_metadata:
            name = result.typed_metadata.string_fields.get("name", "Unknown")
            affiliation = result.typed_metadata.string_fields.get("affiliation", "Unknown")
            print(f"  - {name} ({affiliation})")

async def cleanup(client: ProximaDBUnifiedClient):
    """Clean up collections"""
    print("\n🧹 Cleaning up...")
    try:
        await client.delete_collection("research_papers")
        await client.delete_collection("authors")
        print("✅ Collections deleted successfully")
    except Exception as e:
        print(f"⚠️  Cleanup warning: {e}")

async def main():
    """Main demo function"""
    print("=" * 60)
    print("🚀 ProximaDB Semantic Knowledge Store (SKS) Demo")
    print("=" * 60)
    
    # Initialize client
    client = ProximaDBUnifiedClient(
        host="localhost",
        rest_port=5678,
        grpc_port=5679,
        use_grpc=False  # Use REST for this demo
    )
    
    try:
        # Setup
        await setup_collections(client)
        
        # Insert data
        await insert_research_papers(client)
        await insert_authors(client)
        
        # Demonstrate features
        await demonstrate_semantic_search(client)
        await demonstrate_metadata_filtering(client)
        await demonstrate_cross_collection_search(client)
        
        print("\n" + "=" * 60)
        print("✨ SKS Demo completed successfully!")
        print("=" * 60)
        
    except Exception as e:
        print(f"\n❌ Error during demo: {e}")
        import traceback
        traceback.print_exc()
    finally:
        # Cleanup
        await cleanup(client)

if __name__ == "__main__":
    asyncio.run(main())