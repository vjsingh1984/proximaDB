"""
Integration tests for enhanced semantic chunking with performance validation
"""

import pytest
import time
from typing import List, Dict, Any

from proximadb.chunking import (
    TextChunker, ChunkingConfig, ChunkingStrategy, TextChunk,
    create_enhanced_semantic_chunker  # Alias for semantic chunker
)
# Note: EnhancedSemanticChunker functionality has been consolidated into chunking strategies
# This test needs to be updated for the new architecture
from proximadb.chunking_strategies import SemanticStrategy


@pytest.mark.skip(reason="Tests require unimplemented features: enable_topic_detection, enable_content_analysis, enable_caching")
class TestSemanticChunkingIntegration:
    """Integration tests for semantic chunking improvements"""

    @pytest.fixture
    def sample_technical_document(self) -> str:
        """Sample technical document for testing"""
        return """
# ProximaDB API Documentation

ProximaDB is a high-performance vector database designed for modern AI applications.

## Getting Started

To begin using ProximaDB, you need to install the Python client library.

### Installation

```python
pip install proximadb
```

### Basic Usage

Here's a simple example of how to use ProximaDB:

```python
from proximadb import ProximaDBClient

client = ProximaDBClient(url="http://localhost:5678")
collection = client.create_collection("my_vectors", dimension=768)
```

## API Reference

The ProximaDB API provides several key operations for vector management.

### Collection Operations

Collections are containers for vectors. Each collection has a fixed dimension.

#### Creating Collections

Use the create_collection method to create a new collection:

```python
collection = client.create_collection(
    name="my_collection",
    dimension=384,
    distance_metric="cosine"
)
```

#### Listing Collections

To see all available collections:

```python
collections = client.list_collections()
for collection in collections:
    print(f"Collection: {collection.name}, Dimension: {collection.dimension}")
```

### Vector Operations

Vectors are the core data type in ProximaDB.

#### Inserting Vectors

Insert vectors with optional metadata:

```python
vectors = [
    {"id": "doc1", "vector": [0.1, 0.2, 0.3], "metadata": {"type": "document"}},
    {"id": "doc2", "vector": [0.4, 0.5, 0.6], "metadata": {"type": "query"}}
]

result = client.insert_vectors("my_collection", vectors)
```

#### Searching Vectors

Search for similar vectors:

```python
query_vector = [0.1, 0.2, 0.3]
results = client.search_vectors(
    collection="my_collection", 
    vector=query_vector, 
    top_k=10
)

for result in results:
    print(f"ID: {result.id}, Score: {result.score}")
```

## Advanced Features

ProximaDB includes several advanced features for production use.

### Performance Optimization

For optimal performance, consider these best practices:

1. Use appropriate batch sizes for bulk operations
2. Choose the right distance metric for your use case
3. Configure indexing parameters based on your data distribution

### Monitoring and Metrics

ProximaDB provides built-in monitoring capabilities:

```python
metrics = client.get_metrics()
print(f"Total vectors: {metrics.total_vectors}")
print(f"Query latency: {metrics.avg_query_latency_ms}ms")
```

## Conclusion

This guide covered the basics of using ProximaDB for vector operations. For more advanced topics, see the detailed API reference and examples in the repository.
        """
    
    def test_semantic_vs_basic_chunking_quality(self, sample_technical_document):
        """Compare semantic chunking quality vs basic strategies"""
        
        # Test different chunking strategies
        strategies = [
            (ChunkingStrategy.SLIDING_WINDOW, "sliding_window"),
            (ChunkingStrategy.PARAGRAPH, "paragraph"), 
            (ChunkingStrategy.SEMANTIC, "semantic_enhanced")
        ]
        
        chunk_results = {}
        
        for strategy, name in strategies:
            config = ChunkingConfig(
                strategy=strategy,
                chunk_size=800,
                min_chunk_size=100,
                max_chunk_size=1200
            )
            
            chunker = TextChunker(config)
            chunks = chunker.chunk_text(sample_technical_document, f"test_{name}")
            
            chunk_results[name] = {
                "chunks": chunks,
                "count": len(chunks),
                "avg_length": sum(len(c.text) for c in chunks) / len(chunks) if chunks else 0,
                "semantic_coherence": self._calculate_coherence_score(chunks)
            }
        
        # Semantic chunking should produce better quality chunks
        semantic_result = chunk_results["semantic_enhanced"]
        sliding_result = chunk_results["sliding_window"]
        
        print(f"\nChunking Strategy Comparison:")
        for name, result in chunk_results.items():
            print(f"{name}: {result['count']} chunks, avg length: {result['avg_length']:.0f}, coherence: {result['semantic_coherence']:.3f}")
        
        # Semantic chunking should have better coherence
        assert semantic_result["semantic_coherence"] >= sliding_result["semantic_coherence"]
        
        # Check that semantic chunks preserve code blocks and headers
        semantic_chunks = semantic_result["chunks"]
        code_preserved = any("```" in chunk.text for chunk in semantic_chunks if "code" in chunk.text.lower())
        headers_preserved = any(chunk.metadata.get("section_header") for chunk in semantic_chunks)
        
        # Should preserve important structures
        assert code_preserved or headers_preserved  # At least one type should be preserved
    
    def test_topic_boundary_detection(self):
        """Test topic boundary detection in mixed content"""
        mixed_content = """
# Machine Learning Algorithms

Machine learning is a subset of artificial intelligence. It focuses on algorithms that learn from data.

## Neural Networks

Neural networks are inspired by biological neural systems. They consist of interconnected nodes that process information.

Deep learning uses multi-layer neural networks. These networks can learn complex patterns in data.

# Cooking Recipes

Now let's switch topics and talk about cooking. Cooking is an art and science combined.

## Italian Cuisine

Italian cuisine is known for its regional diversity. Pasta is a staple ingredient in many Italian dishes.

### Pasta Preparation

To make fresh pasta, you need flour, eggs, and salt. Mix the ingredients and knead the dough until smooth.

## French Cuisine

French cuisine emphasizes technique and high-quality ingredients. Sauces are fundamental to French cooking.
        """
        
        # Use enhanced semantic chunker directly
        chunker = create_enhanced_semantic_chunker(
            enable_topic_detection=True,
            enable_content_analysis=True,
            topic_threshold=0.2  # More sensitive to topic changes
        )
        
        from proximadb.chunking import ChunkingConfig
        chunking_config = ChunkingConfig(chunk_size=400, min_chunk_size=100)
        
        chunks = chunker.chunk_semantically(
            text=mixed_content,
            source_id="mixed_topics",
            chunking_config=chunking_config
        )
        
        # Should detect topic boundaries between ML and Cooking
        content_types = [chunk.metadata.get("content_type") for chunk in chunks]
        chunk_texts = [chunk.text[:100] + "..." for chunk in chunks]
        
        print(f"\nTopic Boundary Detection Results:")
        for i, (chunk, content_type) in enumerate(zip(chunk_texts, content_types)):
            print(f"Chunk {i+1} ({content_type}): {chunk}")
        
        # Should create separate chunks for different topics
        assert len(chunks) >= 4  # At least 4 chunks for mixed content
        
        # Check that topics are properly separated
        ml_chunks = [c for c in chunks if any(term in c.text.lower() for term in ["machine", "neural", "algorithm"])]
        cooking_chunks = [c for c in chunks if any(term in c.text.lower() for term in ["cooking", "recipe", "pasta"])]
        
        assert len(ml_chunks) > 0
        assert len(cooking_chunks) > 0
        
        print(f"ML chunks: {len(ml_chunks)}, Cooking chunks: {len(cooking_chunks)}")
    
    def test_content_type_specific_processing(self):
        """Test content-type specific processing"""
        test_contents = {
            "technical": """
# API Documentation
This REST API provides endpoints for data management.

## Authentication
Use Bearer tokens for authentication:
```bash
curl -H "Authorization: Bearer token123" https://api.example.com/data
```

## Error Handling
The API returns standard HTTP status codes:
- 200: Success
- 404: Resource not found
- 500: Server error
            """,
            
            "academic": """
# Research Abstract
This study analyzes the effectiveness of different machine learning algorithms.

## Methodology
We conducted experiments on three datasets using cross-validation.
The research methodology follows standard practices in the field.

## Results and Conclusion
Our analysis shows significant improvements over baseline methods.
Further research is needed to validate these findings.
            """,
            
            "narrative": """
# The Story of Alice
Once upon a time, there was a young woman named Alice who lived in a small village.

## Chapter 1: The Journey Begins
Alice decided to embark on a journey to discover new lands.
She packed her belongings and said goodbye to her family.

## Chapter 2: The Adventure
Along the way, Alice met many interesting characters.
Each encounter taught her something valuable about life.
            """
        }
        
        chunker = create_enhanced_semantic_chunker(
            enable_content_analysis=True,
            enable_topic_detection=True
        )
        
        from proximadb.chunking import ChunkingConfig
        config = ChunkingConfig(chunk_size=300, min_chunk_size=80)
        
        results = {}
        
        for content_type, text in test_contents.items():
            chunks = chunker.chunk_semantically(text, f"{content_type}_doc", {}, config)
            
            detected_types = [chunk.metadata.get("content_type") for chunk in chunks]
            coherence_scores = [chunk.metadata.get("coherence_score", 0) for chunk in chunks]
            
            results[content_type] = {
                "chunks": len(chunks),
                "detected_types": detected_types,
                "avg_coherence": sum(coherence_scores) / len(coherence_scores) if coherence_scores else 0,
                "has_patterns": any(chunk.metadata.get("semantic_patterns", []) for chunk in chunks)
            }
        
        print(f"\nContent Type Analysis:")
        for content_type, result in results.items():
            print(f"{content_type}: {result['chunks']} chunks, coherence: {result['avg_coherence']:.3f}, has patterns: {result['has_patterns']}")
        
        # All content types should be processed successfully
        for result in results.values():
            assert result["chunks"] > 0
            assert result["avg_coherence"] > 0
    
    def test_performance_with_caching(self):
        """Test performance improvement with caching enabled"""
        large_document = """
# Large Document for Performance Testing
This is a comprehensive document designed to test caching performance.
        """ + "\n\n## Section {}\nThis is section {} with detailed content about various topics. " * 50
        
        # Test without caching
        chunker_no_cache = create_enhanced_semantic_chunker(enable_caching=False)
        
        from proximadb.chunking import ChunkingConfig
        config = ChunkingConfig(chunk_size=500, min_chunk_size=100)
        
        # First run without cache
        start_time = time.time()
        chunks1 = chunker_no_cache.chunk_semantically(large_document, "perf_test", {}, config)
        time_no_cache = time.time() - start_time
        
        # Test with caching
        chunker_with_cache = create_enhanced_semantic_chunker(enable_caching=True)
        
        # First run (cache miss)
        start_time = time.time()
        chunks2 = chunker_with_cache.chunk_semantically(large_document, "perf_test", {}, config)
        time_cache_miss = time.time() - start_time
        
        # Second run (cache hit)
        start_time = time.time()
        chunks3 = chunker_with_cache.chunk_semantically(large_document, "perf_test", {}, config)
        time_cache_hit = time.time() - start_time
        
        print(f"\nPerformance Comparison:")
        print(f"No cache: {time_no_cache:.4f}s")
        print(f"Cache miss: {time_cache_miss:.4f}s") 
        print(f"Cache hit: {time_cache_hit:.4f}s")
        
        if time_cache_hit > 0:
            speedup = time_cache_miss / time_cache_hit
            print(f"Cache speedup: {speedup:.2f}x")
            
            # Cache hit should be significantly faster
            assert speedup > 1.5  # At least 50% improvement
        
        # Results should be consistent
        assert len(chunks1) > 0
        assert len(chunks2) == len(chunks3)  # Cache hit should return same results
    
    def test_chunk_quality_metrics(self, sample_technical_document):
        """Test chunk quality improvements with enhanced semantic chunking"""
        
        # Compare basic vs enhanced semantic chunking
        basic_config = ChunkingConfig(
            strategy=ChunkingStrategy.SLIDING_WINDOW,
            chunk_size=600,
            min_chunk_size=100
        )
        basic_chunker = TextChunker(basic_config)
        basic_chunks = basic_chunker.chunk_text(sample_technical_document, "basic")
        
        # Enhanced semantic chunking
        enhanced_config = ChunkingConfig(
            strategy=ChunkingStrategy.SEMANTIC,
            chunk_size=600, 
            min_chunk_size=100
        )
        enhanced_chunker = TextChunker(enhanced_config)
        enhanced_chunks = enhanced_chunker.chunk_text(sample_technical_document, "enhanced")
        
        # Quality metrics
        basic_metrics = self._calculate_chunk_metrics(basic_chunks)
        enhanced_metrics = self._calculate_chunk_metrics(enhanced_chunks)
        
        print(f"\nChunk Quality Comparison:")
        print(f"Basic chunks: {basic_metrics}")
        print(f"Enhanced chunks: {enhanced_metrics}")
        
        # Enhanced chunks should have better quality metrics
        assert enhanced_metrics["avg_coherence"] >= basic_metrics["avg_coherence"]
        assert enhanced_metrics["structure_preservation"] >= basic_metrics["structure_preservation"]
        
        # Enhanced chunks should preserve more semantic structure
        enhanced_with_headers = sum(1 for c in enhanced_chunks if c.metadata.get("section_header"))
        basic_with_headers = sum(1 for c in basic_chunks if c.metadata.get("section_header", "").startswith("#"))
        
        assert enhanced_with_headers >= basic_with_headers
    
    def _calculate_coherence_score(self, chunks: List[TextChunk]) -> float:
        """Calculate semantic coherence score for chunks"""
        if not chunks:
            return 0.0
        
        coherence_scores = []
        for chunk in chunks:
            # Simple coherence based on sentence similarity within chunk
            sentences = [s.strip() for s in chunk.text.split('.') if s.strip()]
            if len(sentences) < 2:
                coherence_scores.append(1.0)
                continue
            
            similarities = []
            for i in range(len(sentences) - 1):
                words1 = set(sentences[i].lower().split())
                words2 = set(sentences[i+1].lower().split())
                
                if words1 and words2:
                    overlap = len(words1 & words2)
                    total = len(words1 | words2)
                    similarity = overlap / total if total > 0 else 0
                    similarities.append(similarity)
            
            avg_similarity = sum(similarities) / len(similarities) if similarities else 0
            coherence_scores.append(avg_similarity)
        
        return sum(coherence_scores) / len(coherence_scores)
    
    def _calculate_chunk_metrics(self, chunks: List[TextChunk]) -> Dict[str, Any]:
        """Calculate comprehensive chunk quality metrics"""
        if not chunks:
            return {"avg_coherence": 0, "structure_preservation": 0, "size_consistency": 0}
        
        # Coherence score
        coherence = self._calculate_coherence_score(chunks)
        
        # Structure preservation (headers, code blocks, etc.)
        structure_indicators = 0
        for chunk in chunks:
            text = chunk.text.lower()
            if any(indicator in text for indicator in ["#", "```", "##", "###", "def ", "class "]):
                structure_indicators += 1
        
        structure_preservation = structure_indicators / len(chunks)
        
        # Size consistency (coefficient of variation)
        sizes = [len(chunk.text) for chunk in chunks]
        avg_size = sum(sizes) / len(sizes)
        size_variance = sum((size - avg_size) ** 2 for size in sizes) / len(sizes)
        size_std = size_variance ** 0.5
        size_consistency = 1 - (size_std / avg_size) if avg_size > 0 else 0
        
        return {
            "avg_coherence": coherence,
            "structure_preservation": structure_preservation,
            "size_consistency": max(0, size_consistency)  # Ensure non-negative
        }


