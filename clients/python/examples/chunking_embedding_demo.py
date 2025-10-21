"""
Demo: Separated Chunking and Embedding

This demo shows how to use the refactored chunking system with
complete separation between chunking and embedding operations.
"""

import os
import time
from typing import List

from proximadb import ProximaDBClient
from proximadb.chunking import (
    TextChunker,
    ChunkingConfig,
    ChunkingStrategy,
    create_vector_records,
    chunk_and_embed_text
)
from proximadb.embedding_providers import (
    get_provider,
    recommend_free_providers
)
from proximadb.models import CollectionConfig, DistanceMetric, StorageEngine


def demo_separation_of_concerns():
    """Demonstrate the separation between chunking and embedding"""
    
    print("=" * 60)
    print("DEMO: Separation of Chunking and Embedding")
    print("=" * 60)
    
    # Sample text
    text = """
    Machine learning is revolutionizing how we process and understand data.
    Neural networks, inspired by the human brain, can learn complex patterns
    from large datasets. Deep learning, a subset of machine learning, uses
    multiple layers to progressively extract higher-level features from raw input.
    
    Natural language processing (NLP) enables computers to understand, interpret,
    and generate human language. Recent advances in transformer models like BERT
    and GPT have dramatically improved language understanding capabilities.
    
    Computer vision allows machines to interpret and understand visual information
    from the world. Convolutional neural networks (CNNs) have been particularly
    successful in image classification, object detection, and segmentation tasks.
    """
    
    # Step 1: Chunking (independent of embeddings)
    print("\n1. CHUNKING TEXT")
    print("-" * 40)
    
    chunker = TextChunker(ChunkingConfig(
        strategy=ChunkingStrategy.PARAGRAPH,
        chunk_size=200,
        chunk_overlap=20
    ))
    
    chunks = chunker.chunk_text(text, source_id="ml_overview")
    
    print(f"Created {len(chunks)} chunks:")
    for i, chunk in enumerate(chunks):
        print(f"\nChunk {i + 1}:")
        print(f"  Text: {chunk.text[:80]}...")
        print(f"  Metadata: {chunk.metadata}")
    
    # Step 2: Embeddings (independent of chunking)
    print("\n\n2. GENERATING EMBEDDINGS")
    print("-" * 40)
    
    # Use simulated provider (no dependencies)
    embedding_provider = get_provider("simulated", dimension=384)
    print(f"Using embedding provider: {embedding_provider}")
    
    # Extract texts from chunks
    chunk_texts = [chunk.text for chunk in chunks]
    
    # Generate embeddings
    embeddings = embedding_provider.embed_texts(chunk_texts)
    print(f"Generated embeddings with shape: {embeddings.shape}")
    
    # Step 3: Create vector records (combine chunks + embeddings)
    print("\n\n3. CREATING VECTOR RECORDS")
    print("-" * 40)
    
    records = create_vector_records(
        chunks=chunks,
        embeddings=embeddings.tolist(),
        collection_metadata={"document_type": "tutorial", "topic": "ML"},
        filterable_fields=["document_type", "topic", "source_id"]
    )
    
    print(f"Created {len(records)} vector records")
    for i, record in enumerate(records):
        print(f"\nRecord {i + 1}:")
        print(f"  ID: {record.id}")
        print(f"  Vector dims: {len(record.vector)}")
        print(f"  Metadata: {record.metadata}")


def demo_different_strategies():
    """Show different chunking strategies"""
    
    print("\n\n" + "=" * 60)
    print("DEMO: Different Chunking Strategies")
    print("=" * 60)
    
    text = """
    ProximaDB Architecture. The system uses a modular design with pluggable components.
    
    Storage engines include SST for write-optimized workloads and VIPER for analytics.
    Each engine has different performance characteristics and use cases.
    
    The indexing layer supports multiple algorithms. HNSW provides fast approximate search.
    IVF enables partition-based retrieval. Flat index gives exact results.
    """
    
    strategies = [
        ChunkingStrategy.SLIDING_WINDOW,
        ChunkingStrategy.SENTENCE,
        ChunkingStrategy.PARAGRAPH,
        ChunkingStrategy.SEMANTIC,
    ]
    
    for strategy in strategies:
        print(f"\n{strategy.value.upper()} Strategy:")
        print("-" * 40)
        
        chunker = TextChunker(ChunkingConfig(
            strategy=strategy,
            chunk_size=150,
            chunk_overlap=20
        ))
        
        chunks = chunker.chunk_text(text, "architecture_doc")
        
        print(f"Created {len(chunks)} chunks:")
        for i, chunk in enumerate(chunks[:3]):  # Show first 3
            print(f"  Chunk {i + 1}: {chunk.text[:60]}...")


