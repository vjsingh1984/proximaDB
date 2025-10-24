#!/usr/bin/env python3
"""


STATUS: ⚠️  Requires Future Feature
SDK Version: v1.1+ (requires SFREmbeddingProvider)
Server Version: v0.1.4+
Test Result: SKIP - Advanced embedding providers not yet implemented

ProximaDB Embedding Providers Demonstration

This script demonstrates how to use all available embedding providers
with ProximaDB. It shows examples for each provider category:
1. SFR - Top accuracy (Salesforce Research)
2. BGE - Top retrieval (BAAI General Embedding)
3. E5 - Excellent general purpose (Microsoft)
4. Sentence-Transformers - Wide variety of models

Each example shows:
- Provider initialization
- Query embedding generation
- Document embedding generation
- Integration with ProximaDB

"""

import numpy as np
from typing import List, Dict, Any

# Import all embedding providers
from proximadb.embedding_providers import (
    SFREmbeddingProvider,
    BGEEmbeddingProvider,
    E5EmbeddingProvider,
    SentenceTransformerProvider,
    SimulatedEmbeddingProvider,
    EmbeddingProviderFactory,
    EmbeddingConfig
)


def demo_sfr_provider():
    """
    Demo: SFR Provider - Top MTEB Accuracy (66.4)
    Best for: Maximum accuracy, research, when quality is paramount
    """
    print("\n" + "="*80)
    print("1. SFR PROVIDER - TOP ACCURACY (MTEB 66.4)")
    print("="*80)

    # Initialize with default (SFR-Embedding-2_R)
    config = EmbeddingConfig(
        model_name="Salesforce/SFR-Embedding-2_R",
        dimension=4096,
        batch_size=16,  # Smaller batch due to large dimensions
        normalize=True
    )

    print(f"Model: {config.model_name}")
    print(f"Dimensions: {config.dimension}")
    print(f"Use case: Maximum accuracy retrieval")

    # Note: Actual initialization commented out to avoid downloading large models
    # provider = SFREmbeddingProvider(config)

    # Example usage (simulated):
    print("\nExample usage:")
    print("  query_emb = provider.embed_query('What is machine learning?')")
    print("  doc_embs = provider.embed_documents([{'text': 'ML is...'}])")
    print("  # Returns 4096-dimensional embeddings")


def demo_bge_provider():
    """
    Demo: BGE Provider - Top Retrieval Performance
    Best for: Production retrieval, semantic search
    """
    print("\n" + "="*80)
    print("2. BGE PROVIDER - TOP RETRIEVAL")
    print("="*80)

    # Show different BGE model options
    models = [
        ("BAAI/bge-large-en-v1.5", 1024, "Best quality"),
        ("BAAI/bge-base-en-v1.5", 768, "Balanced"),
        ("BAAI/bge-small-en-v1.5", 384, "Fast"),
        ("BAAI/bge-m3", 1024, "Multilingual (100+ languages)")
    ]

    print("\nAvailable BGE models:")
    for model_name, dims, desc in models:
        print(f"  - {model_name}: {dims} dims - {desc}")

    # Example with large model
    config = EmbeddingConfig(
        model_name="BAAI/bge-large-en-v1.5",
        dimension=1024,
        normalize=True
    )

    print(f"\nSelected: {config.model_name}")
    print("Special feature: Query instruction prefix for better retrieval")

    # Example usage:
    print("\nExample usage:")
    print("  # For queries - automatically adds instruction prefix")
    print("  query_emb = provider.embed_query('machine learning tutorial')")
    print("  ")
    print("  # For documents - no instruction needed")
    print("  doc_embs = provider.embed_documents([{'text': 'This is a tutorial...'}])")


def demo_e5_provider():
    """
    Demo: E5 Provider - Excellent General Purpose
    Best for: General purpose production use
    """
    print("\n" + "="*80)
    print("3. E5 PROVIDER - EXCELLENT GENERAL PURPOSE")
    print("="*80)

    # Show E5 model variants
    models = [
        ("intfloat/e5-large-v2", 1024, "Best quality"),
        ("intfloat/e5-base-v2", 768, "Balanced"),
        ("intfloat/e5-small-v2", 384, "Fast"),
        ("intfloat/multilingual-e5-large", 1024, "Multilingual")
    ]

    print("\nAvailable E5 models:")
    for model_name, dims, desc in models:
        print(f"  - {model_name}: {dims} dims - {desc}")

    config = EmbeddingConfig(
        model_name="intfloat/e5-large-v2",
        dimension=1024,
        normalize=True  # Required for E5
    )

    print(f"\nSelected: {config.model_name}")
    print("Special feature: Requires 'query: ' and 'passage: ' prefixes")

    # Example usage:
    print("\nExample usage:")
    print("  # For queries - automatically adds 'query: ' prefix")
    print("  query_emb = provider.embed_query('python tutorial')")
    print("  ")
    print("  # For passages - automatically adds 'passage: ' prefix")
    print("  passage_embs = provider.embed_passages(['Python is a language...'])")


