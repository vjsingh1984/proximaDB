"""
Offline unit tests for proximadb_sdk.chunking_strategies.

Covers the pure text-chunking strategies (base, paragraph, recursive, semantic,
sentence) plus the factory dispatch. No network, no embeddings, no models — all
strategies are pure-Python text transforms.

The CODE strategy is exercised only at construction time (it falls back to a
regex parser when tree-sitter is unavailable) so we never trigger a model/parser
download.
"""

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
from proximadb_sdk.chunking_strategies.paragraph import ParagraphStrategy
from proximadb_sdk.chunking_strategies.recursive import RecursiveStrategy
from proximadb_sdk.chunking_strategies.semantic import SemanticStrategy
from proximadb_sdk.chunking_strategies.sentence import SentenceStrategy

# --------------------------------------------------------------------------- #
# base.py — TextChunk + ChunkingConfig + interface helpers
# --------------------------------------------------------------------------- #


def test_textchunk_post_init_populates_metadata():
    c = TextChunk(text="hello world", start_pos=0, end_pos=11, chunk_id="src_chunk_0")
    # __post_init__ fills chunk_length + chunk_id when absent
    assert c.metadata["chunk_length"] == len("hello world")
    assert c.metadata["chunk_id"] == "src_chunk_0"
    # backward-compat aliases
    assert c.start == 0
    assert c.end == 11


def test_textchunk_respects_preexisting_metadata():
    c = TextChunk(
        text="abc",
        start_pos=0,
        end_pos=3,
        chunk_id="x_0",
        metadata={"chunk_length": 999, "chunk_id": "custom"},
    )
    assert c.metadata["chunk_length"] == 999
    assert c.metadata["chunk_id"] == "custom"


def test_config_auto_adjusts_overlap_oversize():
    # overlap >= chunk_size => reset to min(20%, chunk_size-1)
    cfg = ChunkingConfig(chunk_size=100, chunk_overlap=150)
    assert cfg.chunk_overlap == 20  # 20% of 100


def test_config_overlap_clamped_when_tiny_chunk():
    cfg = ChunkingConfig(chunk_size=2, chunk_overlap=5)
    # 20% of 2 == 0 -> min(0, 1) == 0
    assert cfg.chunk_overlap == 0


def test_config_negative_overlap_zeroed():
    cfg = ChunkingConfig(chunk_size=100, chunk_overlap=-10)
    assert cfg.chunk_overlap == 0


def test_config_max_chunk_size_bumped_to_chunk_size():
    cfg = ChunkingConfig(chunk_size=500, max_chunk_size=100)
    assert cfg.max_chunk_size == 500


def test_validate_config_passes_for_sane_config():
    s = ParagraphStrategy(ChunkingConfig(chunk_size=100, chunk_overlap=10))
    # should not raise
    s.validate_config()


def test_validate_config_rejects_nonpositive_chunk_size():
    s = ParagraphStrategy(ChunkingConfig())
    s.config.chunk_size = 0
    with pytest.raises(ValueError, match="chunk_size must be positive"):
        s.validate_config()


def test_validate_config_rejects_negative_overlap():
    s = ParagraphStrategy(ChunkingConfig())
    s.config.chunk_overlap = -1
    with pytest.raises(ValueError, match="chunk_overlap cannot be negative"):
        s.validate_config()


def test_validate_config_rejects_overlap_ge_chunk_size():
    s = ParagraphStrategy(ChunkingConfig())
    s.config.chunk_overlap = s.config.chunk_size
    with pytest.raises(ValueError, match="less than chunk_size"):
        s.validate_config()


def test_validate_config_rejects_negative_min_chunk():
    s = ParagraphStrategy(ChunkingConfig())
    s.config.min_chunk_size = -1
    with pytest.raises(ValueError, match="min_chunk_size cannot be negative"):
        s.validate_config()


