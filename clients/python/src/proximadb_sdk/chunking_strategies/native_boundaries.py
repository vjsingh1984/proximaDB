"""A native sentence boundary source, behind the port (TD-CHUNK-3 item 3).

The census recorded an abbreviation-aware sentence splitter in ``victor_native``
as "genuinely implemented, never called", with five Python regexes doing the job
worse. Measured 2026-08-19, that is right about the *direction* and wrong about
the *shape*, and both corrections are why this is a boundary source rather than
a drop-in chunker.

Measured, not assumed
---------------------
On ``Dr. Smith met Mr. Lee at 3 p.m. on Jan. 5 in Washington D.C. It was cold.
See fig. 2, i.e. the chart, e.g. panel A. Prof. Chan agreed.``:

* the Python path cuts inside ``Jan. | 5`` and inside ``e.g. | panel`` -- two
  abbreviations broken;
* the native path holds ``Jan. 5``, ``D.C.``, ``p.m.``, ``e.g.`` and ``Prof.``,
  and breaks one: ``See fig. | 2``.

So it is better, not perfect -- one error against two or three -- and the honest
claim is a reduction, not a fix. Throughput is not the argument either way
(60 KB in ~1 ms), though it does remove the quadratic Python path.

Two defects found on the way, which is why this wraps rather than exposes it
-----------------------------------------------------------------------------
* **It returns text pieces, not offsets.** That is the rejoin-versus-span
  problem ADR-091 axiom 2 exists to remove: pieces are stripped, so 3 of 143
  characters vanish between them and no span can describe the result. This
  module recovers offsets by scanning forward from a cursor, so what leaves
  here is spans into the original document and the axiom holds.
* **``overlap`` defaults to 128 and is not clamped against ``chunk_size``.**
  With ``chunk_size=40, overlap=128`` a 143-character input yields 619
  characters across 7 cumulative-prefix chunks -- a 4.3x KEU multiplier from
  accepting the defaults. This module never passes an overlap: a boundary
  source proposes cuts and has no business with a budget, which is exactly the
  separation TD-CHUNK-2 made and exactly what makes the defect unreachable
  from here.

Absent the package, the source reports itself unavailable and callers fall back
to the existing Python sentence source. Optional, not required.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from typing import Any

from .boundaries import Boundary, BoundaryKind

try:  # pragma: no cover - availability depends on the optional package
    import victor_native as _victor_native
except Exception:  # ImportError, or a native load failure
    _victor_native = None


def native_sentences_available() -> bool:
    """True when the native splitter can be used."""
    return _victor_native is not None and hasattr(_victor_native, "chunk_by_sentences")


def _sentence_pieces(text: str) -> list[str]:
    """One piece per sentence.

    ``chunk_size=1`` forces the packer to close after every sentence, so what
    comes back is the splitter's own boundaries rather than its packing
    decisions -- the grid, with the budget removed. ``overlap=0`` is passed
    explicitly, never defaulted: see the module docstring.
    """
    return list(_victor_native.chunk_by_sentences(text, 1, 0))


@dataclass
class NativeSentenceBoundarySource:
    """Sentence ends proposed by the native splitter, as spans of ``text``.

    A :class:`~.boundaries.BoundarySource`, so it composes with the heading and
    paragraph sources and is consumed by the same segmenter. Swapping the
    sentence grid is a backend change, not a rewrite -- which is the property
    TD-CHUNK-2's port was created to provide, demonstrated here by its first
    non-Python occupant.
    """

    name: str = "native_sentence"

    def boundaries(
        self,
        text: str,
        *,
        source_id: str = "doc",
        base_metadata: dict[str, Any] | None = None,
    ) -> Sequence[Boundary]:
        if not text or not native_sentences_available():
            return ()

        out: list[Boundary] = []
        cursor = 0
        for piece in _sentence_pieces(text):
            piece = piece.strip()
            if not piece:
                continue
            found = text.find(piece, cursor)
            if found < 0:
                # The splitter normalised something, so this piece is not a
                # substring of the source and no span can describe it. Skip it
                # rather than emit an offset derived from a length -- that
                # derivation is the original defect.
                continue
            cursor = found + len(piece)
            out.append(Boundary(end=cursor, kind=BoundaryKind.SENTENCE))

        # A trailing boundary at the document end is the segmenter's business,
        # not a proposal about structure.
        return tuple(b for b in out if 0 < b.end < len(text)) or tuple(out[:-1])
