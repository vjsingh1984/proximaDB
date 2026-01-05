"""
Tests for ProximaDB semantic chunking with real embeddings

Uses real BERT embeddings and ProximaDB server to test semantic chunking,
topic boundary detection, and content analysis.
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

# Note: EnhancedSemanticChunker functionality has been consolidated into chunking strategies
# This test needs to be updated for the new architecture
from proximadb_sdk.chunking_strategies import SemanticStrategy
from proximadb_sdk.chunking import ChunkingConfig, ChunkingStrategy
from proximadb_sdk.chunking import (
    TextChunk,
    ChunkingConfig,
    ChunkingStrategy,
    TextChunker,
)
from proximadb_sdk.embedding_interface import (
    create_embedding_provider,
    get_default_embedding_provider,
    BERTEmbeddingProvider,
    SimulatedEmbeddingProvider,
)


@pytest.mark.skip(
    reason="SemanticChunkingConfig not yet implemented in new architecture"
)
class TestSemanticChunkingConfig:
    """Test semantic chunking configuration"""

    def test_default_config(self):
        """Test default configuration values"""
        config = SemanticChunkingConfig()

        assert config.enable_topic_boundary_detection == True
        assert config.enable_content_type_detection == True
        assert config.enable_semantic_coherence == True
        assert config.use_embeddings == True
        assert config.embedding_similarity_threshold == 0.75
        assert config.topic_similarity_threshold == 0.3
        assert config.min_topic_length == 200

    def test_custom_config(self):
        """Test custom configuration with embedding provider"""
        provider = create_embedding_provider("simulated")

        config = SemanticChunkingConfig(
            use_embeddings=True,
            embedding_provider=provider,
            embedding_similarity_threshold=0.8,
            enable_analysis_caching=False,
        )

        assert config.use_embeddings == True
        assert config.embedding_provider == provider
        assert config.embedding_similarity_threshold == 0.8
        assert config.enable_analysis_caching == False


@pytest.mark.skip(
    reason="EnhancedSemanticChunker not yet implemented in new architecture"
)
class TestEnhancedSemanticChunker(BaseProximaDBTest):
    """Test enhanced semantic chunking with real embeddings"""

    @pytest.fixture
    def chunker_with_embeddings(self):
        """Create chunker with embedding provider"""
        provider = get_default_embedding_provider()

        config = SemanticChunkingConfig(
            use_embeddings=True,
            embedding_provider=provider,
            enable_topic_boundary_detection=True,
            enable_content_type_detection=True,
            enable_semantic_coherence=True,
        )

        return EnhancedSemanticChunker(config)

    def test_content_type_detection(self, chunker_with_embeddings):
        """Test content type detection with embeddings"""
        # Technical content
        tech_text = """
        The API endpoint accepts HTTP POST requests with JSON payloads.
        Authentication is handled via Bearer tokens in the Authorization header.
        Response codes include 200 for success, 401 for unauthorized access.
        The system uses RESTful principles and follows OpenAPI specification.
        """

        content_type = chunker_with_embeddings.analyze_content_type(tech_text)
        assert content_type == ContentType.TECHNICAL_DOCUMENTATION

        # Narrative content
        narrative_text = """
        Once upon a time in a distant land, there lived a young programmer.
        She spent her days writing code and telling stories to her computer.
        The characters in her programs came alive with each line she wrote.
        This narrative continues as she explores the digital realm.
        """

        content_type = chunker_with_embeddings.analyze_content_type(narrative_text)
        assert content_type == ContentType.NARRATIVE_TEXT

        # Academic content
        academic_text = """
        Abstract: This research investigates the effectiveness of semantic chunking.
        Our methodology involves analyzing text coherence using embedding vectors.
        The results demonstrate significant improvements in retrieval accuracy.
        In conclusion, semantic chunking enhances information retrieval systems.
        References: [1] Smith et al., 2023. [2] Johnson, 2024.
        """

        content_type = chunker_with_embeddings.analyze_content_type(academic_text)
        assert content_type == ContentType.ACADEMIC_PAPER

    def test_topic_boundary_detection_with_embeddings(self, chunker_with_embeddings):
        """Test topic boundary detection using real embeddings"""
        sentences = [
            "Machine learning algorithms process large datasets efficiently.",
            "Neural networks can learn complex patterns from data.",
            "Deep learning has revolutionized computer vision tasks.",
            "I love cooking Italian pasta with fresh ingredients.",
            "The best pasta sauce uses ripe tomatoes and basil.",
            "Cooking requires patience and attention to detail.",
        ]

        boundaries = chunker_with_embeddings.detect_topic_boundaries(sentences)

        # Should detect boundary between ML/AI and cooking topics
        assert len(boundaries) > 0

        # The boundary should be around position 3 (between AI and cooking)
        boundary_positions = [b.position for b in boundaries]
        assert 3 in boundary_positions or 4 in boundary_positions

        # Check boundary properties
        for boundary in boundaries:
            assert isinstance(boundary, TopicBoundary)
            assert isinstance(boundary.position, int)
            assert 0 < boundary.confidence <= 1.0
            assert boundary.boundary_type in [
                "embedding_topic_change",
                "text_topic_change",
            ]

    def test_semantic_coherence_with_embeddings(self, chunker_with_embeddings):
        """Test semantic coherence calculation with embeddings"""
        # Coherent text (similar topics)
        coherent_text = """
        Machine learning is a powerful technology.
        AI algorithms can solve complex problems.
        Deep learning models achieve state-of-the-art results.
        Neural networks are inspired by the human brain.
        """

        coherent_score = chunker_with_embeddings.calculate_semantic_coherence(
            coherent_text
        )

        # Incoherent text (random topics)
        incoherent_text = """
        Machine learning is powerful.
        I enjoy eating pizza for lunch.
        The weather today is sunny and warm.
        Basketball is a popular sport worldwide.
        """

        incoherent_score = chunker_with_embeddings.calculate_semantic_coherence(
            incoherent_text
        )

        # Coherent text should have higher score
        assert coherent_score > incoherent_score
        assert 0 <= coherent_score <= 1
        assert 0 <= incoherent_score <= 1

    def test_semantic_segmentation(self, chunker_with_embeddings):
        """Test creating semantic segments with embeddings"""
        text = """
        # Introduction to Machine Learning
        
        Machine learning is a subset of artificial intelligence that enables 
        computers to learn from data without explicit programming. It uses 
        statistical techniques to give computer systems the ability to learn.
        
        ## Supervised Learning
        
        Supervised learning uses labeled training data to learn mappings between 
        inputs and outputs. Common algorithms include decision trees, random 
        forests, and neural networks. These methods are widely used in 
        classification and regression tasks.
        
        ## Unsupervised Learning
        
        Unsupervised learning finds hidden patterns in unlabeled data. 
        Clustering algorithms like K-means and hierarchical clustering group 
        similar data points together. Dimensionality reduction techniques like 
        PCA help visualize high-dimensional data.
        """

        segments = chunker_with_embeddings.create_semantic_segments(text)

        assert len(segments) > 0

        for segment in segments:
            assert isinstance(segment, SemanticSegment)
            assert segment.text
            assert segment.coherence_score >= 0
            assert segment.topic_score >= 0
            assert segment.content_type in ContentType

    def test_chunk_semantically_with_embeddings(self, chunker_with_embeddings):
        """Test full semantic chunking pipeline with embeddings"""
        text = """
        Natural Language Processing (NLP) is a field of artificial intelligence 
        that focuses on the interaction between computers and human language. 
        It involves developing algorithms and models that can understand, 
        interpret, and generate human language in a valuable way.
        
        One of the key challenges in NLP is dealing with the ambiguity and 
        complexity of natural language. Words can have multiple meanings 
        depending on context, and sentences can be structured in countless ways. 
        Modern NLP systems use deep learning models like transformers to better 
        capture these nuances.
        
        Applications of NLP are everywhere in our daily lives. From voice 
        assistants like Siri and Alexa to machine translation services like 
        Google Translate, NLP powers many of the technologies we use. Sentiment 
        analysis helps businesses understand customer feedback, while text 
        summarization helps us digest large amounts of information quickly.
        """

        chunks = chunker_with_embeddings.chunk_semantically(
            text=text,
            source_id="nlp_guide",
            base_metadata={"topic": "NLP", "type": "educational"},
            chunking_config=ChunkingConfig(chunk_size=500, min_chunk_size=100),
        )

        assert len(chunks) > 0

        for chunk in chunks:
            assert isinstance(chunk, TextChunk)
            assert chunk.text
            assert chunk.chunk_id.startswith("nlp_guide")
            assert chunk.metadata.get("chunk_type") in [
                "semantic_enhanced",
                "semantic_enhanced_split",
            ]
            assert chunk.metadata.get("coherence_score") is not None
            assert chunk.metadata.get("topic") == "NLP"

    def test_embedding_provider_fallback(self):
        """Test fallback when embedding provider is unavailable"""
        # Create chunker without embedding provider
        config = SemanticChunkingConfig(
            use_embeddings=False, enable_topic_boundary_detection=True
        )

        chunker = EnhancedSemanticChunker(config)

        sentences = [
            "Python is a programming language.",
            "It is widely used for data science.",
            "Cooking is an art form.",
            "It requires creativity and skill.",
        ]

        # Should still detect boundaries using text-based method
        boundaries = chunker.detect_topic_boundaries(sentences)

        assert len(boundaries) > 0
        # Should use text-based detection
        assert all(b.boundary_type == "text_topic_change" for b in boundaries)


@pytest.mark.skip(reason="Semantic chunking integration needs architecture update")
class TestSemanticChunkingIntegration(BaseProximaDBTest):
    """Integration tests for semantic chunking with vector storage"""

    def test_semantic_chunks_to_vectors(self):
        """Test converting semantic chunks to vectors and storing"""
        # Create collection
        collection_name = self.create_collection(dimension=384)

        # Create chunker with embeddings
        provider = get_default_embedding_provider()
        chunker = create_enhanced_semantic_chunker(
            use_embeddings=True, embedding_provider=provider
        )

        # Test document
        document = """
        # Vector Databases Overview
        
        Vector databases are specialized systems designed to store and search 
        high-dimensional vector embeddings. They use advanced indexing algorithms 
        like HNSW and IVF to enable fast similarity search across millions of vectors.
        
        ## Key Features
        
        Modern vector databases support metadata filtering, allowing users to 
        combine semantic search with traditional filters. They also provide 
        horizontal scaling capabilities and can handle real-time updates while 
        maintaining search performance.
        
        ## Use Cases
        
        Vector databases power many AI applications including recommendation systems, 
        semantic search engines, and RAG (Retrieval Augmented Generation) systems. 
        They are essential infrastructure for building AI-powered applications.
        """

        # Chunk document
        chunks = chunker.chunk_semantically(
            text=document,
            source_id="vector_db_doc",
            base_metadata={"doc_type": "technical"},
            chunking_config=ChunkingConfig(chunk_size=300),
        )

        # Generate embeddings for chunks
        chunk_texts = [chunk.text for chunk in chunks]
        embeddings = provider.embed_texts(chunk_texts)

        # Convert to vector records
        from proximadb_sdk.chunking import chunks_to_vector_records

        records = chunks_to_vector_records(
            chunks=chunks,
            embeddings=embeddings.tolist(),
            source_type="documentation",
            filterable_fields=["doc_type", "chunk_type"],
        )

        # Insert into collection
        response = self.rest_client.insert_vectors(collection_name, records=records)

        assert response.get("success") or response.get("inserted") == len(records)

        # Wait for indexing
        self.wait_for_indexing()

        # Search using semantic query
        query_text = "How do vector databases handle searching?"
        query_embedding = provider.embed_text(query_text)

        results = self.rest_client.search(
            collection_name, query_embedding.tolist(), top_k=3
        )

        self.verify_search_results(results, expected_count=min(3, len(chunks)))

        # Verify metadata
        for result in results:
            metadata = result.get("metadata", {})
            assert metadata.get("doc_type") == "technical"
            assert metadata.get("chunk_type") in [
                "semantic_enhanced",
                "semantic_enhanced_split",
            ]
            assert "coherence_score" in metadata


@pytest.mark.skip(reason="Convenience functions need architecture update")
class TestConvenienceFunctions:
    """Test convenience functions for semantic chunking"""

    def test_create_enhanced_semantic_chunker(self):
        """Test convenience function for creating chunker"""
        chunker = create_enhanced_semantic_chunker(
            enable_topic_detection=True,
            enable_content_analysis=False,
            topic_threshold=0.5,
            enable_caching=True,
            use_embeddings=True,
        )

        assert isinstance(chunker, EnhancedSemanticChunker)
        assert chunker.config.enable_topic_boundary_detection == True
        assert chunker.config.enable_content_type_detection == False
        assert chunker.config.topic_similarity_threshold == 0.5
        assert chunker.config.enable_analysis_caching == True
        assert chunker.config.use_embeddings == True

    def test_chunker_stats(self):
        """Test getting chunker statistics"""
        chunker = create_enhanced_semantic_chunker()

        stats = chunker.get_analysis_stats()

        assert isinstance(stats, dict)
        assert "config" in stats
        assert stats["config"]["topic_boundary_detection"] == True
        assert stats["patterns_compiled"] > 0


@pytest.mark.skip(reason="Chunking fallback needs architecture update")
class TestChunkingFallback(BaseProximaDBTest):
    """Test fallback behavior for semantic chunking"""

    def test_fallback_to_basic_semantic(self):
        """Test fallback to basic semantic chunking"""
        # Use standard TextChunker with semantic strategy
        config = ChunkingConfig(strategy=ChunkingStrategy.SEMANTIC)
        chunker = TextChunker(config)

        text = """
        # Test Document
        
        This is a test document with multiple sections.
        It should be chunked using the basic semantic strategy.
        
        ## Section Two
        
        This is another section with different content.
        The chunker should handle this appropriately.
        """

        chunks = chunker.chunk_text(text, "test_doc", {"source": "test"})

        assert len(chunks) > 0

        # Should create chunks with appropriate types
        chunk_types = set(c.metadata.get("chunk_type") for c in chunks)
        valid_types = {
            "semantic_basic",
            "semantic_enhanced",
            "bert_semantic_enhanced",
            "paragraph",
            "sliding_window",
            "sentence",
        }

        assert len(chunk_types & valid_types) > 0