def demo_sentence_transformer_provider():
    """
    Demo: Sentence-Transformers Provider - Wide Variety
    Best for: Quick start, many model options
    """
    print("\n" + "="*80)
    print("4. SENTENCE-TRANSFORMERS PROVIDER - MOST VERSATILE")
    print("="*80)

    # Popular models
    models = [
        ("all-mpnet-base-v2", 768, "Best quality general purpose"),
        ("all-MiniLM-L6-v2", 384, "Fastest, most popular"),
        ("paraphrase-multilingual-mpnet-base-v2", 768, "Multilingual")
    ]

    print("\nPopular models (100+ available):")
    for model_name, dims, desc in models:
        print(f"  - {model_name}: {dims} dims - {desc}")

    # Example with MiniLM (fastest)
    config = EmbeddingConfig(
        model_name="all-MiniLM-L6-v2",
        dimension=384,
        batch_size=64  # Larger batches for smaller dimensions
    )

    print(f"\nSelected: {config.model_name}")
    print("Best for: Quick prototyping, high throughput")

    # Example usage:
    print("\nExample usage:")
    print("  embeddings = provider.embed_texts(['text1', 'text2', 'text3'])")
    print("  # Returns 384-dimensional embeddings, very fast")


def demo_factory_pattern():
    """
    Demo: Using EmbeddingProviderFactory
    Best for: Dynamic provider selection
    """
    print("\n" + "="*80)
    print("5. FACTORY PATTERN - DYNAMIC PROVIDER SELECTION")
    print("="*80)

    print("\nAvailable provider types:")
    providers = [
        ("sfr", "Salesforce/SFR-Embedding-2_R"),
        ("bge", "BAAI/bge-large-en-v1.5"),
        ("e5", "intfloat/e5-large-v2"),
        ("sentence-transformer", "all-mpnet-base-v2"),
        ("simulated", "test-model")
    ]

    for provider_type, default_model in providers:
        print(f"  - {provider_type}: {default_model}")

    print("\nExample usage:")
    print("  # Create provider using factory")
    print("  config = EmbeddingConfig(")
    print("      model_name='BAAI/bge-large-en-v1.5',")
    print("      dimension=1024")
    print("  )")
    print("  provider = EmbeddingProviderFactory.create_provider('bge', config)")
    print("  ")
    print("  # Or use aliases")
    print("  provider = EmbeddingProviderFactory.create_provider('baai', config)")


def demo_proximadb_integration():
    """
    Demo: Complete ProximaDB Integration
    Shows end-to-end workflow with embeddings
    """
    print("\n" + "="*80)
    print("6. COMPLETE PROXIMADB INTEGRATION")
    print("="*80)

    print("\nEnd-to-end workflow:")
    print("""
# 1. Initialize ProximaDB client
from proximadb import ProximaDB
client = ProximaDB(url="http://localhost:5678")

# 2. Create embedding provider
from proximadb.embedding_providers import BGEEmbeddingProvider, EmbeddingConfig

config = EmbeddingConfig(
    model_name="BAAI/bge-base-en-v1.5",
    dimension=768,
    batch_size=32
)
embedding_provider = BGEEmbeddingProvider(config)

# 3. Create collection (dimension must match embeddings)
collection = client.create_collection(
    name="my_documents",
    dimension=768
)

# 4. Prepare documents
documents = [
    {"text": "AI is transforming technology", "category": "tech"},
    {"text": "Machine learning enables predictions", "category": "tech"},
    {"text": "Deep learning uses neural networks", "category": "tech"}
]

# 5. Generate embeddings for documents
doc_embeddings = embedding_provider.embed_documents(documents)

# 6. Insert into ProximaDB
for i, (doc, embedding) in enumerate(zip(documents, doc_embeddings)):
    collection.insert({
        "id": f"doc_{i}",
        "vector": embedding.tolist(),
        "metadata": doc
    })

# 7. Search with query
query = "what is artificial intelligence?"
query_embedding = embedding_provider.embed_query(query)

results = collection.search(
    query_vector=query_embedding.tolist(),
    top_k=5
)

# 8. Process results
for result in results:
    print(f"Score: {result.score}, Text: {result.metadata['text']}")
    """)


