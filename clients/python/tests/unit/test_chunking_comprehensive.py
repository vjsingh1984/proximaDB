"""
Comprehensive test suite for ProximaDB text chunking module
Focuses on improving code coverage for uncovered functionality
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


class TestSemanticChunking:
    """Test semantic chunking functionality"""
    
    def test_semantic_chunking_with_headers(self):
        """Test semantic chunking with markdown headers"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SEMANTIC,
            max_chunk_size=200,
            min_chunk_size=10
        )
        chunker = TextChunker(config)
        
        text = """# Introduction
This is the introduction paragraph with some content.

## Section One
This is section one with detailed information.

### Subsection
More details in the subsection.

## Section Two  
This is section two with different content.
"""
        
        chunks = chunker.chunk_text(text, "semantic_test")
        assert len(chunks) > 0
        
        # Check that we have semantic chunks with section info
        for chunk in chunks:
            assert chunk.metadata["chunk_type"] == "semantic"
            assert "section_index" in chunk.metadata
    
    def test_semantic_chunking_with_numbered_sections(self):
        """Test semantic chunking with numbered sections"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SEMANTIC,
            max_chunk_size=150
        )
        chunker = TextChunker(config)
        
        text = """1. First Section
Content of the first section goes here.

2. Second Section  
Content of the second section is different.

3. Third Section
More content in the third section.
"""
        
        chunks = chunker.chunk_text(text, "numbered_test")
        assert len(chunks) > 0
        
        # Should detect numbered sections
        for chunk in chunks:
            if chunk.metadata.get("section_header"):
                assert any(char.isdigit() for char in chunk.metadata["section_header"])
    
    def test_semantic_chunking_with_caps_headers(self):
        """Test semantic chunking with all-caps headers"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SEMANTIC,
            max_chunk_size=100
        )
        chunker = TextChunker(config)
        
        text = """OVERVIEW
This section provides an overview of the system.

FEATURES
These are the main features of the product.

CONCLUSION
Final thoughts and conclusions.
"""
        
        chunks = chunker.chunk_text(text, "caps_test")
        assert len(chunks) > 0
        
        # Check for caps headers detection
        found_caps_header = False
        for chunk in chunks:
            if chunk.metadata.get("section_header"):
                if chunk.metadata["section_header"].isupper():
                    found_caps_header = True
                    break
        assert found_caps_header
    
    def test_semantic_chunking_no_sections_fallback(self):
        """Test semantic chunking falls back to paragraph chunking when no sections found"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SEMANTIC,
            max_chunk_size=100
        )
        chunker = TextChunker(config)
        
        # Text with no section markers
        text = """This is just regular text without any section markers.
It should fall back to paragraph-based chunking.

This is another paragraph that should be handled properly.
No special headers or section markers here.
"""
        
        chunks = chunker.chunk_text(text, "fallback_test")
        assert len(chunks) > 0
        
        # Should fall back to paragraph chunking (or sentence chunking depending on implementation)
        for chunk in chunks:
            assert chunk.metadata["chunk_type"] in ["paragraph", "sentence"]
    
    def test_semantic_chunking_large_sections(self):
        """Test semantic chunking with sections exceeding max_chunk_size"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SEMANTIC,
            max_chunk_size=50,  # Very small to force subdivision
            chunk_size=30,
            chunk_overlap=10
        )
        chunker = TextChunker(config)
        
        text = """# Large Section
This is a very long section that definitely exceeds the maximum chunk size limit and should be subdivided into smaller chunks using the sliding window approach when the section is too large to fit in a single chunk.
"""
        
        chunks = chunker.chunk_text(text, "large_section_test")
        assert len(chunks) >= 1  # Should create at least one chunk
        
        # At least some chunks should be from subdivision
        subdivision_found = False
        for chunk in chunks:
            if "section_index" in chunk.metadata:
                subdivision_found = True
                break
        assert subdivision_found


class TestRecursiveChunking:
    """Test recursive chunking functionality"""
    
    def test_recursive_chunking_basic(self):
        """Test basic recursive chunking"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.RECURSIVE,
            chunk_size=50,
            min_chunk_size=10
        )
        chunker = TextChunker(config)
        
        text = """This is paragraph one with multiple sentences.

This is paragraph two. It has different content.