def test_validate_config_rejects_max_lt_chunk_size():
    s = ParagraphStrategy(ChunkingConfig())
    s.config.max_chunk_size = s.config.chunk_size - 1
    with pytest.raises(ValueError, match="max_chunk_size must be"):
        s.validate_config()


def test_add_chunk_metadata():
    s = ParagraphStrategy(ChunkingConfig(chunk_size=120, chunk_overlap=12))
    c = TextChunk(text="x", start_pos=0, end_pos=1, chunk_id="a_0")
    s.add_chunk_metadata(c, 3, 9, "paragraph")
    assert c.metadata["chunk_index"] == 3
    assert c.metadata["total_chunks"] == 9
    assert c.metadata["chunking_strategy"] == "paragraph"
    assert c.metadata["chunk_size_config"] == 120
    assert c.metadata["chunk_overlap_config"] == 12


def test_normalize_text_collapses_whitespace():
    s = ParagraphStrategy(ChunkingConfig())
    # text.split() collapses ALL whitespace (incl. the paragraph break) to
    # single spaces, so the output is a single normalized line.
    out = s.normalize_text("a   b\n\nc\nd")
    assert out == "a b c d"


def test_interface_is_abstract():
    with pytest.raises(TypeError):
        ChunkingStrategyInterface(ChunkingConfig())  # type: ignore[abstract]


# --------------------------------------------------------------------------- #
# sentence.py
# --------------------------------------------------------------------------- #


def test_sentence_empty_returns_empty():
    s = SentenceStrategy(ChunkingConfig())
    assert s.chunk("", "src") == []


def test_sentence_basic_groups_sentences():
    cfg = ChunkingConfig(
        strategy=ChunkingStrategy.SENTENCE, chunk_size=60, min_chunk_size=1
    )
    s = SentenceStrategy(cfg)
    text = (
        "This is the first sentence. Here comes the second one. "
        "And a third sentence appears. Finally the fourth ends it."
    )
    chunks = s.chunk(text, "doc")
    assert len(chunks) >= 2
    for c in chunks:
        assert c.metadata["chunk_type"] == "sentence"
        assert c.metadata["total_chunks"] == len(chunks)
        assert c.metadata["sentence_count"] >= 1
        assert c.chunk_id.startswith("doc_chunk_")


def test_sentence_abbreviation_not_split():
    s = SentenceStrategy(ChunkingConfig(chunk_size=200, min_chunk_size=1))
    # 'Dr.' should not be treated as a sentence end on its own
    sents = s._split_into_sentences("Dr. Smith arrived. He was late.")
    # The Dr. abbreviation keeps merging until a real sentence end is reached
    assert any("Dr. Smith arrived." in x for x in sents)


def test_sentence_long_first_sentence_truncated_in_metadata():
    long_sentence = "x" * 80 + ". short."
    s = SentenceStrategy(ChunkingConfig(chunk_size=500, min_chunk_size=1))
    chunks = s.chunk(long_sentence, "doc")
    assert chunks
    assert chunks[0].metadata["first_sentence"].endswith("...")


def test_sentence_repr():
    s = SentenceStrategy(ChunkingConfig(chunk_size=77))
    assert "SentenceStrategy" in repr(s)
    assert "77" in repr(s)


def test_sentence_below_min_size_only_one_chunk_kept():
    # min size forces the trailing-chunk "or not chunks" branch
    s = SentenceStrategy(ChunkingConfig(chunk_size=10, min_chunk_size=1000))
    chunks = s.chunk("Short. Tiny.", "doc")
    # Nothing exceeds min, but the final fallback keeps one chunk when none exist
    assert len(chunks) == 1


# --------------------------------------------------------------------------- #
# paragraph.py
# --------------------------------------------------------------------------- #


def test_paragraph_empty_returns_empty():
    s = ParagraphStrategy(ChunkingConfig())
    assert s.chunk("", "src") == []


