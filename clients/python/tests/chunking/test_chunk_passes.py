"""TD-CHUNK-3 items 4/5 + the pass seam that unifies all four capabilities.

Written against the *claims* rather than the execution, per the lesson TD-CG2
recorded: a suite that asserts a function ran cannot see that it ran wrong.
"""

from __future__ import annotations

import pytest

from proximadb_sdk.chunking_strategies.base import TextChunk
from proximadb_sdk.chunking_strategies.capabilities import (
    ContextEnrichmentPass,
    DedupPass,
    HeadingPathPass,
    ParentLinkagePass,
    PassPipeline,
    structural_context,
)
from proximadb_sdk.chunking_strategies.passes import (
    CHUNK_TYPE_PARENT,
    EMBEDDED_TEXT_KEY,
    FACE_ORDER,
    PARENT_ID_KEY,
    Face,
    PassResult,
    embedded_text_of,
    run_passes,
)

DOC = (
    "# Guide\n\n"
    "Intro paragraph about the guide.\n\n"
    "## Install\n\n"
    "Run the installer and wait.\n\n"
    "Then verify the installation worked.\n\n"
    "## Usage\n\n"
    "Call the entry point with a config.\n\n"
    "Read the output carefully.\n"
)


def chunks_of(document: str, size: int = 40) -> list[TextChunk]:
    """A cheap total partition: consecutive spans, exact offsets."""
    out: list[TextChunk] = []
    pos = 0
    index = 0
    while pos < len(document):
        end = min(pos + size, len(document))
        out.append(
            TextChunk(
                text=document[pos:end],
                start_pos=pos,
                end_pos=end,
                chunk_id=f"d_{index}",
                metadata={"source_id": "d"},
            )
        )
        pos = end
        index += 1
    return out


# --- the seam ---------------------------------------------------------------


def test_face_order_is_the_run_order_not_the_config_order():
    """Configuration order must not be able to buy a saving at quality's expense.

    Reversing METADATA and SELECTION is the specific mistake: a survivor that
    absorbed duplicates loses the heading path it would have been labelled with.
    """
    seen: list[str] = []

    def recorder(name: str, face: Face):
        class P:
            def __init__(self):
                self.name = name
                self.face = face

            def apply(self, document, chunks):
                seen.append(self.name)
                return PassResult(chunks=chunks)

        return P()

    run_passes(
        DOC,
        chunks_of(DOC),
        [
            recorder("d", Face.RETRIEVED),
            recorder("c", Face.EMBEDDED),
            recorder("b", Face.SELECTION),
            recorder("a", Face.METADATA),
        ],
    )
    assert seen == ["a", "b", "c", "d"]


@pytest.mark.parametrize(
    "face,sabotage",
    [
        (Face.METADATA, "drop"),
        (Face.METADATA, "edit"),
        (Face.SELECTION, "add"),
        (Face.SELECTION, "edit"),
        (Face.EMBEDDED, "drop"),
        (Face.EMBEDDED, "edit"),
        (Face.RETRIEVED, "drop"),
        (Face.RETRIEVED, "edit"),
    ],
)
def test_a_pass_that_exceeds_its_face_fails_loudly(face, sabotage):
    """The guard's teeth. Without these the declaration is decoration.

    A METADATA pass that silently drops chunks is deleting tenant content, and
    its own tests would never catch it -- they assert what it means to do.
    """

    class Rogue:
        name = "rogue"

        def __init__(self):
            self.face = face

        def apply(self, document, chunks):
            out = list(chunks)
            if sabotage == "drop":
                out = out[:-1]
            elif sabotage == "add":
                out.append(
                    TextChunk(text="x", start_pos=0, end_pos=1, chunk_id="extra")
                )
            elif sabotage == "edit":
                out[0] = TextChunk(
                    text="TAMPERED",
                    start_pos=out[0].start_pos,
                    end_pos=out[0].end_pos,
                    chunk_id=out[0].chunk_id,
                )
            return PassResult(chunks=out)

    with pytest.raises(ValueError, match="face"):
        run_passes(DOC, chunks_of(DOC), [Rogue()])


def test_metadata_pass_may_write_metadata():
    class Tagger:
        name = "tag"
        face = Face.METADATA

        def apply(self, document, chunks):
            for c in chunks:
                c.metadata["tagged"] = True
            return PassResult(chunks=chunks)

    result = run_passes(DOC, chunks_of(DOC), [Tagger()])
    assert all(c.metadata["tagged"] for c in result.chunks)