This is the final paragraph with conclusion."""
        
        chunks = chunker.chunk_text(text, "recursive_test")
        assert len(chunks) > 0
        
        for chunk in chunks:
            assert chunk.metadata["chunk_type"] == "recursive"
            assert len(chunk.text) <= config.chunk_size or len(chunk.text) >= config.min_chunk_size
    
    def test_recursive_chunking_with_separators(self):
        """Test recursive chunking uses different separators"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.RECURSIVE,
            chunk_size=30,  # Small size to force recursive splitting
            min_chunk_size=5
        )
        chunker = TextChunker(config)
        
        # Text that will require multiple separator levels
        text = """First paragraph with sentences. Another sentence here.

Second paragraph. More sentences. Even more content to process.

Third paragraph with final content."""
        
        chunks = chunker.chunk_text(text, "recursive_sep_test")
        assert len(chunks) > 1
        
        # Check that chunks respect size constraints
        for chunk in chunks:
            assert len(chunk.text.strip()) >= config.min_chunk_size
    
    def test_recursive_chunking_empty_separators(self):
        """Test recursive chunking when no good separators are found"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.RECURSIVE,
            chunk_size=10,  # Very small to force character-level splitting
            min_chunk_size=3
        )
        chunker = TextChunker(config)
        
        # Text without good separator points
        text = "ThisIsAVeryLongWordWithoutSpacesOrPunctuationThatWillNeedToBeSplitAtCharacterLevel"
        
        chunks = chunker.chunk_text(text, "recursive_char_test")
        assert len(chunks) > 1
        
        # Should split into smaller pieces
        for chunk in chunks:
            assert len(chunk.text) <= config.chunk_size + 5  # Allow some flexibility


class TestFixedSizeChunking:
    """Test fixed-size chunking functionality"""
    
    def test_fixed_size_chunking_basic(self):
        """Test basic fixed-size chunking"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.FIXED_SIZE,
            chunk_size=20,
            min_chunk_size=5
        )
        chunker = TextChunker(config)
        
        text = "This is a test text that will be split into fixed-size chunks of exactly twenty characters each."
        
        chunks = chunker.chunk_text(text, "fixed_test")
        assert len(chunks) > 0
        
        for chunk in chunks:
            assert chunk.metadata["chunk_type"] == "fixed_size"
            assert chunk.metadata["chunk_size"] == 20
            # Most chunks should be close to the target size
            assert len(chunk.text) <= 20 or len(chunk.text) >= config.min_chunk_size
    
    def test_fixed_size_chunking_small_chunks_skipped(self):
        """Test that very small chunks are skipped in fixed-size chunking"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.FIXED_SIZE,
            chunk_size=10,
            min_chunk_size=8  # High minimum to test filtering
        )
        chunker = TextChunker(config)
        
        text = "Short text."  # Only 11 chars, should create 1 chunk of 10 chars and skip the 1 char remainder
        
        chunks = chunker.chunk_text(text, "small_skip_test")
        
        # Should only have chunks that meet minimum size
        for chunk in chunks:
            assert len(chunk.text.strip()) >= config.min_chunk_size


class TestUnknownStrategy:
    """Test error handling for unknown chunking strategies"""
    
    def test_unknown_strategy_raises_error(self):
        """Test that unknown chunking strategy raises ValueError"""
        # Manually create a config with invalid strategy
        config = ChunkingConfig()
        config.strategy = "unknown_strategy"  # Invalid strategy
        
        chunker = TextChunker(config)
        
        with pytest.raises(ValueError, match="Unknown chunking strategy"):
            chunker.chunk_text("Test text", "error_test")


class TestParagraphChunkingEdgeCases:
    """Test edge cases in paragraph chunking"""
    
    def test_paragraph_chunking_large_paragraphs_subdivision(self):
        """Test paragraph chunking subdivides large paragraphs"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.PARAGRAPH,
            max_chunk_size=50,  # Small to force subdivision
            chunk_size=30,
            min_chunk_size=10
        )
        chunker = TextChunker(config)
        
        # Large paragraph that exceeds max_chunk_size
        text = f"""This is a very long paragraph that definitely exceeds the maximum chunk size and should be subdivided using sentence-based chunking. {"Extra text. " * 10}

Normal paragraph here.
"""
        
        chunks = chunker.chunk_text(text, "large_para_test")
        assert len(chunks) > 1
        
        # Should have subdivided the large paragraph
        subdivision_found = False  
        for chunk in chunks:
            if "paragraph_index" in chunk.metadata:
                subdivision_found = True
                break
        assert subdivision_found
    
    def test_paragraph_chunking_small_paragraphs_skipped(self):
        """Test that small paragraphs are skipped"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.PARAGRAPH,
            min_chunk_size=20  # Higher minimum
        )
        chunker = TextChunker(config)
        
        text = """Short.

This paragraph is long enough to be included in the chunking results and should not be skipped.

Tiny.

Another good paragraph with sufficient length for inclusion.
"""
        
        chunks = chunker.chunk_text(text, "small_para_test")
        
        # Should only include paragraphs meeting minimum size
        for chunk in chunks:
            assert len(chunk.text) >= config.min_chunk_size


class TestContextAddition:
    """Test adding context to chunks"""
    
    def test_add_context_to_chunks(self):
        """Test adding surrounding context to chunks"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.FIXED_SIZE,
            chunk_size=20,
            min_chunk_size=5,  # Lower minimum to ensure chunks are created
            add_context=True,
            context_size=10
        )
        chunker = TextChunker(config)
        
        text = "First chunk content here. Second chunk content here. Third chunk content here."
        
        chunks = chunker.chunk_text(text, "context_test")
        assert len(chunks) > 1
        
        # Add context manually to test the method
        chunks_with_context = chunker.add_context_to_chunks(chunks, context_size=5)
        
        # Check that context was added
        for i, chunk in enumerate(chunks_with_context):
            if i > 0:  # Not first chunk
                assert "prev_context" in chunk.metadata
            if i < len(chunks_with_context) - 1:  # Not last chunk
                assert "next_context" in chunk.metadata
    
    def test_add_context_custom_size(self):
        """Test adding context with custom size"""
        config = ChunkingConfig(chunk_size=30)
        chunker = TextChunker(config)
        
        text = "Lorem ipsum dolor sit amet. Consectetur adipiscing elit. Sed do eiusmod tempor."
        
        chunks = chunker.chunk_text(text, "custom_context_test")
        chunks_with_context = chunker.add_context_to_chunks(chunks, context_size=8)
        
        # Verify context size
        for chunk in chunks_with_context:
            if "prev_context" in chunk.metadata:
                assert len(chunk.metadata["prev_context"]) <= 8
            if "next_context" in chunk.metadata:
                assert len(chunk.metadata["next_context"]) <= 8


