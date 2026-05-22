import pytest

from proximadb_sdk.chunking_strategies.base import (
    ChunkingConfig,
    ChunkingStrategy,
    ChunkingStrategyInterface,
    TextChunk,
)
from proximadb_sdk.chunking_strategies.factory import (
    ChunkingStrategyFactory,
    get_chunking_strategy,
)
from proximadb_sdk.chunking_strategies.fixed_size import FixedSizeStrategy
from proximadb_sdk.chunking_strategies.paragraph import ParagraphStrategy
from proximadb_sdk.chunking_strategies.recursive import RecursiveStrategy
from proximadb_sdk.chunking_strategies.semantic import SemanticStrategy
from proximadb_sdk.chunking_strategies.sentence import SentenceStrategy
from proximadb_sdk.chunking_strategies.sliding_window import SlidingWindowStrategy
from proximadb_sdk.config import ClientConfig, Protocol
from proximadb_sdk.protocol_selector import (
    ProtocolSelector,
    SelectionStrategy,
    create_protocol_selector,
)


class ConcreteStrategy(ChunkingStrategyInterface):
    def chunk(self, text, source_id, base_metadata=None):
        return []


def test_text_chunk_metadata_and_backwards_compatible_positions():
    chunk = TextChunk("hello", start_pos=3, end_pos=8, chunk_id="chunk-1")

    assert chunk.start == 3
    assert chunk.end == 8
    assert chunk.metadata["chunk_length"] == 5
    assert chunk.metadata["chunk_id"] == "chunk-1"


def test_chunking_config_normalizes_overlap_and_max_size():
    too_much_overlap = ChunkingConfig(chunk_size=100, chunk_overlap=1000)
    assert too_much_overlap.chunk_overlap == 20

    negative_overlap = ChunkingConfig(chunk_size=100, chunk_overlap=-5)
    assert negative_overlap.chunk_overlap == 0

    raised_max = ChunkingConfig(chunk_size=100, max_chunk_size=50)
    assert raised_max.max_chunk_size == 100


def test_strategy_interface_validation_rejects_invalid_config():
    invalid_configs = [
        ChunkingConfig(chunk_size=0),
        ChunkingConfig(chunk_size=10, min_chunk_size=-1),
    ]
    max_size_config = ChunkingConfig(chunk_size=10)
    max_size_config.max_chunk_size = 5
    invalid_configs.append(max_size_config)

    for config in invalid_configs:
        strategy = ConcreteStrategy(config)

        with pytest.raises(ValueError):
            strategy.validate_config()


def test_strategy_interface_metadata_and_normalization_helpers():
    strategy = ConcreteStrategy(ChunkingConfig(chunk_size=10))
    chunk = TextChunk("text", 0, 4, "doc_chunk_0")

    strategy.add_chunk_metadata(chunk, 0, 1, "test")
    assert chunk.metadata["chunk_index"] == 0
    assert chunk.metadata["total_chunks"] == 1
    assert chunk.metadata["chunking_strategy"] == "test"

    assert strategy.normalize_text(" alpha   beta \n gamma ") == "alpha beta gamma"


def test_fixed_size_strategy_chunks_and_skips_small_tail():
    strategy = FixedSizeStrategy(
        ChunkingConfig(chunk_size=5, min_chunk_size=3, chunk_overlap=0)
    )
    chunks = strategy.chunk("abcdefghijxy", "doc", {"tenant": "acme"})

    assert [chunk.text for chunk in chunks] == ["abcde", "fghij"]
    assert chunks[0].metadata["chunk_type"] == "fixed_size"
    assert chunks[0].metadata["tenant"] == "acme"
    assert chunks[0].metadata["total_chunks"] == 2
    assert strategy.chunk("   ", "doc") == []


