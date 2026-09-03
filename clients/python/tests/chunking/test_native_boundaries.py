"""TD-CHUNK-3 item 3: the native sentence splitter, behind TD-CHUNK-2's port.

Tests the capability CLAIM ("implemented and better"), not the wiring. The
claim is half true, and these record which half.
"""

from __future__ import annotations

import pytest

from proximadb_sdk.chunking_strategies.base import ChunkingConfig, ChunkingStrategy
from proximadb_sdk.chunking_strategies.boundaries import BoundaryKind
from proximadb_sdk.chunking_strategies.native_boundaries import (
    NativeSentenceBoundarySource,
    native_sentences_available,
)
from proximadb_sdk.chunking_strategies.sentence import SentenceStrategy

pytestmark = pytest.mark.skipif(
    not native_sentences_available(),
    reason="victor_native is optional; the Python sentence source is the fallback",
)

#: Every cut here is an abbreviation trap.
ABBREVIATIONS = (
    "Dr. Smith met Mr. Lee at 3 p.m. on Jan. 5 in Washington D.C. "
    "It was cold. See fig. 2, i.e. the chart, e.g. panel A. Prof. Chan agreed."
)


def _python_cut_texts(text: str) -> list[str]:
    strategy = SentenceStrategy(
        ChunkingConfig(
            strategy=ChunkingStrategy.SENTENCE,
            chunk_size=40,
            chunk_overlap=0,
            min_chunk_size=1,
            max_chunk_size=400,
        )
    )
    return [c.text for c in strategy.chunk(text, "t")]


def _native_cut_texts(text: str) -> list[str]:
    out, prev = [], 0
    for boundary in NativeSentenceBoundarySource().boundaries(text):
        out.append(text[prev : boundary.end])
        prev = boundary.end
    out.append(text[prev:])
    return out


def _broken_sentences(pieces: list[str]) -> int:
    """Cuts that land mid-sentence, judged by what FOLLOWS the cut.

    The first metric here was a list of known abbreviations, and it could not
    tell a broken cut from a correct one: `Washington D.C. | It was cold` ends
    on an abbreviation and is right, while `on Jan. | 5 in` ends on one and is
    wrong. Both scored the same, so both splitters tied at 2 and the comparison
    said nothing.

    A sentence never begins with a digit or a lowercase letter, in any of the
    languages either splitter claims. So judge the cut by its successor -- a
    rule rather than a table, which is also why it does not need maintaining.
    """
    broken = 0
    for following in pieces[1:]:
        head = following.strip()
        if head and (head[0].islower() or head[0].isdigit()):
            broken += 1
    return broken


# --- axiom 2: what leaves this module must be spans ------------------------


def test_boundaries_are_offsets_into_the_source_not_derived_lengths():
    """The defect this wrapper exists to contain.

    The native call returns stripped text PIECES; rejoining them loses the
    separators, so a cursor advanced by piece length drifts. Offsets are
    recovered by scanning, so slicing the source reproduces the document
    exactly.
    """
    boundaries = NativeSentenceBoundarySource().boundaries(ABBREVIATIONS)
    assert boundaries
    prev = 0
    rebuilt = ""
    for boundary in boundaries:
        assert boundary.end > prev, "boundaries must be strictly increasing"
        rebuilt += ABBREVIATIONS[prev : boundary.end]
        prev = boundary.end
    rebuilt += ABBREVIATIONS[prev:]
    assert rebuilt == ABBREVIATIONS


def test_every_boundary_is_inside_the_document():
    boundaries = NativeSentenceBoundarySource().boundaries(ABBREVIATIONS)
    for boundary in boundaries:
        assert 0 < boundary.end < len(ABBREVIATIONS)
        assert boundary.kind is BoundaryKind.SENTENCE


def test_empty_and_missing_inputs_propose_nothing():
    assert NativeSentenceBoundarySource().boundaries("") == ()


# --- the capability claim, measured ----------------------------------------


def test_native_breaks_fewer_sentences_than_the_python_source():
    """Better, and the test says by how much rather than that it is better.

    Recorded as an inequality with both numbers asserted, so an upstream
    regression in either direction is visible instead of being absorbed.
    """
    python_broken = _broken_sentences(_python_cut_texts(ABBREVIATIONS))
    native_broken = _broken_sentences(_native_cut_texts(ABBREVIATIONS))
    assert native_broken < python_broken
    assert (native_broken, python_broken) == (1, 2)


def test_native_is_not_claimed_to_be_perfect():
    """`See fig. | 2` is still cut. Pinned so the docs cannot drift optimistic.

    A capability recorded as "fixed" when it is "improved" is how this
    program's census entries went stale in the first place.
    """
    pieces = _native_cut_texts(ABBREVIATIONS)
    assert any(p.strip().endswith("fig.") for p in pieces[:-1])


# --- the upstream defect this port makes unreachable ------------------------


def test_the_overlap_default_is_a_keu_multiplier_we_never_expose():
    """`overlap` defaults to 128 and is not clamped against `chunk_size`.

    At chunk_size=40 the native call emits cumulative prefixes: 619 characters
    out of a 143-character input. A boundary source has no budget to pass, so
    the defect is unreachable through this port -- which is the concrete
    payoff of TD-CHUNK-2 separating proposal from budget, rather than an
    abstract one.
    """
    import victor_native

    text = ABBREVIATIONS[:143]
    defaulted = victor_native.chunk_by_sentences(text, 40, 128)
    assert sum(len(c) for c in defaulted) > 3 * len(text)

    # Through the port: every character accounted for exactly once.
    boundaries = NativeSentenceBoundarySource().boundaries(text)
    prev, total = 0, 0
    for boundary in boundaries:
        total += boundary.end - prev
        prev = boundary.end
    total += len(text) - prev
    assert total == len(text)


def test_source_composes_with_the_other_sources():
    from proximadb_sdk.chunking_strategies.boundaries import (
        CompositeBoundarySource,
        HeadingBoundarySource,
    )

    document = "# Title\n\nDr. Lee arrived. It was late.\n\n## Next\n\nHe left.\n"
    composite = CompositeBoundarySource(
        HeadingBoundarySource(), NativeSentenceBoundarySource()
    )
    boundaries = composite.boundaries(document)
    assert boundaries
    kinds = {b.kind for b in boundaries}
    assert BoundaryKind.SENTENCE in kinds
    assert all(0 < b.end <= len(document) for b in boundaries)