def demo_model_comparison():
    """
    Demo: Model Comparison and Selection Guide
    """
    print("\n" + "="*80)
    print("7. MODEL COMPARISON & SELECTION GUIDE")
    print("="*80)

    print("\nPerformance comparison:")
    print("┌─────────────────────────┬──────────┬────────┬────────────┬───────────────────────┐")
    print("│ Model                   │ Provider │ Dims   │ MTEB Score │ Use Case              │")
    print("├─────────────────────────┼──────────┼────────┼────────────┼───────────────────────┤")
    print("│ SFR-Embedding-2_R       │ SFR      │ 4096   │ 66.4       │ Best accuracy         │")
    print("│ bge-large-en-v1.5       │ BGE      │ 1024   │ 64+        │ Production retrieval  │")
    print("│ e5-large-v2             │ E5       │ 1024   │ 65+        │ General purpose       │")
    print("│ all-mpnet-base-v2       │ ST       │ 768    │ 63+        │ Balanced quality      │")
    print("│ bge-base-en-v1.5        │ BGE      │ 768    │ 63+        │ Production balanced   │")
    print("│ all-MiniLM-L6-v2        │ ST       │ 384    │ 59+        │ High throughput       │")
    print("│ bge-small-en-v1.5       │ BGE      │ 384    │ 62+        │ Latency-sensitive     │")
    print("└─────────────────────────┴──────────┴────────┴────────────┴───────────────────────┘")

    print("\nSelection guide:")
    print("  • Maximum Accuracy       → SFR-Embedding-2_R (4096 dims)")
    print("  • Production Quality     → bge-large-en-v1.5 or e5-large-v2 (1024 dims)")
    print("  • Balanced               → bge-base-en-v1.5 or e5-base-v2 (768 dims)")
    print("  • High Speed             → all-MiniLM-L6-v2 or bge-small-en-v1.5 (384 dims)")
    print("  • Multilingual           → bge-m3 or multilingual-e5-large (1024 dims)")


def demo_simulated_provider():
    """
    Demo: Simulated Provider for Testing
    Best for: Development and testing without downloading models
    """
    print("\n" + "="*80)
    print("8. SIMULATED PROVIDER - TESTING & DEVELOPMENT")
    print("="*80)

    config = EmbeddingConfig(
        model_name="test-model",
        dimension=128,  # Configurable dimension
        normalize=True
    )

    print(f"Dimension: {config.dimension}")
    print("Use case: Fast testing without downloading real models")

    # Actually test simulated provider since it has no dependencies
    provider = SimulatedEmbeddingProvider(config)

    print("\nTesting simulated provider:")

    # Test query embedding
    query_emb = provider.embed_query("test query")
    print(f"  ✓ Query embedding shape: {query_emb.shape}")

    # Test document embeddings
    docs = [
        {"text": "document 1"},
        {"text": "document 2"},
        {"text": "document 3"}
    ]
    doc_embs = provider.embed_documents(docs)
    print(f"  ✓ Document embeddings shape: {doc_embs.shape}")

    # Test batch embeddings
    texts = ["text 1", "text 2", "text 3", "text 4", "text 5"]
    batch_embs = provider.embed_texts(texts)
    print(f"  ✓ Batch embeddings shape: {batch_embs.shape}")

    # Verify dimension
    print(f"  ✓ Dimension: {provider.get_dimension()}")

    # Get model info
    info = provider.get_model_info()
    print(f"  ✓ Model info: {info['model_name']}, method={info['method']}")

    print("\n  All simulated provider tests passed!")


def main():
    """Run all demonstrations"""
    print("\n" + "="*80)
    print(" " * 20 + "PROXIMADB EMBEDDING PROVIDERS DEMO")
    print("="*80)
    print("\nThis demonstration shows all available embedding providers and how to use them.")
    print("Note: Real model downloads are disabled to keep the demo fast.")
    print("      Enable providers by uncommenting initialization code in each demo.")

    # Run all demos
    demo_simulated_provider()  # Test with actual provider
    demo_sfr_provider()
    demo_bge_provider()
    demo_e5_provider()
    demo_sentence_transformer_provider()
    demo_factory_pattern()
    demo_proximadb_integration()
    demo_model_comparison()

    print("\n" + "="*80)
    print("DEMO COMPLETE")
    print("="*80)
    print("\nFor more information, see:")
    print("  • EMBEDDING_PROVIDERS.md - Comprehensive documentation")
    print("  • tests/unit/test_embedding_providers.py - Test suite")
    print("  • https://huggingface.co/spaces/mteb/leaderboard - MTEB leaderboard")
    print("\n")


if __name__ == "__main__":
    try:
        main()
    except (ImportError, ModuleNotFoundError) as e:
        print("=" * 70)
        print("🚧 FUTURE FEATURE - Not Yet Implemented")
        print("=" * 70)
        print(f"\n❌ Error: {e}\n")
        print(f"📋 This example requires: SFREmbeddingProvider and other providers")
        print(f"   Expected in SDK v1.1+\n")
        print(f"💡 Workaround:")
        print(f"   Use bert_utils.py for current embedding capabilities\n")
        print("=" * 70)
        exit(1)
    except Exception as e:
        print(f"❌ Unexpected error: {e}")
        import traceback
        traceback.print_exc()
        exit(1)