def test_sliding_window_strategy_chunks_with_overlap_and_repr():
    strategy = SlidingWindowStrategy(
        ChunkingConfig(chunk_size=6, chunk_overlap=2, min_chunk_size=2)
    )
    chunks = strategy.chunk("abcdefghijkl", "doc", {"source": "unit"})

    assert [chunk.text for chunk in chunks] == ["abcdef", "efghij", "ijkl"]
    assert chunks[1].metadata["has_overlap"] is True
    assert chunks[1].metadata["overlap_size"] == 2
    assert chunks[-1].metadata["total_chunks"] == 3
    assert "SlidingWindowStrategy" in repr(strategy)
    assert strategy.chunk("", "doc") == []


def test_sentence_strategy_splits_sentences_and_handles_abbreviations():
    strategy = SentenceStrategy(
        ChunkingConfig(chunk_size=34, chunk_overlap=0, min_chunk_size=1)
    )
    text = "Dr. Smith wrote this. Another sentence follows. final fragment"

    sentences = strategy._split_into_sentences(text)
    chunks = strategy.chunk(text, "doc", {"kind": "note"})

    assert sentences[0] == "Dr. Smith wrote this."
    assert chunks[0].metadata["chunk_type"] == "sentence"
    assert chunks[0].metadata["kind"] == "note"
    assert chunks[-1].metadata["total_chunks"] == len(chunks)
    assert "SentenceStrategy" in repr(strategy)
    assert strategy.chunk("", "doc") == []


def test_paragraph_strategy_groups_lists_and_splits_large_paragraphs():
    strategy = ParagraphStrategy(
        ChunkingConfig(chunk_size=55, max_chunk_size=70, min_chunk_size=1)
    )
    text = (
        "Intro paragraph.\n\n"
        "- first item\n- second item\n- third item\n\n"
        "This paragraph is deliberately long. It should split at a sentence boundary."
    )

    paragraphs = strategy._split_into_paragraphs(text)
    chunks = strategy.chunk(text, "doc", {"source": "unit"})
    large_split = strategy._split_large_paragraph(
        "Sentence one is long. Sentence two is also long.", 25
    )

    assert len(paragraphs) == 3
    assert strategy._is_list_paragraph("- a\n- b\n- c") is True
    assert strategy._is_list_paragraph("not a list") is False
    assert len(large_split) == 2
    assert chunks[0].metadata["chunk_type"] == "paragraph"
    list_chunk = strategy._create_chunk(
        "- a\n- b", 0, 99, "doc", {"source": "unit"}, paragraph_count=1, is_list=True
    )
    assert list_chunk.metadata["is_list"] is True
    assert chunks[-1].metadata["total_chunks"] == len(chunks)
    assert "ParagraphStrategy" in repr(strategy)
    assert strategy.chunk("", "doc") == []


def test_semantic_strategy_detects_headers_topics_and_preserves_blocks():
    strategy = SemanticStrategy(
        ChunkingConfig(chunk_size=60, min_chunk_size=1, preserve_tables=True)
    )
    markdown = (
        "Preface text.\n\n"
        "# Overview\n"
        "Alpha details.\n\n"
        "## Data\n"
        "```python\nprint('x')\n```\n\n"
        "| a | b |\n|---|---|\n| 1 | 2 |"
    )

    sections = strategy._identify_sections(markdown)
    preserved_text, preserved_blocks = strategy._preserve_special_blocks(markdown)
    chunks = strategy.chunk(markdown, "doc", {"tenant": "acme"})

    assert sections[0][3]["section_type"] == "introduction"
    assert any(section[3].get("header_title") == "Overview" for section in sections)
    assert "<<CODE_BLOCK_0>>" in preserved_text
    assert any(block["type"] == "code" for block in preserved_blocks)
    assert any(block["type"] == "table" for block in preserved_blocks)
    assert (
        strategy._restore_special_blocks(preserved_text, preserved_blocks) == markdown
    )
    assert chunks
    assert chunks[0].metadata["tenant"] == "acme"
    assert chunks[-1].metadata["total_chunks"] == len(chunks)

    html_sections = strategy._identify_sections("<h2>Intro</h2>Body")
    assert html_sections[0][3]["header_type"] == "html"
    assert html_sections[0][3]["header_title"] == "Intro"

    topic_sections = strategy._identify_topic_sections(
        "First topic.\n\nHowever this is a transition.\n\n---\n\nFinally done."
    )
    assert topic_sections
    assert {section[3]["boundary_type"] for section in topic_sections} <= {
        "section_break",
        "topic_transition",
    }
    assert "SemanticStrategy" in repr(strategy)
    assert strategy.chunk("", "doc") == []