def test_paragraph_whitespace_only_returns_empty():
    s = ParagraphStrategy(ChunkingConfig())
    # _split_into_paragraphs yields nothing -> []
    assert s.chunk("   \n\n   ", "src") == []


def test_paragraph_basic_grouping():
    cfg = ChunkingConfig(chunk_size=200, min_chunk_size=1, max_chunk_size=400)
    s = ParagraphStrategy(cfg)
    text = "First paragraph here.\n\nSecond paragraph here.\n\nThird paragraph here."
    chunks = s.chunk(text, "doc")
    assert chunks
    for c in chunks:
        assert c.metadata["chunk_type"] == "paragraph"
        assert c.metadata["total_chunks"] == len(chunks)
        assert "first_line" in c.metadata


def test_paragraph_splits_when_exceeding_chunk_size():
    cfg = ChunkingConfig(chunk_size=30, min_chunk_size=1, max_chunk_size=1000)
    s = ParagraphStrategy(cfg)
    text = "Alpha paragraph one.\n\nBravo paragraph two.\n\nCharlie paragraph three."
    chunks = s.chunk(text, "doc")
    # Should produce more than a single chunk because each para nearly fills size
    assert len(chunks) >= 2


def test_paragraph_oversize_paragraph_is_split():
    cfg = ChunkingConfig(chunk_size=40, min_chunk_size=1, max_chunk_size=50)
    s = ParagraphStrategy(cfg)
    big = ("Sentence one is here. " * 6).strip()  # > max_chunk_size, multi-sentence
    chunks = s.chunk(big, "doc")
    assert len(chunks) >= 2


def test_paragraph_oversize_paragraph_after_accumulated():
    cfg = ChunkingConfig(chunk_size=40, min_chunk_size=1, max_chunk_size=50)
    s = ParagraphStrategy(cfg)
    small = "Tiny intro paragraph."
    big = ("Sentence one is here. " * 6).strip()
    text = f"{small}\n\n{big}"
    chunks = s.chunk(text, "doc")
    # First the accumulated small paragraph flushes, then the big one splits
    assert len(chunks) >= 2


def test_paragraph_is_list_detection():
    s = ParagraphStrategy(ChunkingConfig())
    list_text = "- item one\n- item two\n- item three"
    assert s._is_list_paragraph(list_text) is True
    assert s._is_list_paragraph("single line") is False
    numbered = "1. first\n2. second\n3. third"
    assert s._is_list_paragraph(numbered) is True


def test_paragraph_split_oversize_paragraph_helper_short_circuits():
    s = ParagraphStrategy(ChunkingConfig())
    assert s._split_large_paragraph("small text", 1000) == ["small text"]


def test_paragraph_split_oversize_paragraph_groups_sentences():
    s = ParagraphStrategy(ChunkingConfig())
    text = "One sentence here. Two sentence here. Three sentence here."
    out = s._split_large_paragraph(text, 25)
    assert len(out) >= 2


def test_paragraph_repr():
    s = ParagraphStrategy(ChunkingConfig(chunk_size=321))
    assert "ParagraphStrategy" in repr(s)
    assert "321" in repr(s)


def test_paragraph_first_line_truncation():
    cfg = ChunkingConfig(chunk_size=500, min_chunk_size=1, max_chunk_size=1000)
    s = ParagraphStrategy(cfg)
    long_first = "z" * 80
    chunks = s.chunk(long_first, "doc")
    assert chunks
    assert chunks[0].metadata["first_line"].endswith("...")


# --------------------------------------------------------------------------- #
# semantic.py
# --------------------------------------------------------------------------- #


def test_semantic_empty_returns_empty():
    s = SemanticStrategy(ChunkingConfig())
    assert s.chunk("", "src") == []


