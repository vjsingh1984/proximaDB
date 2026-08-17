"""Span primitives — the shared spine of span-first chunking (ADR-091 axiom 2).

    A chunk is a span; its text is derived.

Exactness (``source[start:end] == chunk.text``) admits exactly one implementation
shape, because a normalized rejoin — ``" ".join(...)``, ``"\\n\\n".join(...)`` —
is *not a substring of the source*, so no span can ever describe it. Every
offset defect in the audit traces to the same inversion: a position derived from
the length of a reconstructed string rather than tracked through the original.

So: **compute a span in the source, then slice.** Never advance a cursor by the
length of text you built.

`sliding_window.py` already works this way and is the only strategy that
survived the audit clean; `paragraph.py` does it for one case. These helpers
hoist that pattern so the other strategies converge on it instead of each
re-deriving the same off-by-one arithmetic.
"""

from __future__ import annotations

from collections.abc import Iterator

#: A half-open ``[start, end)`` range of character offsets into a source string.
Span = tuple[int, int]


def strip_span(text: str, start: int, end: int) -> Span:
    """Narrow ``[start, end)`` past leading/trailing whitespace.

    The offset-preserving replacement for ``text[start:end].strip()``. Stripping
    the *text* while keeping the unstripped bounds is the `fixed_size.py` defect:
    the chunk no longer equals its own span. Narrowing the *span* keeps them
    equal by construction.

    Returns an empty span (``start == end``) when the range is all whitespace;
    callers should skip such spans rather than emit an empty chunk.
    """
    start = max(0, min(start, len(text)))
    end = max(start, min(end, len(text)))
    while start < end and text[start].isspace():
        start += 1
    while end > start and text[end - 1].isspace():
        end -= 1
    return start, end


def is_empty(span: Span) -> bool:
    """True when the span covers nothing."""
    return span[1] <= span[0]


def merge_spans(spans: list[Span]) -> Span:
    """The hull of a contiguous group: first start to last end.

    This is what replaces ``" ".join(parts)`` — the group's text is
    ``source[merge_spans(group)]``, so the source's own separators survive
    verbatim instead of being normalised to a single space.
    """
    if not spans:
        raise ValueError("merge_spans requires at least one span")
    return spans[0][0], spans[-1][1]


def span_length(span: Span) -> int:
    return span[1] - span[0]


def hard_split(text: str, start: int, end: int, cap: int) -> Iterator[Span]:
    """Split ``[start, end)`` into spans of at most ``cap`` characters.

    The cap enforcement of last resort, for when no boundary source found a
    split point below the budget — a minified asset, a base64 blob, CJK without
    terminators. Prefers the last whitespace before the cap so the cut lands
    between words where possible, and falls back to an exact cap cut when the
    span contains no whitespace at all.

    The alternative — emitting over the cap — is worse than an ugly boundary:
    the embedding provider either rejects the call or silently truncates, and
    truncation loses the tail with no signal. Raising instead would make a legal
    document permanently unindexable.
    """
    if cap <= 0:
        raise ValueError("cap must be positive")
    start, end = strip_span(text, start, end)
    while start < end:
        if end - start <= cap:
            yield start, end
            return
        limit = start + cap
        cut = text.rfind(" ", start, limit)
        if cut <= start:
            cut = text.rfind("\n", start, limit)
        if cut <= start:
            cut = limit  # no whitespace to cut on — take the exact cap
        piece = strip_span(text, start, cut)
        if not is_empty(piece):
            yield piece
        nxt = cut
        while nxt < end and text[nxt].isspace():
            nxt += 1
        if nxt <= start:  # defensive: guarantee forward progress
            nxt = cut + 1
        start = nxt


class SpanBuffer:
    """An absolute-offset window over a growing streaming buffer.

    Streaming strategies must emit ``source[span]`` without holding the whole
    document. This keeps a suffix of the source plus the absolute offset at
    which that suffix starts, so a span computed in document coordinates can
    still be sliced after earlier text has been released.

    Generalised from the ``trim_to`` pattern already proven correct in
    `sliding_window.py`.
    """

    __slots__ = ("_buffer", "_origin")

    def __init__(self, initial: str = "", origin: int = 0) -> None:
        self._buffer = initial
        self._origin = origin

    @property
    def origin(self) -> int:
        """Absolute offset of ``buffer[0]``."""
        return self._origin

    @property
    def end(self) -> int:
        """Absolute offset one past the last buffered character."""
        return self._origin + len(self._buffer)

    @property
    def buffer(self) -> str:
        return self._buffer

    def append(self, piece: str) -> None:
        self._buffer += piece

    def slice(self, start: int, end: int) -> str:
        """Slice by ABSOLUTE offsets; raises if the range was already released."""
        if start < self._origin:
            raise ValueError(
                f"span start {start} was released (buffer origin {self._origin})"
            )
        return self._buffer[start - self._origin : end - self._origin]

    def trim_to(self, absolute_pos: int) -> None:
        """Release everything before ``absolute_pos``. Never moves backwards."""
        if absolute_pos <= self._origin:
            return
        cut = min(absolute_pos - self._origin, len(self._buffer))
        self._buffer = self._buffer[cut:]
        self._origin += cut