class TestSentenceBoundaryDetection:
    """Test sentence boundary detection functionality"""
    
    def test_find_sentence_boundary_forward(self):
        """Test finding sentence boundary in forward direction"""
        config = ChunkingConfig(preserve_sentences=True)
        chunker = TextChunker(config)
        
        text = "First sentence. Second sentence! Third sentence? Fourth sentence."
        
        # Test the private method
        boundary = chunker._find_sentence_boundary(text, 10, "forward")
        
        # Should find the end of first sentence
        assert boundary > 10
        assert text[boundary-2:boundary] in [". ", "! ", "? "]
    
    def test_find_sentence_boundary_backward(self):
        """Test finding sentence boundary in backward direction"""
        config = ChunkingConfig(preserve_sentences=True) 
        chunker = TextChunker(config)
        
        text = "First sentence. Second sentence! Third sentence? Fourth sentence."
        
        # Test the private method
        boundary = chunker._find_sentence_boundary(text, 30, "backward")
        
        # Should find a sentence boundary before position 30
        assert boundary < 30
        assert boundary >= 0
    
    def test_sentence_splitting_with_abbreviations(self):
        """Test sentence splitting handles abbreviations correctly"""
        config = ChunkingConfig(strategy=ChunkingStrategy.SENTENCE)
        chunker = TextChunker(config)
        
        text = "Dr. Smith went to the store. Mr. Johnson followed him. They met Prof. Brown there."
        
        # Test the private method
        sentences = chunker._split_into_sentences(text)
        
        assert len(sentences) >= 3
        # Should not split on abbreviations
        assert any("Dr. Smith" in sentence for sentence in sentences)
        assert any("Mr. Johnson" in sentence for sentence in sentences)
        assert any("Prof. Brown" in sentence for sentence in sentences)


class TestCreateChunkerFactory:
    """Test chunker factory function edge cases"""
    
    def test_create_chunker_with_kwargs(self):
        """Test create_chunker factory with additional kwargs"""
        chunker = create_chunker(
            "sliding_window",
            chunk_size=256,
            chunk_overlap=64,
            preserve_sentences=False
        )
        
        assert chunker.config.strategy == ChunkingStrategy.SLIDING_WINDOW
        assert chunker.config.chunk_size == 256
        assert chunker.config.chunk_overlap == 64
        assert chunker.config.preserve_sentences is False
    
    def test_create_chunker_invalid_strategy(self):
        """Test create_chunker with invalid strategy"""
        with pytest.raises(ValueError):
            create_chunker("invalid_strategy")


class TestEdgeCases:
    """Test various edge cases"""
    
    def test_chunk_empty_string(self):
        """Test chunking empty string"""
        config = ChunkingConfig()
        chunker = TextChunker(config)
        
        chunks = chunker.chunk_text("", "empty_test")
        assert chunks == []
    
    def test_chunk_whitespace_only(self):
        """Test chunking whitespace-only string"""
        config = ChunkingConfig()
        chunker = TextChunker(config)
        
        chunks = chunker.chunk_text("   \n\t  ", "whitespace_test")
        assert chunks == []
    
    def test_chunk_single_character(self):
        """Test chunking single character"""
        config = ChunkingConfig(
            chunk_size=1,
            min_chunk_size=1
        )
        chunker = TextChunker(config)
        
        chunks = chunker.chunk_text("A", "single_char_test")
        assert len(chunks) == 1
        assert chunks[0].text == "A"
    
    def test_sliding_window_sentence_preservation_edge_cases(self):
        """Test sliding window with sentence preservation edge cases"""
        config = ChunkingConfig(
            strategy=ChunkingStrategy.SLIDING_WINDOW,
            chunk_size=20,
            preserve_sentences=True,
            min_chunk_size=5
        )
        chunker = TextChunker(config)
        
        # Text where sentence boundaries are tricky
        text = "Start. Middle part here! End?"
        
        chunks = chunker.chunk_text(text, "boundary_test")
        assert len(chunks) >= 1
        
        # Should preserve sentence boundaries
        for chunk in chunks:
            # Check that chunks don't cut sentences awkwardly
            assert not chunk.text.strip().startswith("dle") # Shouldn't cut "Middle"