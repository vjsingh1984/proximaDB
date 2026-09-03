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


def test_fixed_size_strategy_chunks_and_keeps_small_tail():
    """The undersized final window is retained, not dropped.

    Previously asserted ``["abcde", "fghij"]`` — i.e. that the trailing ``"xy"``
    was discarded. Silently dropping the tail loses content; chunking is a total
    partition of its input (ADR-091 axiom 1).
    """
    strategy = FixedSizeStrategy(
        ChunkingConfig(chunk_size=5, min_chunk_size=3, chunk_overlap=0)
    )
    source = "abcdefghijxy"
    chunks = strategy.chunk(source, "doc", {"tenant": "acme"})

    assert [chunk.text for chunk in chunks] == ["abcde", "fghij", "xy"]
    assert "".join(chunk.text for chunk in chunks) == source
    for chunk in chunks:
        assert source[chunk.start_pos : chunk.end_pos] == chunk.text
    assert chunks[0].metadata["chunk_type"] == "fixed_size"
    assert chunks[0].metadata["tenant"] == "acme"
    assert chunks[0].metadata["total_chunks"] == 3
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
    # THREE, not two. "Sentence two is also long." is 26 characters and the cap
    # is 25, so the cap backstop splits it -- which is the behaviour the
    # max_chunk_size post-condition exists to guarantee. Expecting two encoded
    # the pre-cap behaviour, where an oversized sentence was emitted whole.
    assert large_split == [
        "Sentence one is long.",
        "Sentence two is also",
        "long.",
    ]
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

    sections = strategy._section_spans(markdown)
    barriers = strategy._protected_spans(markdown)
    chunks = strategy.chunk(markdown, "doc", {"tenant": "acme"})

    # Sections are (start, end, metadata) spans and tile the document
    # contiguously, which is what makes totality structural.
    assert sections[0][2]["section_type"] == "introduction"
    assert any(section[2].get("header_title") == "Overview" for section in sections)
    assert sections[0][0] == 0 and sections[-1][1] == len(markdown)
    for earlier, later in zip(sections, sections[1:], strict=False):
        assert earlier[1] == later[0]

    # Fences and tables are protected spans now, not placeholder substitutions.
    assert any("print('x')" in markdown[a:b] for a, b in barriers)
    assert any(markdown[a:b].lstrip().startswith("|") for a, b in barriers)

    assert chunks
    assert chunks[0].metadata["tenant"] == "acme"
    assert chunks[-1].metadata["total_chunks"] == len(chunks)
    # The heading line now belongs to a chunk; it previously belonged to none.
    assert any("# Overview" in c.text for c in chunks)
    for chunk in chunks:
        assert markdown[chunk.start_pos : chunk.end_pos] == chunk.text

    # `<h2 class=...>` used to be unmatchable, so real HTML never split.
    html_sections = strategy._section_spans('<h2 class="hdr">Intro</h2>Body')
    assert html_sections[0][2]["header_type"] == "html"
    assert html_sections[0][2]["header_title"] == "Intro"

    topic_sections = strategy._topic_section_spans(
        "First topic.\n\nHowever this is a transition.\n\n---\n\nFinally done."
    )
    assert topic_sections
    assert {section[2]["boundary_type"] for section in topic_sections} <= {
        "section_break",
        "topic_transition",
    }
    assert "SemanticStrategy" in repr(strategy)
    assert strategy.chunk("", "doc") == []


def test_semantic_strategy_splits_large_sections():
    """An oversized section splits into spans, not into reconstructed strings.

    Was ``_split_large_section``, which advanced its cursor by the length of the
    NEXT paragraph (``current_chunk_start += len(current_chunk_text)`` right after
    rebinding ``current_chunk_text = para``) — unbounded offset drift, and the
    direct source of the overlapping spans that showed up as no-containment
    violations. The replacement returns ``(span, forced)`` pairs and the text is
    derived from the span.
    """
    strategy = SemanticStrategy(ChunkingConfig(chunk_size=25, min_chunk_size=1))
    source = (
        "# Large\n" "Paragraph one has enough text.\n\nParagraph two has enough text."
    )

    pieces = strategy._split_section(source, 0, len(source), [])
    assert len(pieces) >= 2
    for (start, end), _forced in pieces:
        assert source[start:end] == source[start:end].strip()
        assert end - start <= strategy.config.max_chunk_size
    # Spans are ordered and non-overlapping, which is what the old cursor broke.
    #
    # Each piece is ((start, end), forced) -- the same shape the loop above
    # unpacks. This loop used to destructure it as (start, end), which bound
    # `e1` to the FORCED FLAG and `s2` to a span tuple, so the comparison was
    # bool <= tuple. It never raised because it never ran: the enclosing test
    # was skipped by the --run-slow gate, and that gate silently stopped
    # applying on the pytest 9.0 -> 9.1 upgrade.
    for ((_s1, e1), _f1), ((s2, _e2), _f2) in zip(pieces, pieces[1:], strict=False):
        assert e1 <= s2

    chunks = strategy.chunk(source, "doc", {"source": "unit"})
    assert chunks
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
