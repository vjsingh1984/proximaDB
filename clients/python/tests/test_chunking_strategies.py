"""
Test all chunking strategies with ProximaDB SDK
"""
import pytest
from proximadb.chunking import (
    TextChunker, ChunkingConfig, ChunkingStrategy,
    chunk_by_sentences, chunk_by_paragraphs, chunk_sliding_window,
    create_chunker
)


class TestChunkingStrategies:
    """Test various text chunking strategies"""
    
    @pytest.fixture
    def sample_text(self):
        """Sample text for testing"""
        return """ProximaDB is a high-performance vector database. It supports multiple storage engines.

The VIPER engine uses columnar storage for analytics. It provides excellent compression and query performance.

The SST engine uses row-based storage. It excels at write-heavy workloads and provides consistent performance.

Both engines support advanced indexing algorithms. These include HNSW, IVF, and LSH for different use cases."""
    
    def test_sentence_chunking(self, sample_text):
        """Test sentence-based chunking"""
        chunks = chunk_by_sentences(sample_text, chunk_size=100)
        
        assert len(chunks) > 0
        # Each chunk should contain complete sentences
        for chunk in chunks:
            assert chunk.text.strip()
            assert not chunk.text.startswith(' ')
            assert chunk.metadata["chunk_type"] == "sentence"
            assert "sentence_count" in chunk.metadata
    
    def test_paragraph_chunking(self, sample_text):
        """Test paragraph-based chunking"""
        chunks = chunk_by_paragraphs(sample_text, max_size=200)
        
        assert len(chunks) == 4  # 4 paragraphs in sample
        for i, chunk in enumerate(chunks):
            assert chunk.metadata["chunk_type"] == "paragraph"
            assert chunk.metadata["paragraph_index"] == i
    
    def test_sliding_window_chunking(self, sample_text):
        """Test sliding window chunking"""
        chunks = chunk_sliding_window(
            sample_text,
            window_size=100,
            overlap=20
        )
        
        assert len(chunks) > 1
        # Check overlap exists
        for i in range(len(chunks) - 1):
            chunk1_end = chunks[i].text[-20:]
            chunk2_start = chunks[i+1].text[:20]
            # Some overlap should exist (not exact due to sentence preservation)
            assert any(word in chunk2_start for word in chunk1_end.split())
    
    def test_semantic_chunking(self, sample_text):
        """Test semantic chunking"""
        # Add headers to make semantic boundaries clear
        text_with_headers = """# Vector Databases

ProximaDB is a high-performance vector database.

## Storage Engines

The VIPER engine uses columnar storage.
The SST engine uses row-based storage.

## Indexing Algorithms

Support for HNSW, IVF, and LSH."""
        
        chunker = create_chunker("semantic")
        chunks = chunker.chunk_text(text_with_headers)
        
        assert len(chunks) >= 3  # At least 3 sections
        for chunk in chunks:
            assert chunk.metadata["chunk_type"] == "semantic"
            if "section_header" in chunk.metadata:
                assert chunk.metadata["section_header"].startswith("#")
    
    def test_fixed_size_chunking(self, sample_text):
        """Test fixed size chunking"""
        chunker = create_chunker("fixed_size", chunk_size=50)
        chunks = chunker.chunk_text(sample_text)
        
        assert len(chunks) > 0
        for chunk in chunks[:-1]:  # All but last
            assert len(chunk.text) <= 50
            assert chunk.metadata["chunk_type"] == "fixed_size"
    
    def test_recursive_chunking(self, sample_text):
        """Test recursive chunking"""
        chunker = create_chunker("recursive", chunk_size=100)
        chunks = chunker.chunk_text(sample_text)
        
        assert len(chunks) > 0
        for chunk in chunks:
            assert chunk.metadata["chunk_type"] == "recursive"
            assert len(chunk.text) <= 100 or "." not in chunk.text
    
    def test_chunk_metadata_enrichment(self):
        """Test metadata enrichment during chunking"""
        text = "ProximaDB version 2.0 supports GPU acceleration."
        
        metadata = {
            "source": "documentation",
            "version": "2.0",
            "author": "ProximaDB Team"
        }
        
        chunks = chunk_by_sentences(
            text,
            chunk_size=100,
            document_id="doc_123",
            metadata=metadata
        )
        
        assert len(chunks) == 1
        chunk = chunks[0]
        
        # Original metadata preserved
        assert chunk.metadata["source"] == "documentation"
        assert chunk.metadata["version"] == "2.0"
        assert chunk.metadata["author"] == "ProximaDB Team"
        
        # Auto-generated metadata
        assert chunk.chunk_id == "doc_123_chunk_0"
        assert chunk.metadata["chunk_index"] == 0
        assert chunk.metadata["chunk_type"] == "sentence"
    
    def test_min_chunk_size_filtering(self):
        """Test that small chunks are filtered out"""
        text = "Hi. This is short. But this is a much longer sentence that should be kept."
        
        chunker = create_chunker(
            "sentence",
            chunk_size=50,
            min_chunk_size=20
        )
        chunks = chunker.chunk_text(text)
        
        # Short sentences should be grouped or filtered
        for chunk in chunks:
            assert len(chunk.text) >= 20
    
    def test_preserve_sentences_option(self):
        """Test sentence preservation in sliding window"""
        text = "First sentence here. Second sentence is longer and continues. Third one."
        
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SLIDING_WINDOW,
            chunk_size=30,
            chunk_overlap=10,
            preserve_sentences=True
        )
        chunker = TextChunker(config)
        chunks = chunker.chunk_text(text)
        
        # Each chunk should start/end at sentence boundaries
        for chunk in chunks:
            assert not chunk.text.startswith(" is ")
            assert not chunk.text.endswith(" and")
    
    def test_context_addition(self):
        """Test adding surrounding context to chunks"""
        text = "First chunk content. Second chunk content. Third chunk content."
        
        chunker = create_chunker("sentence", chunk_size=25)
        chunks = chunker.chunk_text(text)
        chunks = chunker.add_context_to_chunks(chunks, context_size=10)
        
        # Middle chunks should have both prev and next context
        if len(chunks) > 2:
            middle = chunks[1]
            assert "prev_context" in middle.metadata
            assert "next_context" in middle.metadata
            assert len(middle.metadata["prev_context"]) <= 10
            assert len(middle.metadata["next_context"]) <= 10


