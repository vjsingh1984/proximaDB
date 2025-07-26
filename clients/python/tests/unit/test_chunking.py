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
        config = ChunkingConfig()
        chunker = TextChunker(config)
        
        # Test basic chunking
        text = "This is a test text that should be chunked."
        chunks = chunker.chunk_text(text)
        assert len(chunks) > 0
        assert all(isinstance(chunk, TextChunk) for chunk in chunks)
    
    def test_chunk_text_with_sentence_strategy(self):
        """Test chunking with sentence strategy"""
        config = ChunkingConfig(strategy=ChunkingStrategy.SENTENCE)
        chunker = TextChunker(config)
        
        text = "This is the first sentence. This is the second sentence. And here is the third."
        chunks = chunker.chunk_text(text)
        
        assert len(chunks) == 3
        assert chunks[0].text == "This is the first sentence."
        assert chunks[1].text == "This is the second sentence."
        assert chunks[2].text == "And here is the third."
    
    def test_chunk_text_with_paragraph_strategy(self):
        """Test chunking with paragraph strategy"""
        config = ChunkingConfig(strategy=ChunkingStrategy.PARAGRAPH)
        chunker = TextChunker(config)
        
        text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph."
        chunks = chunker.chunk_text(text)
        
        assert len(chunks) == 3
        assert chunks[0].text == "First paragraph."
        assert chunks[1].text == "Second paragraph."
        assert chunks[2].text == "Third paragraph."
    
    def test_chunk_text_with_sliding_window_strategy(self):
        """Test chunking with sliding window strategy"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SLIDING_WINDOW,
            chunk_size=20,
            chunk_overlap=10
        )
        chunker = TextChunker(config)
        
        text = "0123456789abcdefghijklmnopqrstuvwxyz"
        chunks = chunker.chunk_text(text)
        
        # First chunk: 20 chars
        assert chunks[0].text == "0123456789abcdefghij"
        # Second chunk: overlaps by 10 chars
        assert chunks[1].text == "abcdefghijklmnopqrst"
    
    def test_chunk_text_with_fixed_size_strategy(self):
        """Test chunking with fixed size strategy"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.FIXED_SIZE,
            chunk_size=10
        )
        chunker = TextChunker(config)
        
        text = "0123456789abcdefghij"
        chunks = chunker.chunk_text(text)
        
        assert len(chunks) == 2
        assert chunks[0].text == "0123456789"
        assert chunks[1].text == "abcdefghij"
    
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
        config = ChunkingConfig()
        chunker = TextChunker(config)
        
        text = "Test text for metadata"
        metadata = {"source": "test.txt", "author": "Test Author"}
        chunks = chunker.chunk_text(text, metadata=metadata)
        
        assert len(chunks) > 0
        for chunk in chunks:
            assert "source" in chunk.metadata
            assert chunk.metadata["source"] == "test.txt"
            assert "author" in chunk.metadata
            assert chunk.metadata["author"] == "Test Author"
            assert "chunk_index" in chunk.metadata
            assert "total_chunks" in chunk.metadata


class TestChunkingFunctions:
    """Test standalone chunking functions"""
    
    def test_chunk_by_sentences(self):
        """Test chunk_by_sentences function"""
        text = "First sentence. Second sentence! Third sentence?"
        chunks = chunk_by_sentences(text)
        
        assert len(chunks) == 3
        assert chunks[0].text == "First sentence."
        assert chunks[1].text == "Second sentence!"
        assert chunks[2].text == "Third sentence?"
    
    def test_chunk_by_paragraphs(self):
        """Test chunk_by_paragraphs function"""
        text = "Para 1\n\nPara 2\n\nPara 3"
        chunks = chunk_by_paragraphs(text)
        
        assert len(chunks) == 3
        assert chunks[0].text == "Para 1"
        assert chunks[1].text == "Para 2"
        assert chunks[2].text == "Para 3"
    
    def test_chunk_sliding_window(self):
        """Test chunk_sliding_window function"""
        text = "This is a longer text that needs to be chunked into smaller pieces."
        chunks = chunk_sliding_window(text, window_size=30, overlap=10)
        
        assert len(chunks) > 1
        # Check overlap
        if len(chunks) > 1:
            # Last 10 chars of first chunk should be first 10 chars of second chunk
            assert chunks[0].text[-10:] == chunks[1].text[:10]
    
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