# --- the KEU instrument -----------------------------------------------------


def test_enrichment_tax_is_one_when_nothing_enriches():
    result = run_passes(DOC, chunks_of(DOC), [HeadingPathPass()])
    assert result.enrichment_tax == pytest.approx(1.0)
    assert result.embedded_chars == result.span_chars


def test_enrichment_tax_reports_the_keu_multiplier():
    """The number TD-CHUNK-3 requires enrichment to be gated on.

    Before this seam the pipeline had one text, so "what does enrichment cost"
    could not be asked, only estimated from the template.
    """
    result = run_passes(
        DOC,
        chunks_of(DOC),
        [HeadingPathPass(), ContextEnrichmentPass(source_title="Guide")],
    )
    assert result.enrichment_tax > 1.0
    assert result.embedded_chars > result.span_chars
    # And the span side is untouched: enrichment must not change what is stored.
    plain = run_passes(DOC, chunks_of(DOC), [HeadingPathPass()])
    assert result.span_chars == plain.span_chars


def test_parents_are_excluded_from_the_embedding_bill():
    """A parent is RETURNED, not embedded. Counting it would double-bill KEU."""
    result = run_passes(
        DOC, chunks_of(DOC), [HeadingPathPass(), ParentLinkagePass(parent_window=200)]
    )
    assert any(c.metadata.get("chunk_type") == CHUNK_TYPE_PARENT for c in result.chunks)
    plain = run_passes(DOC, chunks_of(DOC), [HeadingPathPass()])
    assert result.embedded_chars == plain.embedded_chars


# --- context enrichment (item 4) --------------------------------------------


def test_enrichment_changes_the_vector_input_not_the_chunk():
    chunks = chunks_of(DOC)
    before = [(c.text, c.start_pos, c.end_pos) for c in chunks]
    run_passes(DOC, chunks, [HeadingPathPass(), ContextEnrichmentPass()])
    assert [(c.text, c.start_pos, c.end_pos) for c in chunks] == before
    enriched = [c for c in chunks if EMBEDDED_TEXT_KEY in c.metadata]
    assert enriched, "no chunk was enriched at all"
    for chunk in enriched:
        assert embedded_text_of(chunk).endswith(chunk.text)
        assert embedded_text_of(chunk) != chunk.text


def test_enrichment_consumes_what_the_metadata_pass_wrote():
    """The composition the face order exists to make possible.

    Without HeadingPathPass first there is no heading path to prefix, so this
    is the direct test that METADATA-before-EMBEDDED is load-bearing rather
    than cosmetic.
    """
    with_headings = chunks_of(DOC)
    run_passes(DOC, with_headings, [HeadingPathPass(), ContextEnrichmentPass()])

    without = chunks_of(DOC)
    run_passes(DOC, without, [ContextEnrichmentPass()])

    assert sum(EMBEDDED_TEXT_KEY in c.metadata for c in with_headings) > sum(
        EMBEDDED_TEXT_KEY in c.metadata for c in without
    )


def test_enrichment_skips_chunks_it_would_dominate():
    """A vector that is mostly section title retrieves the section, not the chunk."""
    tiny = TextChunk(
        text="ok",
        start_pos=0,
        end_pos=2,
        chunk_id="t",
        metadata={"heading_path": "A very long heading path indeed > and deeper still"},
    )
    result = run_passes("ok", [tiny], [ContextEnrichmentPass()])
    assert EMBEDDED_TEXT_KEY not in tiny.metadata
    assert result.stats["context_enrichment"]["skipped_context_ratio"] == 1


def test_structural_context_is_empty_without_structure():
    bare = TextChunk(text="x", start_pos=0, end_pos=1, chunk_id="b")
    assert structural_context(bare) == ""


# --- small-to-big (item 5) --------------------------------------------------


def test_parent_contains_every_child_span_exactly():
    chunks = chunks_of(DOC)
    result = run_passes(
        DOC, chunks, [HeadingPathPass(), ParentLinkagePass(parent_window=200)]
    )
    parents = {
        c.chunk_id: c
        for c in result.chunks
        if c.metadata.get("chunk_type") == CHUNK_TYPE_PARENT
    }
    assert parents
    linked = 0
    for child in result.chunks:
        parent_id = child.metadata.get(PARENT_ID_KEY)
        if not parent_id:
            continue
        linked += 1
        parent = parents[parent_id]
        assert parent.start_pos <= child.start_pos
        assert child.end_pos <= parent.end_pos
    assert linked