def test_semantic_markdown_headers():
    cfg = ChunkingConfig(chunk_size=200, min_chunk_size=1, max_chunk_size=400)
    s = SemanticStrategy(cfg)
    text = (
        "Intro paragraph before any header.\n\n"
        "# Heading One\n\nContent under heading one goes here.\n\n"
        "## Heading Two\n\nContent under heading two goes here as well."
    )
    chunks = s.chunk(text, "doc")
    assert chunks
    # the leading text becomes an 'introduction' section
    assert any(c.metadata.get("section_type") == "introduction" for c in chunks)
    assert any(c.metadata.get("has_header") for c in chunks)
    for c in chunks:
        assert c.metadata["total_chunks"] == len(chunks)


def test_semantic_html_headers():
    cfg = ChunkingConfig(chunk_size=300, min_chunk_size=1, max_chunk_size=500)
    s = SemanticStrategy(cfg)
    text = "<h1>Title</h1>\n\nSome introductory content here for the html doc."
    chunks = s.chunk(text, "doc")
    assert chunks
    assert any(c.metadata.get("header_type") == "html" for c in chunks)


def test_semantic_topic_transitions_no_headers():
    cfg = ChunkingConfig(chunk_size=200, min_chunk_size=1, max_chunk_size=400)
    s = SemanticStrategy(cfg)
    text = (
        "First we introduce the subject matter here.\n\n"
        "However, there is a complication worth noting.\n\n"
        "In conclusion, everything resolves nicely."
    )
    chunks = s.chunk(text, "doc")
    assert chunks
    assert any(c.metadata.get("section_type") == "topic_based" for c in chunks)


def test_semantic_section_break_marker():
    cfg = ChunkingConfig(chunk_size=200, min_chunk_size=1, max_chunk_size=400)
    s = SemanticStrategy(cfg)
    text = "Part one content lives here.\n\n---\n\nPart two content lives here."
    chunks = s.chunk(text, "doc")
    assert chunks


def test_semantic_single_section_no_boundaries():
    cfg = ChunkingConfig(chunk_size=500, min_chunk_size=1, max_chunk_size=1000)
    s = SemanticStrategy(cfg)
    # No headers / breaks / transitions -> boundaries = [0, len] -> single
    # 'topic_based' section spanning the whole text.
    sections = s._identify_topic_sections("just one continuous block of text")
    assert len(sections) == 1
    assert sections[0][3]["section_type"] == "topic_based"


def test_semantic_topic_sections_empty_text_fallback():
    s = SemanticStrategy(ChunkingConfig())
    # Whitespace-only text strips to nothing for every boundary slice, so the
    # 'single' fallback branch is taken.
    sections = s._identify_topic_sections("   ")
    assert len(sections) == 1
    assert sections[0][3]["section_type"] == "single"


def test_semantic_preserve_and_restore_code_blocks():
    s = SemanticStrategy(ChunkingConfig())
    text = "Before.\n\n```python\nprint('hi')\n```\n\nAfter."
    stripped, blocks = s._preserve_special_blocks(text)
    assert blocks and blocks[0]["type"] == "code"
    assert "<<CODE_BLOCK_0>>" in stripped
    restored = s._restore_special_blocks(stripped, blocks)
    assert "print('hi')" in restored


def test_semantic_preserve_table_block():
    s = SemanticStrategy(ChunkingConfig())
    text = "Intro.\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nOutro."
    stripped, blocks = s._preserve_special_blocks(text)
    assert any(b["type"] == "table" for b in blocks)


def test_semantic_oversize_section_split():
    cfg = ChunkingConfig(chunk_size=40, min_chunk_size=1, max_chunk_size=60)
    s = SemanticStrategy(cfg)
    # A header followed by a long multi-paragraph body to force _split_large_section
    body = "\n\n".join(f"Paragraph number {i} with some text." for i in range(6))
    text = f"# Big Section\n\n{body}"
    chunks = s.chunk(text, "doc")
    assert len(chunks) >= 2
    assert any(c.metadata.get("chunk_type") == "semantic_split" for c in chunks)


