"""
Tests for refactored chunking system with separation of concerns

Tests all chunking strategies independently from embeddings and validates
the clean architecture with pluggable strategies.
"""

import pytest
import numpy as np
from pathlib import Path
import sys
from typing import List, Dict, Any

# Add utils to path
sys.path.insert(0, str(Path(__file__).parent.parent))
from utils.base_test import BaseProximaDBTest
from utils.server_utils import ensure_server_running

from proximadb.chunking import (
    TextChunker,
    ChunkingStrategy,
    ChunkingConfig,
    TextChunk,
    create_vector_records,
    chunk_and_embed_text,
)
from proximadb.chunking_strategies import (
    SlidingWindowStrategy,
    SentenceStrategy,
    ParagraphStrategy,
    SemanticStrategy,
    RecursiveStrategy,
    get_chunking_strategy,
)
from proximadb.embedding_interface import get_default_embedding_provider
from proximadb.models import VectorRecord


class TestChunkingStrategies:
    """Test individual chunking strategies"""
    
    @pytest.fixture
    def sample_text(self):
        """Sample text for testing"""
        return """
# Introduction to Machine Learning

Machine learning is a subset of artificial intelligence that focuses on building systems that learn from data. Rather than being explicitly programmed to perform a task, these systems improve their performance through experience.

## Types of Machine Learning

There are three main types of machine learning:

1. Supervised Learning: The algorithm learns from labeled training data. Each training example consists of an input and the desired output. Common algorithms include decision trees, neural networks, and support vector machines.

2. Unsupervised Learning: The algorithm finds patterns in unlabeled data. It discovers hidden structures without human supervision. Examples include clustering algorithms like K-means and dimensionality reduction techniques like PCA.

3. Reinforcement Learning: The algorithm learns by interacting with an environment. It receives rewards or penalties for its actions and learns to maximize cumulative reward. This approach is used in game playing and robotics.

## Applications

Machine learning has numerous applications across various industries:

- Healthcare: Disease diagnosis, drug discovery, personalized treatment plans
- Finance: Fraud detection, risk assessment, algorithmic trading
- Retail: Recommendation systems, demand forecasting, customer segmentation
- Transportation: Autonomous vehicles, route optimization, traffic prediction

## Conclusion

Machine learning continues to evolve rapidly, with new techniques and applications emerging regularly. As data becomes more abundant and computational power increases, the potential for machine learning to transform industries and solve complex problems grows exponentially.
"""
    
    def test_sliding_window_strategy(self, sample_text):
        """Test sliding window chunking"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SLIDING_WINDOW,
            chunk_size=200,
            chunk_overlap=50,
            min_chunk_size=50
        )
        
        strategy = SlidingWindowStrategy(config)
        chunks = strategy.chunk(sample_text, "test_doc", {"source": "test"})
        
        assert len(chunks) > 0
        
        # Verify chunk properties
        for i, chunk in enumerate(chunks):
            assert isinstance(chunk, TextChunk)
            assert len(chunk.text) <= config.chunk_size
            assert len(chunk.text) >= config.min_chunk_size or i == len(chunks) - 1
            assert chunk.metadata["chunk_type"] == "sliding_window"
            assert chunk.metadata["chunking_strategy"] == "sliding_window"
            
            # Check overlap
            if i > 0:
                assert chunk.metadata["has_overlap"] == True
                assert chunk.metadata["overlap_size"] == config.chunk_overlap
    
    def test_sentence_strategy(self, sample_text):
        """Test sentence-based chunking"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SENTENCE,
            chunk_size=300,
            min_chunk_size=100
        )
        
        strategy = SentenceStrategy(config)
        chunks = strategy.chunk(sample_text, "test_doc", {"source": "test"})
        
        assert len(chunks) > 0
        
        for chunk in chunks:
            assert isinstance(chunk, TextChunk)
            assert chunk.metadata["chunk_type"] == "sentence"
            assert "sentence_count" in chunk.metadata
            assert chunk.metadata["sentence_count"] > 0
            
            # Verify sentences are not split mid-sentence
            for ending in config.sentence_endings:
                if ending in chunk.text[:-1]:  # Exclude last character
                    # Should be followed by space and capital letter
                    idx = chunk.text.find(ending)
                    if idx < len(chunk.text) - 2:
                        assert chunk.text[idx + 1] == ' ' or chunk.text[idx + 1] == '\n'
    
    def test_paragraph_strategy(self, sample_text):
        """Test paragraph-based chunking"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.PARAGRAPH,
            chunk_size=500,
            min_chunk_size=100
        )
        
        strategy = ParagraphStrategy(config)
        chunks = strategy.chunk(sample_text, "test_doc", {"source": "test"})
        
        assert len(chunks) > 0
        
        for chunk in chunks:
            assert isinstance(chunk, TextChunk)
            assert chunk.metadata["chunk_type"] == "paragraph"
            assert "paragraph_count" in chunk.metadata
            
            # Check for paragraph preservation
            if chunk.metadata["paragraph_count"] > 1:
                assert "\n\n" in chunk.text or "\n" in chunk.text
    
    def test_semantic_strategy(self, sample_text):
        """Test semantic chunking"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SEMANTIC,
            chunk_size=400,
            min_chunk_size=100,
            preserve_code_blocks=True,
            preserve_tables=True
        )
        
        strategy = SemanticStrategy(config)
        chunks = strategy.chunk(sample_text, "test_doc", {"source": "test"})
        
        assert len(chunks) > 0
        
        # Should detect headers and sections
        section_chunks = [c for c in chunks if c.metadata.get("has_header")]
        assert len(section_chunks) > 0
        
        for chunk in chunks:
            assert chunk.metadata["chunk_type"] in ["semantic", "semantic_split"]
            
            # Check for section metadata
            if chunk.metadata.get("has_header"):
                assert "header_title" in chunk.metadata
                assert "header_level" in chunk.metadata
    
    def test_recursive_strategy(self, sample_text):
        """Test recursive chunking"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.RECURSIVE,
            chunk_size=300,
            max_chunk_size=400,
            min_chunk_size=50
        )
        
        strategy = RecursiveStrategy(config)
        chunks = strategy.chunk(sample_text, "test_doc", {"source": "test"})
        
        assert len(chunks) > 0
        
        for chunk in chunks:
            assert chunk.metadata["chunk_type"] == "recursive"
            assert "recursive_level" in chunk.metadata
            assert "strategy_used" in chunk.metadata
            assert chunk.metadata["strategy_used"] in ["paragraph", "sentence", "sliding_window"]


class TestTextChunker:
    """Test the main TextChunker interface"""
    
    def test_chunker_initialization(self):
        """Test chunker initialization with different configs"""
        # Default initialization
        chunker = TextChunker()
        assert chunker.config.strategy == ChunkingStrategy.SLIDING_WINDOW
        
        # Custom config
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SEMANTIC,
            chunk_size=1000
        )
        chunker = TextChunker(config)
        assert chunker.config.strategy == ChunkingStrategy.SEMANTIC
        assert chunker.config.chunk_size == 1000
    
    def test_chunk_text_all_strategies(self):
        """Test chunking with all available strategies"""
        text = """
        This is a test document with multiple paragraphs.
        
        Each paragraph contains several sentences. These sentences are used to test
        the different chunking strategies. Some strategies preserve sentence boundaries.
        
        Other strategies focus on semantic meaning. They try to keep related content
        together while respecting size constraints.
        """
        
        for strategy in ChunkingStrategy:
            config = ChunkingConfig(strategy=strategy, chunk_size=200)
            chunker = TextChunker(config)
            
            chunks = chunker.chunk_text(text, f"test_{strategy.value}")
            
            assert len(chunks) > 0
            assert all(isinstance(c, TextChunk) for c in chunks)
            assert all(c.metadata["chunking_strategy"] == strategy.value for c in chunks)
    
    def test_add_context_to_chunks(self):
        """Test adding context to chunks"""
        # Use smaller chunk size and longer text with substantial content per chunk
        chunker = TextChunker(ChunkingConfig(chunk_size=100, chunk_overlap=0, min_chunk_size=30))

        # Long text with clear chunks (each sentence is ~50+ chars)
        text = ("This is the first substantial chunk of text with enough content to be meaningful. "
                "This is the second substantial chunk of text with enough content to be meaningful. "
                "This is the third substantial chunk of text with enough content to be meaningful. "
                "This is the fourth substantial chunk of text with enough content to be meaningful.")
        chunks = chunker.chunk_text(text, "test_doc")

        # Ensure we have multiple chunks for meaningful test
        assert len(chunks) > 1, f"Test requires multiple chunks, got {len(chunks)}"

        # Add context
        enhanced_chunks = chunker.add_context_to_chunks(chunks, context_size=10)

        assert len(enhanced_chunks) == len(chunks)

        # Check context metadata
        for i, chunk in enumerate(enhanced_chunks):
            assert chunk.metadata["has_context"] == True

            if i > 0:
                assert "prev_context" in chunk.metadata
                assert len(chunk.metadata["prev_context"]) <= 10

            if i < len(chunks) - 1:
                assert "next_context" in chunk.metadata
                assert len(chunk.metadata["next_context"]) <= 10


@pytest.mark.skip(reason="Tests require embedding provider initialization and server setup - actually integration tests")
class TestVectorRecordCreation(BaseProximaDBTest):
    """Test creating vector records from chunks and embeddings"""
    
    def test_create_vector_records(self):
        """Test creating vector records with proper separation"""
        # Create chunks
        chunks = [
            TextChunk(
                text="First chunk text",
                start_pos=0,
                end_pos=16,
                chunk_id="doc_chunk_0",
                metadata={"chunk_type": "test", "index": 0}
            ),
            TextChunk(
                text="Second chunk text",
                start_pos=17,
                end_pos=34,
                chunk_id="doc_chunk_1",
                metadata={"chunk_type": "test", "index": 1}
            )
        ]
        
        # Create embeddings (simulated)
        embeddings = [
            [0.1] * 384,
            [0.2] * 384
        ]
        
        # Create vector records
        records = create_vector_records(
            chunks,
            embeddings,
            collection_metadata={"source": "test"},
            filterable_fields=["source", "chunk_type", "index"]
        )
        
        assert len(records) == 2
        
        for i, record in enumerate(records):
            assert isinstance(record, VectorRecord)
            assert record.id == chunks[i].chunk_id
            assert len(record.vector) == 384
            assert record.metadata["source"] == "test"
            assert record.metadata["chunk_type"] == "test"
            assert record.metadata["index"] == i
            assert "text_preview" in record.metadata
            assert "embedding_dimension" in record.metadata
    
    def test_chunk_and_embed_integration(self):
        """Test the convenience function with real embeddings"""
        text = """
        ProximaDB is a high-performance vector database designed for AI applications.
        It supports multiple storage engines and indexing algorithms.
        The Python SDK provides easy integration with machine learning workflows.
        """
        
        # Get embedding provider
        provider = get_default_embedding_provider()
        
        # Process text
        records = chunk_and_embed_text(
            text=text,
            source_id="test_doc",
            embedding_provider=provider,
            chunking_config=ChunkingConfig(
                strategy=ChunkingStrategy.SENTENCE,
                chunk_size=200
            ),
            metadata={"doc_type": "technical"},
            filterable_fields=["doc_type", "source_id"]
        )
        
        assert len(records) > 0
        
        for record in records:
            assert isinstance(record, VectorRecord)
            assert len(record.vector) == provider.dimension
            assert record.metadata["doc_type"] == "technical"
            assert "text_preview" in record.metadata
            
            # Non-filterable metadata should be in additional_metadata
            assert "additional_metadata" in record.metadata
            assert isinstance(record.metadata["additional_metadata"], dict)
    
    def test_store_chunks_in_proximadb(self):
        """Test storing chunked and embedded text in ProximaDB"""
        # Create collection
        collection_name = self.create_collection(dimension=384)
        
        # Test document
        document = """
        # Vector Databases
        
        Vector databases are specialized systems for storing and searching high-dimensional vectors.
        They are essential for modern AI applications like semantic search and recommendation systems.
        
        ## Key Features
        
        - Fast similarity search using specialized indexes
        - Support for metadata filtering
        - Horizontal scalability
        - Real-time updates
        
        ## Use Cases
        
        Vector databases power many AI applications including RAG systems, semantic search,
        and recommendation engines.
        """
        
        # Process document
        provider = get_default_embedding_provider()
        records = chunk_and_embed_text(
            text=document,
            source_id="vector_db_guide",
            embedding_provider=provider,
            chunking_config=ChunkingConfig(
                strategy=ChunkingStrategy.SEMANTIC,
                chunk_size=300
            ),
            metadata={"doc_type": "guide", "topic": "vector_databases"},
            filterable_fields=["doc_type", "topic", "source_id"]
        )
        
        # Insert into ProximaDB
        response = self.rest_client.insert_vectors(
            collection_name,
            records=records
        )
        
        assert response.get("success") or response.get("inserted") == len(records)
        
        # Wait and search
        self.wait_for_indexing()
        
        # Search with query
        query_text = "What are vector databases used for?"
        query_embedding = provider.embed_text(query_text)
        
        results = self.rest_client.search_vectors(
            collection_name,
            query_embedding.tolist(),
            top_k=3
        )
        
        self.verify_search_results(results, min(3, len(records)))
        
        # Verify metadata
        for result in results:
            assert result["metadata"]["doc_type"] == "guide"
            assert result["metadata"]["topic"] == "vector_databases"


class TestSeparationOfConcerns:
    """Test that chunking and embedding are properly separated"""
    
    def test_chunking_has_no_embedding_logic(self):
        """Verify chunking strategies don't contain embedding logic"""
        # Check all strategy files for embedding-related imports
        strategies_dir = Path(__file__).parent.parent.parent / "src" / "proximadb" / "chunking_strategies"

        embedding_keywords = ["sentence_transformers", "BERT"]  # Only check actual imports/modules

        for strategy_file in strategies_dir.glob("*.py"):
            if strategy_file.name == "__init__.py":
                continue

            content = strategy_file.read_text()

            # Only check import statements for embedding libraries
            import_lines = [line for line in content.split('\n') if 'import' in line and not line.strip().startswith('#')]
            for line in import_lines:
                for keyword in embedding_keywords:
                    if keyword in line:
                        pytest.fail(f"Found embedding import in {strategy_file.name}: {line.strip()}")
    
    def test_chunk_metadata_is_pure(self):
        """Test that chunk metadata doesn't include embedding information"""
        config = ChunkingConfig(strategy=ChunkingStrategy.SEMANTIC)
        chunker = TextChunker(config)
        
        text = "This is a test. This is another test."
        chunks = chunker.chunk_text(text, "test_doc")
        
        # Check metadata doesn't contain embedding-related fields
        embedding_fields = ["embedding", "vector", "embedding_model", "embedding_dimension"]
        
        for chunk in chunks:
            for field in embedding_fields:
                assert field not in chunk.metadata
    
    def test_strategies_are_pluggable(self):
        """Test that strategies can be easily swapped"""
        # Use longer text with multiple structural elements to differentiate strategies
        text = ("This is the first sentence in a paragraph with substantial content. "
                "This is the second sentence with different words and structure. "
                "This is the third sentence providing even more testing material here.\n\n"
                "Here begins a second paragraph with its own set of sentences. "
                "This second paragraph sentence has different content entirely. "
                "The final sentence in this paragraph wraps up the content nicely.")

        strategies = [
            ChunkingStrategy.SLIDING_WINDOW,
            ChunkingStrategy.SENTENCE,
            ChunkingStrategy.PARAGRAPH,
            ChunkingStrategy.SEMANTIC,
            ChunkingStrategy.RECURSIVE
        ]

        results = {}

        for strategy in strategies:
            chunker = TextChunker(ChunkingConfig(strategy=strategy, chunk_size=80, min_chunk_size=20))
            chunks = chunker.chunk_text(text, "test")
            results[strategy] = chunks

        # Different strategies should produce different results
        chunk_counts = [len(results[s]) for s in strategies]
        assert len(set(chunk_counts)) > 1, f"All strategies produced same number of chunks: {chunk_counts}"


if __name__ == "__main__":
    pytest.main([__file__, "-v"])