def test_parent_text_is_a_real_span_of_the_document():
    """Axiom 2 holds for parents too: derived text, never rejoined pieces."""
    chunks = chunks_of(DOC)
    result = run_passes(DOC, chunks, [ParentLinkagePass(parent_window=200)])
    for parent in result.chunks:
        if parent.metadata.get("chunk_type") != CHUNK_TYPE_PARENT:
            continue
        assert DOC[parent.start_pos : parent.end_pos] == parent.text


def test_parent_text_is_not_copied_onto_children():
    """The KSU argument. Copying is the obvious implementation and is ~4x wrong."""
    chunks = chunks_of(DOC)
    result = run_passes(DOC, chunks, [ParentLinkagePass(parent_window=200)])
    for child in result.chunks:
        if child.metadata.get("chunk_type") == CHUNK_TYPE_PARENT:
            continue
        assert "parent_text" not in child.metadata
        assert isinstance(child.metadata.get(PARENT_ID_KEY, ""), str)


def test_edges_are_emitted_for_every_link():
    chunks = chunks_of(DOC)
    result = run_passes(DOC, chunks, [ParentLinkagePass(parent_window=200)])
    linked = [c for c in result.chunks if c.metadata.get(PARENT_ID_KEY)]
    assert len(result.edges) == len(linked)
    assert {e.relation for e in result.edges} == {"parent_of"}
    ids = {c.chunk_id for c in result.chunks}
    for edge in result.edges:
        assert edge.source_id in ids and edge.target_id in ids


def test_a_parent_never_spans_two_sections():
    chunks = chunks_of(DOC, size=30)
    result = run_passes(
        DOC,
        chunks,
        [HeadingPathPass(), ParentLinkagePass(parent_window=10_000)],
    )
    parents = [
        c for c in result.chunks if c.metadata.get("chunk_type") == CHUNK_TYPE_PARENT
    ]
    # A 10k window would swallow the whole document if headings were ignored.
    assert len(parents) > 1


def test_a_lone_child_gets_no_parent():
    """A parent identical to its only child doubles storage to add nothing."""
    single = chunks_of(DOC, size=len(DOC))
    result = run_passes(DOC, single, [ParentLinkagePass()])
    assert not [
        c for c in result.chunks if c.metadata.get("chunk_type") == CHUNK_TYPE_PARENT
    ]
    assert not result.edges


# --- pipeline assembly ------------------------------------------------------


def test_pipeline_from_config_selects_only_what_is_enabled():
    class Config:
        preserve_structure = True
        deduplicate_chunks = False
        enrich_context = True
        link_parent_chunks = False

    pipeline = PassPipeline.from_processor_config(Config())
    assert [p.name for p in pipeline.passes] == ["heading_path", "context_enrichment"]


def test_all_four_compose():
    class Config:
        preserve_structure = True
        deduplicate_chunks = True
        deduplicate_threshold = 0.9
        enrich_context = True
        link_parent_chunks = True
        parent_window = 200

    pipeline = PassPipeline.from_processor_config(Config(), source_title="Guide")
    result = pipeline.run(DOC, chunks_of(DOC))
    assert [f.value for f in FACE_ORDER] == [
        "metadata",
        "selection",
        "embedded",
        "retrieved",
    ]
    assert set(result.stats) == {
        "heading_path",
        "dedup",
        "context_enrichment",
        "parent_linkage",
    }
    assert result.enrichment_tax > 1.0


def test_dedup_pass_matches_the_function_it_wraps():
    """The retrofit must be the same behaviour, not a reimplementation."""
    from proximadb_sdk.chunking_strategies.dedup import deduplicate

    doc = "Alpha beta gamma delta.\n\n" * 6
    chunks = chunks_of(doc, size=25)
    direct = deduplicate(list(chunks), threshold=0.9).kept
    viaPass = run_passes(doc, list(chunks), [DedupPass(threshold=0.9)]).chunks
    assert [c.chunk_id for c in direct] == [c.chunk_id for c in viaPass]