def test_semantic_code_blocks_disabled():
    cfg = ChunkingConfig(
        chunk_size=300,
        min_chunk_size=1,
        max_chunk_size=500,
        preserve_code_blocks=False,
        preserve_tables=False,
    )
    s = SemanticStrategy(cfg)
    chunks = s.chunk("Plain text without special handling enabled.", "doc")
    assert chunks


def test_semantic_repr():
    s = SemanticStrategy(ChunkingConfig(chunk_size=64))
    assert "SemanticStrategy" in repr(s)
    assert "64" in repr(s)


# --------------------------------------------------------------------------- #
# recursive.py
# --------------------------------------------------------------------------- #


def test_recursive_empty_returns_empty():
    s = RecursiveStrategy(ChunkingConfig())
    assert s.chunk("", "src") == []


def test_recursive_acceptable_paragraph_chunks():
    cfg = ChunkingConfig(chunk_size=200, min_chunk_size=1, max_chunk_size=400)
    s = RecursiveStrategy(cfg)
    text = "Para one here.\n\nPara two here.\n\nPara three here."
    chunks = s.chunk(text, "doc")
    assert chunks
    for c in chunks:
        assert c.metadata["chunk_type"] == "recursive"
        assert c.metadata["total_chunks"] == len(chunks)
        assert c.chunk_id.startswith("doc_chunk_")
    # at least one stayed at level 1 (paragraph)
    assert any(c.metadata.get("recursive_level") == 1 for c in chunks)


def test_recursive_split_sentence_level_direct():
    # The chunk() path can't surface an oversized-but-sentence-splittable
    # paragraph (paragraph strategy pre-splits those), so exercise the
    # level-2 sentence descent directly. Each sentence fits under max so they
    # stay at 'sentence' level and inherit the start_pos offset.
    cfg = ChunkingConfig(
        chunk_size=40, chunk_overlap=5, min_chunk_size=1, max_chunk_size=40
    )
    s = RecursiveStrategy(cfg)
    text = "Alpha one here. Bravo two here. Gamma three here."
    out = s._recursive_split(
        text, 100, "doc", 0, {}, parent_chunk_id="doc_chunk_0", level=2
    )
    assert out
    assert all(c.metadata.get("strategy_used") == "sentence" for c in out)
    assert all(c.metadata.get("parent_chunk") == "doc_chunk_0" for c in out)
    # start_pos is offset by the parent start (100)
    assert out[0].start_pos >= 100


def test_recursive_split_wrong_level_returns_empty():
    s = RecursiveStrategy(ChunkingConfig())
    # level != 2 short-circuits to an empty list
    assert s._recursive_split("x", 0, "doc", 0, {}, "p", level=3) == []


def test_recursive_descends_to_sliding_window_via_chunk():
    # A long single paragraph with NO sentence endings can't be split by the
    # paragraph or sentence strategies, so chunk() falls through to the
    # sliding-window level with forced_split metadata.
    cfg = ChunkingConfig(
        chunk_size=30, chunk_overlap=5, min_chunk_size=1, max_chunk_size=30
    )
    s = RecursiveStrategy(cfg)
    text = ("word " * 30).strip()
    chunks = s.chunk(text, "doc")
    assert chunks
    assert any(c.metadata.get("strategy_used") == "sliding_window" for c in chunks)
    assert any(c.metadata.get("forced_split") for c in chunks)


def test_recursive_repr():
    s = RecursiveStrategy(ChunkingConfig(chunk_size=11, max_chunk_size=99))
    r = repr(s)
    assert "RecursiveStrategy" in r
    assert "11" in r and "99" in r


# --------------------------------------------------------------------------- #
# factory.py
# --------------------------------------------------------------------------- #


