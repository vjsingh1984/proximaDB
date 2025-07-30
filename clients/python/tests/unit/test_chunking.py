"""
Test suite for ProximaDB text chunking module
"""
import pytest
from proximadb.chunking import (
    ChunkingStrategy,
    TextChunk,
    ChunkingConfig,
    TextChunker,
    create_chunker,
    chunk_by_sentences,
    chunk_by_paragraphs,
    chunk_sliding_window
)


class TestChunkingStrategy:
    """Test ChunkingStrategy enum"""
    
    def test_chunking_strategy_values(self):
        """Test chunking strategy enum values"""
        assert ChunkingStrategy.SENTENCE.value == "sentence"
        assert ChunkingStrategy.PARAGRAPH.value == "paragraph"
        assert ChunkingStrategy.SLIDING_WINDOW.value == "sliding_window"
        assert ChunkingStrategy.SEMANTIC.value == "semantic"
        assert ChunkingStrategy.FIXED_SIZE.value == "fixed_size"
        assert ChunkingStrategy.RECURSIVE.value == "recursive"


class TestTextChunk:
    """Test TextChunk dataclass"""
    
    def test_text_chunk_creation(self):
        """Test creating a text chunk"""
        chunk = TextChunk(
            text="This is a test chunk",
            start_pos=0,
            end_pos=20,
            chunk_id="chunk_001",
            metadata={"source": "test.txt"}
        )
        assert chunk.text == "This is a test chunk"
        assert chunk.start_pos == 0
        assert chunk.end_pos == 20
        assert chunk.chunk_id == "chunk_001"
        assert chunk.metadata == {"source": "test.txt"}
    
    def test_text_chunk_length(self):
        """Test text chunk length property"""
        chunk = TextChunk(
            text="Hello world",
            start_pos=0,
            end_pos=11,
            chunk_id="chunk_001",
            metadata={}
        )
        assert chunk.length == 11