def test_semantic_strategy_splits_large_sections():
    strategy = SemanticStrategy(ChunkingConfig(chunk_size=25, min_chunk_size=1))
    section_metadata = {"header_title": "Large", "has_header": True}

    chunks = strategy._split_large_section(
        "Paragraph one has enough text.\n\nParagraph two has enough text.",
        start_pos=5,
        source_id="doc",
        chunk_index=3,
        base_metadata={"source": "unit"},
        section_metadata=section_metadata,
    )

    assert len(chunks) == 2
    assert chunks[0].metadata["chunk_type"] == "semantic_split"
    assert chunks[0].metadata["parent_section"] == "Large"
    assert chunks[0].metadata["source"] == "unit"


def test_recursive_strategy_uses_paragraph_path_and_empty_input():
    strategy = RecursiveStrategy(
        ChunkingConfig(chunk_size=80, max_chunk_size=100, min_chunk_size=1)
    )
    chunks = strategy.chunk("First paragraph.\n\nSecond paragraph.", "doc")

    assert chunks
    assert chunks[0].metadata["chunk_type"] == "recursive"
    assert chunks[0].metadata["strategy_used"] == "paragraph"
    assert chunks[-1].metadata["total_chunks"] == len(chunks)
    assert "RecursiveStrategy" in repr(strategy)
    assert strategy.chunk("", "doc") == []


def test_recursive_strategy_falls_back_to_sliding_window_for_large_sentences():
    strategy = RecursiveStrategy(
        ChunkingConfig(chunk_size=20, max_chunk_size=15, min_chunk_size=1)
    )
    text = "x" * 55

    chunks = strategy._sliding_window_split(
        text,
        start_pos=10,
        source_id="doc",
        chunk_index=2,
        base_metadata={"source": "unit"},
        parent_chunk_id="parent",
    )

    assert len(chunks) > 1
    assert chunks[0].start_pos == 10
    assert chunks[0].metadata["strategy_used"] == "sliding_window"
    assert chunks[0].metadata["forced_split"] is True
    assert chunks[0].metadata["parent_chunk"] == "parent"


def test_chunking_strategy_factory_and_convenience_function():
    assert "sliding_window" in ChunkingStrategyFactory.list_strategies()

    strategy = ChunkingStrategyFactory.create_strategy(
        ChunkingStrategy.FIXED_SIZE, ChunkingConfig(chunk_size=10)
    )
    assert isinstance(strategy, FixedSizeStrategy)
    assert strategy.config.strategy == ChunkingStrategy.FIXED_SIZE

    from_string = get_chunking_strategy("sliding_window", chunk_size=10)
    assert isinstance(from_string, SlidingWindowStrategy)

    with pytest.raises(ValueError):
        get_chunking_strategy("unknown")


def test_protocol_selector_factory_preserves_legacy_alias_and_factories():
    grpc_client = object()
    rest_client = object()

    selector = create_protocol_selector(
        ClientConfig(url="http://localhost:5678"),
        strategy=SelectionStrategy.PERFORMANCE_BASED,
        grpc_factory=lambda: grpc_client,
        rest_factory=lambda: rest_client,
        health_check_interval_seconds=0,
    )

    assert isinstance(selector, ProtocolSelector)
    assert selector.config.strategy == SelectionStrategy.PERFORMANCE_BASED
    assert selector._client_factories[Protocol.GRPC]() is grpc_client
    assert selector._client_factories[Protocol.REST]() is rest_client
