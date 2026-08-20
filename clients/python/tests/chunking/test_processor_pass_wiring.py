"""The passes must reach the SDK's only production text-ingest path.

This census's central finding was capabilities built and never called. A pass
that writes `embedded_text` while the pipeline embeds `text` anyway would
reproduce exactly that -- and every unit test of the pass would still pass. So
the wiring is asserted through `TextDocumentProcessor` end to end, not through
the pass in isolation.
"""

from __future__ import annotations

from proximadb_sdk.document_processor import ProcessorConfig, TextDocumentProcessor

DOC = (
    "# Guide\n\n"
    "Intro text about the guide, long enough to be its own chunk.\n\n"
    "## Install\n\n"
    "Run the installer and then wait for it to finish completely.\n\n"
    "## Usage\n\n"
    "Call the entry point with a config file path argument here.\n"
)


def _processor(**overrides) -> TextDocumentProcessor:
    return TextDocumentProcessor(
        ProcessorConfig(
            chunk_size=60,
            chunk_overlap=0,
            min_chunk_size=1,
            max_chunk_size=240,
            **overrides,
        )
    )


def test_defaults_change_nothing_a_tenant_has_stored():
    """Only the free capability is on by default; the three costed ones are off."""
    processor = _processor()
    chunks = processor.chunk(DOC, "guide.md", {"title": "Guide"})
    assert chunks
    for chunk in chunks:
        assert processor.prepare_for_embedding(chunk) == chunk.text
        assert chunk.metadata.get("chunk_type") != "parent"
    assert processor._last_pass_stats["enrichment_tax"] == 1.0
    assert processor._last_chunk_edges == ()
    # ...and the free one still ran.
    assert any(c.metadata.get("heading_path") for c in chunks)


def test_enrichment_reaches_prepare_for_embedding():
    """The whole point. Without the wiring the vector is unchanged."""
    processor = _processor(enrich_context=True)
    chunks = processor.chunk(DOC, "guide.md", {"title": "Guide"})
    enriched = [c for c in chunks if processor.prepare_for_embedding(c) != c.text]
    assert enriched, "enrichment ran but never reached the embedding text"
    for chunk in enriched:
        prepared = processor.prepare_for_embedding(chunk)
        assert prepared.endswith(chunk.text)
        # The stored span is untouched; only the payload grew.
        assert DOC[chunk.start_pos : chunk.end_pos] == chunk.text
    assert processor._last_pass_stats["enrichment_tax"] > 1.0


def test_the_title_is_not_repeated_when_it_equals_the_h1():
    processor = _processor(enrich_context=True)
    chunks = processor.chunk(DOC, "guide.md", {"title": "Guide"})
    for chunk in chunks:
        assert "Guide > Guide" not in processor.prepare_for_embedding(chunk)


def test_parent_linkage_emits_parents_and_edges_through_the_processor():
    processor = _processor(link_parent_chunks=True)
    chunks = processor.chunk(DOC, "guide.md", {"title": "Guide"})
    parents = [c for c in chunks if c.metadata.get("chunk_type") == "parent"]
    children = [c for c in chunks if c.metadata.get("parent_id")]
    assert parents and children
    assert len(processor._last_chunk_edges) == len(children)
    for parent in parents:
        assert DOC[parent.start_pos : parent.end_pos] == parent.text


def test_stats_report_both_cost_sides():
    processor = _processor(enrich_context=True, link_parent_chunks=True)
    processor.chunk(DOC, "guide.md", {"title": "Guide"})
    stats = processor._last_pass_stats
    assert stats["span_chars"] > 0
    assert stats["embedded_chars"] >= stats["span_chars"]
    assert set(stats) >= {"heading_path", "context_enrichment", "parent_linkage"}