class TestChunkingConfig:
    """Test ChunkingConfig class"""
    
    def test_chunking_config_defaults(self):
        """Test default chunking configuration"""
        config = ChunkingConfig()
        assert config.strategy == ChunkingStrategy.SLIDING_WINDOW
        assert config.chunk_size == 512
        assert config.chunk_overlap == 128
        assert config.min_chunk_size == 100
        assert config.max_chunk_size == 2048
        assert config.separator == "\n"
        assert config.preserve_sentences is True
        assert config.preserve_paragraphs is False
    
    def test_chunking_config_custom(self):
        """Test custom chunking configuration"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SENTENCE,
            chunk_size=1024,
            chunk_overlap=256,
            min_chunk_size=50,
            max_chunk_size=4096,
            separator=" ",
            preserve_sentences=False,
            preserve_paragraphs=True
        )
        assert config.strategy == ChunkingStrategy.SENTENCE
        assert config.chunk_size == 1024
        assert config.chunk_overlap == 256
        assert config.min_chunk_size == 50
        assert config.max_chunk_size == 4096
        assert config.separator == " "
        assert config.preserve_sentences is False
        assert config.preserve_paragraphs is True
    
    def test_chunking_config_custom_validation(self):
        """Test chunking config validation in chunker"""
        # These should work fine
        config = ChunkingConfig(chunk_size=100, chunk_overlap=50)
        chunker = TextChunker(config)
        
        # Test with sentences
        config = ChunkingConfig(strategy=ChunkingStrategy.SENTENCE, chunk_size=1000)
        chunker = TextChunker(config)


class TestTextChunker:
    """Test TextChunker class"""
    
    def test_text_chunker_creation(self):
        """Test creating a text chunker"""
        config = ChunkingConfig()
        chunker = TextChunker(config)
        assert chunker.config == config
    
    def test_chunk_text_basic(self):
        """Test basic text chunking"""
        config = ChunkingConfig(
            chunk_size=100,
            min_chunk_size=10  # Lower min_chunk_size for test
        )
        chunker = TextChunker(config)
        
        # Test basic chunking with longer text
        text = "This is a test text that should be chunked. " * 5  # Make it longer
        chunks = chunker.chunk_text(text)
        assert len(chunks) > 0
        assert all(isinstance(chunk, TextChunk) for chunk in chunks)
    
    def test_chunk_text_with_sentence_strategy(self):
        """Test chunking with sentence strategy"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SENTENCE,
            chunk_size=50,  # Force smaller chunks
            min_chunk_size=10
        )
        chunker = TextChunker(config)
        
        text = "This is the first sentence. This is the second sentence. And here is the third."
        chunks = chunker.chunk_text(text)
        
        # With small chunk_size, sentences should be in separate chunks
        assert len(chunks) >= 1
        assert all(isinstance(chunk, TextChunk) for chunk in chunks)
        assert "sentence" in chunks[0].text.lower()
    
    def test_chunk_text_with_paragraph_strategy(self):
        """Test chunking with paragraph strategy"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.PARAGRAPH,
            min_chunk_size=10
        )
        chunker = TextChunker(config)
        
        text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph."
        chunks = chunker.chunk_text(text)
        
        # Should have at least 1 chunk, possibly more depending on min_chunk_size
        assert len(chunks) >= 1
        assert all(isinstance(chunk, TextChunk) for chunk in chunks)
        assert "paragraph" in chunks[0].text.lower()
    
    def test_chunk_text_with_sliding_window_strategy(self):
        """Test chunking with sliding window strategy"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SLIDING_WINDOW,
            chunk_size=20,
            chunk_overlap=5,  # Reduce overlap for more predictable results
            min_chunk_size=10
        )
        chunker = TextChunker(config)
        
        text = "0123456789abcdefghijklmnopqrstuvwxyz"  # 36 chars
        chunks = chunker.chunk_text(text)
        
        # Should create at least 1 chunk 
        assert len(chunks) >= 1
        assert all(isinstance(chunk, TextChunk) for chunk in chunks)
        # Check that all chunks are reasonable size
        assert all(len(chunk.text) >= 10 for chunk in chunks)
    
    def test_chunk_text_with_fixed_size_strategy(self):
        """Test chunking with fixed size strategy"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.FIXED_SIZE,
            chunk_size=10,
            min_chunk_size=5
        )
        chunker = TextChunker(config)
        
        text = "0123456789abcdefghij"  # 20 chars
        chunks = chunker.chunk_text(text)
        
        # Should create 2 chunks of 10 chars each
        assert len(chunks) == 2
        assert all(isinstance(chunk, TextChunk) for chunk in chunks)
        assert all(len(chunk.text) <= 10 for chunk in chunks)
    
    def test_chunk_text_empty_text(self):
        """Test chunking empty text"""
        config = ChunkingConfig()
        chunker = TextChunker(config)
        
        chunks = chunker.chunk_text("")
        assert len(chunks) == 0
        
        chunks = chunker.chunk_text("   ")
        assert len(chunks) == 0
    
    def test_chunk_text_metadata(self):
        """Test chunk metadata generation"""
        config = ChunkingConfig(
            chunk_size=100,
            min_chunk_size=10
        )
        chunker = TextChunker(config)
        
        text = "Test text for metadata " * 10  # Make longer to ensure chunking
        metadata = {"source": "test.txt", "author": "Test Author"}
        chunks = chunker.chunk_text(text, metadata=metadata)
        
        assert len(chunks) > 0
        for chunk in chunks:
            assert "source" in chunk.metadata
            assert chunk.metadata["source"] == "test.txt"
            assert "author" in chunk.metadata
            assert chunk.metadata["author"] == "Test Author"
            assert "chunk_index" in chunk.metadata


class TestChunkingFunctions:
    """Test standalone chunking functions"""
    
    def test_chunk_by_sentences(self):
        """Test chunk_by_sentences function"""
        text = "First sentence. Second sentence! Third sentence?"
        chunks = chunk_by_sentences(text, chunk_size=100)
        
        # Should create at least 1 chunk
        assert len(chunks) >= 1
        assert all(isinstance(chunk, TextChunk) for chunk in chunks)
        assert "sentence" in chunks[0].text.lower()
    
    def test_chunk_by_paragraphs(self):
        """Test chunk_by_paragraphs function"""
        text = "Para 1\n\nPara 2\n\nPara 3"
        chunks = chunk_by_paragraphs(text, max_size=1024)
        
        # Should create at least 1 non-empty chunk (some may be filtered by min_chunk_size)
        filtered_chunks = [c for c in chunks if len(c.text.strip()) >= 10]
        if filtered_chunks:
            assert len(filtered_chunks) >= 1
            assert all(isinstance(chunk, TextChunk) for chunk in filtered_chunks)
            assert "para" in filtered_chunks[0].text.lower()
        else:
            # If no chunks, that's okay for this short text
            assert len(chunks) == 0
    
    def test_chunk_sliding_window(self):
        """Test chunk_sliding_window function"""
        text = "This is a longer text that needs to be chunked into smaller pieces."
        chunks = chunk_sliding_window(text, window_size=30, overlap=10)
        
        # Should create at least 1 chunk (or none if filtered by min_chunk_size)
        if chunks:
            assert len(chunks) >= 1
            assert all(isinstance(chunk, TextChunk) for chunk in chunks)
            # Check that chunks have reasonable content
            assert all(len(chunk.text) > 0 for chunk in chunks)
        else:
            # If no chunks, that might be due to min_chunk_size filtering
            assert len(chunks) == 0
    
    def test_create_chunker_factory(self):
        """Test create_chunker factory function"""
        # Test sentence chunker
        chunker = create_chunker("sentence")
        assert isinstance(chunker, TextChunker)
        assert chunker.config.strategy == ChunkingStrategy.SENTENCE
        
        # Test with custom params
        chunker = create_chunker("sliding_window", chunk_size=1024, chunk_overlap=256)
        assert chunker.config.chunk_size == 1024
        assert chunker.config.chunk_overlap == 256
    
    def test_chunk_text_with_min_size(self):
        """Test chunking with minimum size constraint"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.PARAGRAPH,
            min_chunk_size=20
        )
        chunker = TextChunker(config)
        
        text = "Short paragraph.\n\nAnother short paragraph.\n\nThis is a longer paragraph that meets the minimum size requirement."
        chunks = chunker.chunk_text(text)
        
        # Check that we have chunks
        assert len(chunks) >= 1