def demo_embedding_providers():
    """Show available embedding providers"""
    
    print("\n\n" + "=" * 60)
    print("DEMO: Embedding Providers")
    print("=" * 60)
    
    # Show recommendations
    print("\nRecommended free providers:")
    recommend_free_providers()
    
    # Test available providers
    print("\nTesting provider availability:")
    print("-" * 40)
    
    providers_to_test = [
        "simulated",
        "sentence-transformer",
        # Note: fastembed and instructor are not in the available provider list
    ]
    
    for provider_name in providers_to_test:
        try:
            provider = get_provider(provider_name)
            available = provider.is_available()
            
            if available:
                # Test with sample text
                test_embedding = provider.embed_text("test text")
                print(f"✓ {provider_name}: Available (dimension={len(test_embedding)})")
            else:
                print(f"✗ {provider_name}: Not installed")
                
        except Exception as e:
            print(f"✗ {provider_name}: Error - {e}")


def demo_complete_workflow():
    """Show complete workflow with ProximaDB"""
    
    print("\n\n" + "=" * 60)
    print("DEMO: Complete Workflow with ProximaDB")
    print("=" * 60)
    
    # Initialize client
    try:
        client = ProximaDBClient(url="http://localhost:5678")
        print("✓ Connected to ProximaDB")
    except Exception as e:
        print(f"✗ Could not connect to ProximaDB: {e}")
        print("  Make sure ProximaDB server is running")
        return
    
    # Collection name
    collection_name = "demo_chunking_embedding"
    
    # Clean up existing collection
    try:
        client.delete_collection(collection_name)
    except:
        pass
    
    # Create collection
    print(f"\nCreating collection: {collection_name}")
    collection = client.create_collection(
        name=collection_name,
        config=CollectionConfig(
            name=collection_name,
            dimension=384,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.SST
        )
    )
    print("✓ Collection created")
    
    # Sample document
    document = """
    Vector databases are specialized databases designed to store and search high-dimensional vectors.
    They are essential for modern AI applications including recommendation systems, semantic search,
    and similarity matching.
    
    ProximaDB offers advanced features like multiple storage engines, various indexing algorithms,
    and support for both REST and gRPC protocols. The system is designed for high performance
    and scalability.
    """
    
    # Use convenience function for complete workflow
    print("\nProcessing document...")
    
    embedding_provider = get_provider("simulated")
    
    records = chunk_and_embed_text(
        text=document,
        source_id="vector_db_intro",
        embedding_provider=embedding_provider,
        chunking_config=ChunkingConfig(
            strategy=ChunkingStrategy.PARAGRAPH,
            chunk_size=200
        ),
        metadata={"category": "documentation", "version": "1.0"},
        filterable_fields=["category", "version", "source_id"]
    )
    
    print(f"✓ Created {len(records)} vector records")
    
    # Insert into ProximaDB
    print("\nInserting vectors...")
    client.insert_vectors(collection_name, records)
    print("✓ Vectors inserted")
    
    # Search example
    print("\nSearching for similar content...")
    query_text = "What are vector databases used for?"
    query_embedding = embedding_provider.embed_text(query_text)
    
    results = client.search(
        collection_id=collection_name,
        vector=query_embedding.tolist(),
        top_k=3
    )

    print(f"✓ Found {len(results)} results")
    for i, result in enumerate(results[:3]):
        print(f"\nResult {i + 1}:")
        print(f"  Score: {result.score:.3f}")
        print(f"  Text: {result.metadata.get('text_preview', '')[:80]}...")
    
    # Cleanup
    print("\nCleaning up...")
    client.delete_collection(collection_name)
    print("✓ Collection deleted")

    # Allow graceful cleanup of gRPC connections
    time.sleep(0.5)


def main():
    """Run all demos"""
    
    # Show separation of concerns
    demo_separation_of_concerns()
    
    # Show different chunking strategies
    demo_different_strategies()
    
    # Show embedding providers
    demo_embedding_providers()
    
    # Show complete workflow (if ProximaDB is running)
    demo_complete_workflow()
    
    print("\n" + "=" * 60)
    print("Demo completed!")
    print("=" * 60)


if __name__ == "__main__":
    main()