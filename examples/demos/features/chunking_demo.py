#!/usr/bin/env python3
"""
ProximaDB Text Chunking Strategies Demo

This script demonstrates various text chunking strategies available in ProximaDB:
- Sentence-based chunking
- Paragraph-based chunking
- Sliding window chunking
- Semantic chunking
- Fixed-size chunking
- Recursive chunking
"""

import time
import logging
import numpy as np
import sys
import os
from pathlib import Path
from typing import List, Dict, Any
import json

# Import ProximaDB SDK (requires PYTHONPATH to include clients/python/src)
from proximadb import (
    connect_rest, CollectionConfig, DistanceMetric,
    TextChunker, ChunkingStrategy, ChunkingConfig, TextChunk,
    chunk_by_sentences, chunk_by_paragraphs, chunk_sliding_window
)

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

# Sample document for chunking demonstration
SAMPLE_DOCUMENT = """
# Introduction to Vector Databases

Vector databases are specialized data management systems designed to store, index, and query high-dimensional vector embeddings. These embeddings represent complex data like text, images, or audio in a mathematical format that captures semantic meaning.

## Why Vector Databases Matter

Traditional databases excel at exact matches and structured queries. However, they struggle with similarity search and semantic understanding. Vector databases fill this gap by enabling:

1. Semantic Search: Find documents based on meaning, not just keywords.
2. Recommendation Systems: Identify similar items based on user preferences.
3. Anomaly Detection: Spot outliers in high-dimensional data.
4. Natural Language Processing: Power chatbots and question-answering systems.

### Key Components

A vector database typically consists of several key components that work together to provide efficient similarity search:

**Vector Storage**: The core component stores high-dimensional vectors, usually as arrays of floating-point numbers. Modern systems support vectors with hundreds or thousands of dimensions.

**Indexing Algorithms**: To enable fast search, vector databases use specialized indexing structures like:
- HNSW (Hierarchical Navigable Small World) graphs
- IVF (Inverted File) indices
- LSH (Locality Sensitive Hashing)
- Annoy (Approximate Nearest Neighbors)

**Distance Metrics**: Similarity is measured using various distance functions:
- Cosine similarity for directional similarity
- Euclidean distance for spatial proximity
- Manhattan distance for grid-based calculations
- Dot product for recommendation systems

## Real-World Applications

Vector databases power many modern AI applications. In e-commerce, they enable visual search where users can find products similar to a photo. Content platforms use them for recommendation engines that suggest articles, videos, or music based on user behavior.

Financial institutions leverage vector databases for fraud detection by identifying unusual transaction patterns. Healthcare organizations use them to find similar patient cases or match symptoms to diagnoses.

### Performance Considerations

When deploying vector databases at scale, several factors impact performance:

**Memory Usage**: Vector data can consume significant memory. A million 768-dimensional vectors require approximately 3GB of RAM just for raw storage.

**Query Latency**: Well-optimized systems can search millions of vectors in milliseconds. However, factors like index type, hardware, and query complexity affect speed.

**Accuracy vs Speed**: Most vector search is approximate. You can trade accuracy for speed by adjusting parameters like the number of probes or graph connections.

## Future Directions

The field of vector databases is rapidly evolving. Emerging trends include:

- Multi-modal embeddings that combine text, image, and audio
- Hybrid search combining vector similarity with traditional filters
- Distributed architectures for web-scale deployments
- Hardware acceleration using GPUs and specialized chips

As AI models become more sophisticated, vector databases will play an increasingly crucial role in making these models practical for real-world applications.
"""