@pytest.mark.parametrize(
    "strategy",
    [
        ChunkingStrategy.SLIDING_WINDOW,
        ChunkingStrategy.SENTENCE,
        ChunkingStrategy.PARAGRAPH,
        ChunkingStrategy.SEMANTIC,
        ChunkingStrategy.RECURSIVE,
        ChunkingStrategy.FIXED_SIZE,
    ],
)
def test_factory_create_each_strategy_default_config(strategy):
    inst = ChunkingStrategyFactory.create_strategy(strategy)
    assert isinstance(inst, ChunkingStrategyInterface)
    assert inst.config.strategy == strategy


def test_factory_create_code_strategy_default_config():
    # CODE uses CodeChunkingConfig path; constructor falls back to regex parser
    inst = ChunkingStrategyFactory.create_strategy(ChunkingStrategy.CODE)
    assert isinstance(inst, ChunkingStrategyInterface)


def test_factory_create_with_explicit_config_sets_strategy():
    cfg = ChunkingConfig(strategy=ChunkingStrategy.SLIDING_WINDOW)
    inst = ChunkingStrategyFactory.create_strategy(ChunkingStrategy.PARAGRAPH, cfg)
    assert isinstance(inst, ParagraphStrategy)
    assert cfg.strategy == ChunkingStrategy.PARAGRAPH  # mutated to match


def test_factory_code_strategy_converts_base_config():
    cfg = ChunkingConfig(chunk_size=256, chunk_overlap=16)
    inst = ChunkingStrategyFactory.create_strategy(ChunkingStrategy.CODE, cfg)
    # config got converted to a CodeChunkingConfig carrying base values
    assert inst.config.chunk_size == 256
    assert inst.config.chunk_overlap == 16


def test_factory_unknown_strategy_raises():
    class _Fake:
        pass

    with pytest.raises(ValueError, match="Unknown chunking strategy"):
        ChunkingStrategyFactory.create_strategy(_Fake())  # type: ignore[arg-type]


def test_factory_register_custom_strategy():
    class CustomStrategy(SlidingLike := ParagraphStrategy):  # noqa: N801
        pass

    # Register under an existing enum to avoid mutating the enum class itself,
    # then restore so we don't leak state into other tests.
    original = ChunkingStrategyFactory._strategies[ChunkingStrategy.FIXED_SIZE]
    try:
        ChunkingStrategyFactory.register_strategy(
            ChunkingStrategy.FIXED_SIZE, CustomStrategy
        )
        inst = ChunkingStrategyFactory.create_strategy(ChunkingStrategy.FIXED_SIZE)
        assert isinstance(inst, CustomStrategy)
    finally:
        ChunkingStrategyFactory._strategies[ChunkingStrategy.FIXED_SIZE] = original


def test_factory_list_strategies():
    names = ChunkingStrategyFactory.list_strategies()
    assert "paragraph" in names
    assert "semantic" in names
    assert "semantic_embedding" in names
    assert "code" in names
    assert len(names) == 8


def test_get_chunking_strategy_by_string():
    inst = get_chunking_strategy("semantic", chunk_size=128, chunk_overlap=12)
    assert isinstance(inst, SemanticStrategy)
    assert inst.config.chunk_size == 128
    assert inst.config.strategy == ChunkingStrategy.SEMANTIC


def test_get_chunking_strategy_by_enum_default():
    inst = get_chunking_strategy(ChunkingStrategy.PARAGRAPH)
    assert isinstance(inst, ParagraphStrategy)


def test_get_chunking_strategy_invalid_string_raises():
    with pytest.raises(ValueError):
        get_chunking_strategy("not_a_real_strategy")


def test_get_chunking_strategy_end_to_end_chunks_text():
    chunker = get_chunking_strategy(
        "paragraph", chunk_size=200, chunk_overlap=20, min_chunk_size=1
    )
    chunks = chunker.chunk("Hello there.\n\nGeneral Kenobi.", "doc")
    assert chunks
    assert all(isinstance(c, TextChunk) for c in chunks)