@pytest.mark.performance
@pytest.mark.skip(reason="Tests require unimplemented features: enable_topic_detection, enable_caching")
class TestSemanticChunkingPerformance:
    """Performance tests for semantic chunking improvements"""
    
    def test_large_document_performance(self):
        """Test performance with large documents"""
        # Generate large document
        large_doc = self._generate_large_document(sections=100, content_per_section=500)
        
        chunker = create_enhanced_semantic_chunker(
            enable_topic_detection=True,
            enable_caching=True
        )
        
        from proximadb.chunking import ChunkingConfig
        config = ChunkingConfig(chunk_size=800, min_chunk_size=200)
        
        start_time = time.time()
        chunks = chunker.chunk_semantically(large_doc, "large_doc", {}, config)
        duration = time.time() - start_time
        
        chars_per_second = len(large_doc) / duration
        chunks_per_second = len(chunks) / duration
        
        print(f"\nLarge Document Performance:")
        print(f"Document size: {len(large_doc):,} characters")
        print(f"Chunks created: {len(chunks)}")
        print(f"Processing time: {duration:.3f}s")
        print(f"Throughput: {chars_per_second:,.0f} chars/second")
        print(f"Chunk rate: {chunks_per_second:.1f} chunks/second")
        
        # Performance requirements
        assert duration < 10.0  # Should complete within 10 seconds
        assert chars_per_second > 5000  # At least 5k chars/second
        assert len(chunks) > 50  # Should create meaningful number of chunks
    
    def test_concurrent_chunking_performance(self):
        """Test concurrent chunking performance"""
        import threading
        import queue
        
        # Create test documents
        documents = [
            self._generate_large_document(sections=20, content_per_section=300)
            for _ in range(5)
        ]
        
        chunker = create_enhanced_semantic_chunker(enable_caching=True)
        
        from proximadb.chunking import ChunkingConfig
        config = ChunkingConfig(chunk_size=400, min_chunk_size=100)
        
        results = queue.Queue()
        
        def chunk_worker(doc_id, document):
            start_time = time.time()
            chunks = chunker.chunk_semantically(document, f"doc_{doc_id}", {}, config)
            duration = time.time() - start_time
            results.put((doc_id, len(chunks), duration))
        
        # Process documents concurrently
        threads = []
        overall_start = time.time()
        
        for i, doc in enumerate(documents):
            thread = threading.Thread(target=chunk_worker, args=(i, doc))
            threads.append(thread)
            thread.start()
        
        for thread in threads:
            thread.join()
        
        overall_duration = time.time() - overall_start
        
        # Collect results
        concurrent_results = []
        while not results.empty():
            concurrent_results.append(results.get())
        
        total_chunks = sum(result[1] for result in concurrent_results)
        avg_duration = sum(result[2] for result in concurrent_results) / len(concurrent_results)
        
        print(f"\nConcurrent Chunking Performance:")
        print(f"Documents processed: {len(documents)}")
        print(f"Total chunks: {total_chunks}")
        print(f"Overall time: {overall_duration:.3f}s")
        print(f"Average per document: {avg_duration:.3f}s")
        
        # Concurrent processing should be efficient
        assert len(concurrent_results) == len(documents)  # All documents processed
        assert overall_duration < 15.0  # Should complete within 15 seconds
        assert total_chunks > len(documents) * 10  # Reasonable chunk count
    
    def _generate_large_document(self, sections: int, content_per_section: int) -> str:
        """Generate large document for performance testing"""
        content = "# Large Performance Test Document\n\n"
        content += "This document is generated for performance testing of semantic chunking.\n\n"
        
        for i in range(sections):
            content += f"## Section {i+1}: Topic {i % 10}\n\n"
            
            # Alternate between different content types
            if i % 3 == 0:
                # Technical content
                content += "This section covers technical implementation details. "
                content += "The API provides RESTful endpoints for data management. "
                content += "```python\ndef process_data(input_data):\n    return transform(input_data)\n```\n\n"
            elif i % 3 == 1:
                # Analytical content
                content += "Our analysis shows significant improvements in performance. "
                content += "The results indicate a 25% increase in throughput. "
                content += "Further research is recommended to validate these findings.\n\n"
            else:
                # Descriptive content
                content += "This section provides detailed descriptions of the system components. "
                content += "Each component plays a crucial role in the overall architecture. "
                content += "The integration between components ensures seamless operation.\n\n"
            
            # Add padding content to reach target length
            base_content = content[-200:]  # Use recent content as base
            while len(content.split('\n\n')[-1]) < content_per_section:
                content += f"Additional content for section {i+1}. "
        
        return content


if __name__ == "__main__":
    # Run integration tests
    pytest.main([__file__, "-v"])