class TestChunkingEdgeCases:
    """Test edge cases and error handling"""
    
    def test_empty_text(self):
        """Test handling of empty text"""
        chunks = chunk_by_sentences("")
        assert chunks == []
        
        chunks = chunk_by_paragraphs("")
        assert chunks == []
    
    def test_single_word_text(self):
        """Test handling of single word"""
        chunks = chunk_by_sentences("Hello")
        assert len(chunks) == 1
        assert chunks[0].text == "Hello"
    
    def test_unicode_text(self):
        """Test handling of Unicode text"""
        text = "ProximaDB支持中文。它也支持emoji😊。"
        chunks = chunk_by_sentences(text, chunk_size=50)
        
        assert len(chunks) > 0
        for chunk in chunks:
            assert chunk.text  # Non-empty
            # Positions should account for Unicode
            assert chunk.end_pos > chunk.start_pos
    
    def test_very_long_sentence(self):
        """Test handling of sentences longer than chunk size"""
        # Create a 200-character sentence
        long_sentence = "This is " + "very " * 35 + "long."
        
        chunks = chunk_by_sentences(long_sentence, chunk_size=100)
        
        # Should still create a chunk even though it exceeds size
        assert len(chunks) == 1
        assert len(chunks[0].text) > 100
    
    def test_custom_separators(self):
        """Test custom paragraph separators"""
        text = "Part 1\n\n\nPart 2\r\n\r\nPart 3"
        
        chunks = chunk_by_paragraphs(text)
        
        # Should handle different newline styles
        assert len(chunks) == 3
        assert chunks[0].text == "Part 1"
        assert chunks[1].text == "Part 2"
        assert chunks[2].text == "Part 3"
    
    def test_position_tracking(self):
        """Test accurate position tracking"""
        text = "First. Second. Third."
        
        chunks = chunk_by_sentences(text, chunk_size=10)
        
        # Positions should be accurate
        for chunk in chunks:
            extracted = text[chunk.start_pos:chunk.end_pos]
            assert extracted.strip() == chunk.text


class TestChunkingPerformance:
    """Test performance characteristics"""
    
    def test_large_document_chunking(self):
        """Test chunking of large documents"""
        # Generate 10KB of text
        large_text = ("This is a test sentence. " * 100 + "\n\n") * 20
        
        import time
        start = time.time()
        chunks = chunk_sliding_window(large_text, window_size=512, overlap=128)
        elapsed = time.time() - start
        
        assert len(chunks) > 10
        assert elapsed < 1.0  # Should chunk 10KB in under 1 second
        
        # Verify chunk consistency
        for i, chunk in enumerate(chunks):
            assert chunk.chunk_id.endswith(f"chunk_{i}")
            assert 384 <= len(chunk.text) <= 640  # 512 ± 128
    
    def test_memory_efficiency(self):
        """Test memory efficiency of chunking"""
        # This is more of a smoke test
        large_text = "Test. " * 10000  # ~60KB
        
        # Should not raise MemoryError
        chunks = chunk_by_sentences(large_text, chunk_size=1000)
        
        assert len(chunks) > 50
        # Chunks should not reference the original text
        del large_text
        assert chunks[0].text == "Test."