class ChunkingDemo:
    """Demonstrates various text chunking strategies with ProximaDB"""
    
    def __init__(self, server_url="http://localhost:5678"):
        self.server_url = server_url
        self.client = None
        self.collection_name = f"chunking_demo_{int(time.time())}"
        self.chunks_data = {}
        
    def setup(self):
        """Connect to ProximaDB and create collection"""
        print("🔗 Setting up ProximaDB connection...")
        
        try:
            self.client = connect_rest(self.server_url)
            logger.info("✅ Connected to ProximaDB")
            
            # Create collection for storing chunks
            config = CollectionConfig(
                dimension=384,  # Using sentence-transformers dimension
                distance_metric=DistanceMetric.COSINE,
                description="Text chunking strategies demonstration"
            )
            
            collection = self.client.create_collection(self.collection_name, config)
            logger.info(f"✅ Created collection: {collection.name}")
            
            return True
        except Exception as e:
            logger.error(f"❌ Setup failed: {e}")
            return False
    
    def demonstrate_sentence_chunking(self):
        """Demonstrate sentence-based chunking"""
        print("\n📝 1. Sentence-Based Chunking")
        print("-" * 50)
        
        chunks = chunk_by_sentences(
            SAMPLE_DOCUMENT,
            chunk_size=512,
            document_id="doc_sentences",
            metadata={"source": "demo", "type": "documentation"}
        )
        
        print(f"✅ Created {len(chunks)} sentence-based chunks")
        print(f"📊 Average chunk size: {np.mean([chunk.length for chunk in chunks]):.0f} chars")
        print(f"📏 Chunk size range: {min(chunk.length for chunk in chunks)} - {max(chunk.length for chunk in chunks)} chars")
        
        # Show first few chunks
        print("\n🔍 Sample chunks:")
        for i, chunk in enumerate(chunks[:3]):
            print(f"\nChunk {i+1} ({chunk.length} chars):")
            print(f"Text: {chunk.text[:100]}...")
            print(f"Metadata: {chunk.metadata}")
        
        self.chunks_data["sentence"] = chunks
        return chunks
    
    def demonstrate_paragraph_chunking(self):
        """Demonstrate paragraph-based chunking"""
        print("\n📄 2. Paragraph-Based Chunking")
        print("-" * 50)
        
        chunks = chunk_by_paragraphs(
            SAMPLE_DOCUMENT,
            max_size=1024,
            document_id="doc_paragraphs",
            metadata={"source": "demo", "type": "documentation"}
        )
        
        print(f"✅ Created {len(chunks)} paragraph-based chunks")
        print(f"📊 Average chunk size: {np.mean([chunk.length for chunk in chunks]):.0f} chars")
        print(f"📏 Chunk size range: {min(chunk.length for chunk in chunks)} - {max(chunk.length for chunk in chunks)} chars")
        
        # Show structure
        print("\n🔍 Paragraph topics:")
        for i, chunk in enumerate(chunks[:5]):
            # Extract first line as topic
            first_line = chunk.text.split('\n')[0][:80]
            print(f"  {i+1}. {first_line}...")
        
        self.chunks_data["paragraph"] = chunks
        return chunks
    
    def demonstrate_sliding_window_chunking(self):
        """Demonstrate sliding window chunking"""
        print("\n🔄 3. Sliding Window Chunking")
        print("-" * 50)
        
        chunks = chunk_sliding_window(
            SAMPLE_DOCUMENT,
            window_size=400,
            overlap=100,
            document_id="doc_sliding",
            metadata={"source": "demo", "type": "documentation"}
        )
        
        print(f"✅ Created {len(chunks)} sliding window chunks")
        print(f"📊 Window size: 400 chars with 100 char overlap")
        print(f"📏 Coverage: {(len(chunks) - 1) * 300 + 400} chars processed")
        
        # Show overlap example
        if len(chunks) >= 2:
            print("\n🔍 Overlap demonstration (chunks 1 & 2):")
            print(f"Chunk 1 end: ...{chunks[0].text[-50:]}")
            print(f"Chunk 2 start: {chunks[1].text[:50]}...")
        
        self.chunks_data["sliding_window"] = chunks
        return chunks
    
    def demonstrate_semantic_chunking(self):
        """Demonstrate semantic/topic-based chunking"""
        print("\n🧠 4. Semantic Chunking")
        print("-" * 50)
        
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SEMANTIC,
            max_chunk_size=2048,
            preserve_sentences=True
        )
        chunker = TextChunker(config)
        
        chunks = chunker.chunk_text(
            SAMPLE_DOCUMENT,
            document_id="doc_semantic",
            metadata={"source": "demo", "type": "documentation"}
        )
        
        print(f"✅ Created {len(chunks)} semantic chunks")
        print(f"📊 Chunks based on document structure and topics")
        
        # Show semantic sections
        print("\n🔍 Detected sections:")
        for i, chunk in enumerate(chunks):
            if "section_header" in chunk.metadata:
                print(f"  {i+1}. {chunk.metadata['section_header']}")
        
        self.chunks_data["semantic"] = chunks
        return chunks
    
    def demonstrate_fixed_size_chunking(self):
        """Demonstrate fixed-size chunking"""
        print("\n📐 5. Fixed-Size Chunking")
        print("-" * 50)
        
        config = ChunkingConfig(
            strategy=ChunkingStrategy.FIXED_SIZE,
            chunk_size=300,
            min_chunk_size=50
        )
        chunker = TextChunker(config)
        
        chunks = chunker.chunk_text(
            SAMPLE_DOCUMENT,
            document_id="doc_fixed",
            metadata={"source": "demo", "type": "documentation"}
        )
        
        print(f"✅ Created {len(chunks)} fixed-size chunks")
        print(f"📊 Each chunk: exactly 300 chars (except last)")
        print(f"📏 Chunk sizes: {[chunk.length for chunk in chunks[:5]]}...")
        
        self.chunks_data["fixed_size"] = chunks
        return chunks
    
    def demonstrate_recursive_chunking(self):
        """Demonstrate recursive chunking"""
        print("\n🔁 6. Recursive Chunking")
        print("-" * 50)
        
        config = ChunkingConfig(
            strategy=ChunkingStrategy.RECURSIVE,
            chunk_size=500,
            min_chunk_size=100
        )
        chunker = TextChunker(config)
        
        chunks = chunker.chunk_text(
            SAMPLE_DOCUMENT,
            document_id="doc_recursive",
            metadata={"source": "demo", "type": "documentation"}
        )
        
        print(f"✅ Created {len(chunks)} recursive chunks")
        print(f"📊 Hierarchical splitting with multiple separators")
        print(f"📏 Preserves document structure while maintaining size limits")
        
        self.chunks_data["recursive"] = chunks
        return chunks
    
    def compare_strategies(self):
        """Compare different chunking strategies"""
        print("\n📊 Strategy Comparison")
        print("-" * 50)
        
        comparison = []
        for strategy, chunks in self.chunks_data.items():
            stats = {
                "strategy": strategy,
                "num_chunks": len(chunks),
                "avg_size": np.mean([c.length for c in chunks]),
                "min_size": min(c.length for c in chunks),
                "max_size": max(c.length for c in chunks),
                "total_chars": sum(c.length for c in chunks)
            }
            comparison.append(stats)
        
        # Print comparison table
        print(f"{'Strategy':<20} {'Chunks':<10} {'Avg Size':<12} {'Min':<8} {'Max':<8}")
        print("-" * 60)
        for stats in comparison:
            print(f"{stats['strategy']:<20} {stats['num_chunks']:<10} "
                  f"{stats['avg_size']:<12.0f} {stats['min_size']:<8} {stats['max_size']:<8}")
    
    def demonstrate_chunk_vectorization(self):
        """Demonstrate storing chunks as vectors in ProximaDB"""
        print("\n🚀 Storing Chunks as Vectors")
        print("-" * 50)
        
        # Use sliding window chunks for this demo
        chunks = self.chunks_data.get("sliding_window", [])[:20]  # First 20 chunks
        
        if not chunks:
            print("❌ No chunks available")
            return
        
        # Generate mock embeddings (in production, use real embedding model)
        vectors = []
        ids = []
        metadata_list = []
        
        for i, chunk in enumerate(chunks):
            # Generate a mock embedding based on chunk content
            # In production, use sentence-transformers or similar
            vector = np.random.randn(384).astype(np.float32)
            vector = vector / np.linalg.norm(vector)  # Normalize
            
            vectors.append(vector.tolist())
            ids.append(f"chunk_{i}")
            
            # Prepare metadata
            chunk_metadata = {
                "text": chunk.text[:200],  # Store first 200 chars
                "chunk_id": chunk.chunk_id,
                "chunk_type": chunk.metadata.get("chunk_type", "unknown"),
                "position": f"{chunk.start_pos}-{chunk.end_pos}",
                "length": chunk.length
            }
            metadata_list.append(chunk_metadata)
        
        # Insert chunks into ProximaDB
        try:
            start_time = time.time()
            result = self.client.insert_vectors(
                self.collection_name,
                vectors,
                ids,
                metadata=metadata_list
            )
            duration = time.time() - start_time
            
            logger.info(f"✅ Stored {result.successful_count} chunk vectors in {duration:.2f}s")
            
            # Demonstrate search
            print("\n🔍 Searching for similar chunks...")
            query_vector = vectors[5]  # Use chunk 5 as query
            
            search_results = self.client.search(
                self.collection_name,
                query_vector,
                k=5
            )
            
            print(f"✅ Found {len(search_results)} similar chunks:")
            for i, result in enumerate(search_results):
                metadata = getattr(result, 'metadata', {})
                print(f"\n{i+1}. Chunk: {result.id} (Score: {result.score:.3f})")
                print(f"   Text preview: {metadata.get('text', 'N/A')[:80]}...")
                print(f"   Position: {metadata.get('position', 'N/A')}")
            
        except Exception as e:
            logger.error(f"❌ Failed to store/search chunks: {e}")
    
    def demonstrate_context_addition(self):
        """Demonstrate adding context to chunks"""
        print("\n🔗 Adding Context to Chunks")
        print("-" * 50)
        
        # Create chunker with context
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SLIDING_WINDOW,
            chunk_size=300,
            chunk_overlap=50,
            add_context=True,
            context_size=50
        )
        chunker = TextChunker(config)
        
        chunks = chunker.chunk_text(SAMPLE_DOCUMENT, "doc_context")
        chunks = chunker.add_context_to_chunks(chunks)
        
        # Show chunks with context
        print("✅ Added surrounding context to chunks")
        print("\n🔍 Example chunk with context:")
        
        if len(chunks) > 2:
            chunk = chunks[2]
            print(f"\nMain chunk: {chunk.text[:100]}...")
            
            if "prev_context" in chunk.metadata:
                print(f"\nPrevious context: ...{chunk.metadata['prev_context']}")
            
            if "next_context" in chunk.metadata:
                print(f"\nNext context: {chunk.metadata['next_context']}...")
    
    def cleanup(self):
        """Clean up resources"""
        print(f"\n🧹 Cleaning up...")
        
        try:
            self.client.delete_collection(self.collection_name)
            logger.info(f"✅ Deleted collection '{self.collection_name}'")
        except Exception as e:
            logger.warning(f"⚠️  Cleanup failed: {e}")
    
    def run_full_demo(self):
        """Run the complete chunking demonstration"""
        print("🎭 ProximaDB Text Chunking Strategies Demo")
        print("=" * 60)
        print("This demo showcases various text chunking strategies:")
        print("• Sentence-based chunking")
        print("• Paragraph-based chunking")
        print("• Sliding window chunking")
        print("• Semantic chunking")
        print("• Fixed-size chunking")
        print("• Recursive chunking")
        print("=" * 60)
        
        if not self.setup():
            return False
        
        try:
            # Demonstrate each strategy
            self.demonstrate_sentence_chunking()
            self.demonstrate_paragraph_chunking()
            self.demonstrate_sliding_window_chunking()
            self.demonstrate_semantic_chunking()
            self.demonstrate_fixed_size_chunking()
            self.demonstrate_recursive_chunking()
            
            # Compare strategies
            self.compare_strategies()
            
            # Advanced features
            self.demonstrate_chunk_vectorization()
            self.demonstrate_context_addition()
            
            print("\n✅ Chunking demonstration completed successfully!")
            return True
            
        except Exception as e:
            logger.error(f"❌ Demo failed: {e}")
            return False
        finally:
            self.cleanup()


def main():
    """Main entry point"""
    print("🚀 Starting ProximaDB Chunking Demo...")
    
    demo = ChunkingDemo()
    success = demo.run_full_demo()
    
    print(f"\n{'='*60}")
    if success:
        print("🎊 Chunking strategies demonstration completed!")
        print("✨ All strategies demonstrated successfully!")
    else:
        print("😞 Demo encountered issues")
    
    return success


if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)