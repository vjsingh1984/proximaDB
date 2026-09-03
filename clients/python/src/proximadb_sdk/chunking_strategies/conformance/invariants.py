"""The chunking invariants (TD-CHUNK-1 deliverable 1).

ADR-091's axioms, made executable. Each invariant exists because violating it is
a *defect*, not a tuning choice, and each is traceable to the axiom it enforces:

===========================  =====  ==============================================
Invariant                    Axiom  What its violation means
===========================  =====  ==============================================
``TOTALITY``                 1      content silently lost
``EXACTNESS``                2      the span does not index its own chunk
``CAP``                      3      a chunk exceeds the model budget
``NON_EMPTY``                1      a whole document silently discarded
``NO_CONTAINMENT``           1      a span emitted twice — paid for twice
``STREAM_EQUIVALENCE``       —      output depends on read granularity
``CONFIG_SAFETY``            —      a legal configuration hangs
``IDEMPOTENCE``              1/3    segmentation is not a fixed point
===========================  =====  ==============================================

Why a violation *set* rather than assertions
--------------------------------------------
Each check returns a :class:`Violation` instead of raising, so one run yields the
complete picture for a (strategy, corpus) pair. That is what makes the baseline in
``tests/chunking/test_chunking_conformance.py`` reviewable as data and lets the
ratchet move in both directions: a new violation fails, and a *fixed* violation
also fails until the baseline is updated, so nobody fixes a defect without
recording that they did.

The audited suite had 426 passing tests and caught none of these, because it
asserted execution rather than invariants. That is the failure mode this module
exists to prevent.
"""

from __future__ import annotations

import platform
import signal
from collections.abc import Callable, Iterator, Sequence
from dataclasses import dataclass
from enum import Enum
from typing import Any

from .trace import BASIS_CHAR, compute_trace


class Invariant(str, Enum):
    """The eight properties. Names are stable — baselines reference them."""

    TOTALITY = "totality"
    EXACTNESS = "exactness"
    CAP = "cap"
    NON_EMPTY = "non_empty"
    NO_CONTAINMENT = "no_containment"
    STREAM_EQUIVALENCE = "stream_equivalence"
    CONFIG_SAFETY = "config_safety"
    IDEMPOTENCE = "idempotence"


@dataclass(frozen=True)
class Violation:
    """A failed invariant, with enough detail to reproduce it by hand."""

    invariant: Invariant
    detail: str

    def __str__(self) -> str:
        return f"{self.invariant.value}: {self.detail}"


class _Timeout(Exception):
    pass


def _alarm(_signum: int, _frame: object) -> None:
    raise _Timeout


class _wall_clock_budget:
    """Bound a call in wall-clock time so a hang fails instead of hanging CI.

    POSIX-only (``SIGALRM``); a no-op elsewhere, in which case CONFIG_SAFETY
    degrades to "did not raise" rather than "did not hang". Stated plainly
    because a guard that silently does nothing is worse than no guard.
    """

    supported = platform.system() != "Windows"

    def __init__(self, seconds: int) -> None:
        self.seconds = seconds
        self._previous: Any = None

    def __enter__(self) -> _wall_clock_budget:
        if self.supported:
            self._previous = signal.signal(signal.SIGALRM, _alarm)
            signal.alarm(self.seconds)
        return self

    def __exit__(self, *_exc: object) -> None:
        if self.supported:
            signal.alarm(0)
            if self._previous is not None:
                signal.signal(signal.SIGALRM, self._previous)


def check_totality(source: str, chunks: Sequence[Any]) -> Violation | None:
    """Axiom 1 — every non-whitespace unit of the input is in some chunk."""
    trace = compute_trace(source, chunks)
    if trace.is_total:
        return None
    lost = trace.units_in - trace.units_covered
    pct = 100.0 * lost / trace.units_in if trace.units_in else 0.0
    preview = "; ".join(f"[{g.start}:{g.end}] {g.preview!r}" for g in trace.gaps[:3])
    return Violation(
        Invariant.TOTALITY,
        f"{lost}/{trace.units_in} non-whitespace units ({pct:.2f}%) in no chunk "
        f"across {trace.spans_dropped} dropped span(s); first: {preview or 'n/a'}",
    )


def check_exactness(
    source: str, chunks: Sequence[Any], *, basis: str = BASIS_CHAR
) -> Violation | None:
    """Axiom 2 — ``source[span] == chunk.text`` under the declared basis.

    Grades the failure, because the two grades demand different fixes: a
    *superset* span still contains the text so a consumer can recover it, while a
    *garbage* span does not index the chunk at all and is unrecoverable.
    """
    if basis == BASIS_CHAR:

        def slice_source(start: int, end: int) -> str:
            return source[start:end]

    else:
        encoded = source.encode("utf-8")

        def slice_source(start: int, end: int) -> str:
            return encoded[start:end].decode("utf-8", "replace")

    superset = 0
    garbage = 0
    examples: list[str] = []
    for index, chunk in enumerate(chunks):
        text = getattr(chunk, "text", "")
        start = int(getattr(chunk, "start_pos", 0))
        end = int(getattr(chunk, "end_pos", 0))
        sliced = slice_source(start, end)
        if sliced == text:
            continue
        if text and text in sliced:
            superset += 1
        else:
            garbage += 1
            if len(examples) < 3:
                examples.append(
                    f"#{index} span=({start},{end}) text={text[:40]!r} "
                    f"slice={sliced[:40]!r}"
                )
    if superset == 0 and garbage == 0:
        return None
    return Violation(
        Invariant.EXACTNESS,
        f"{garbage} garbage + {superset} superset of {len(chunks)} chunks "
        f"(basis={basis}); first garbage: {examples[0] if examples else 'none'}",
    )


def check_cap(
    chunks: Sequence[Any],
    *,
    budget: int,
    token_counter: Callable[[str], int] | None = None,
) -> Violation | None:
    """Axiom 3 — no chunk exceeds the budget.

    A chunk over the cap does not mean the cap is advisory; it means the
    partition must be refined.
    """
    measure = token_counter or len
    over = [(i, measure(getattr(c, "text", ""))) for i, c in enumerate(chunks)]
    offenders = [(i, n) for i, n in over if n > budget]
    if not offenders:
        return None
    worst = max(offenders, key=lambda pair: pair[1])
    return Violation(
        Invariant.CAP,
        f"{len(offenders)}/{len(chunks)} chunks over budget {budget}; "
        f"worst chunk #{worst[0]} = {worst[1]} ({worst[1] / budget:.1f}x)",
    )


def check_non_empty(source: str, chunks: Sequence[Any]) -> Violation | None:
    """Axiom 1 — non-empty input yields at least one chunk.

    A document below the minimum must be emitted short or rejected loudly. Zero
    chunks with no error is the worst outcome: the caller cannot tell.
    """
    if not source.strip():
        return None
    if chunks:
        return None
    return Violation(
        Invariant.NON_EMPTY,
        f"{len(source)}-char non-empty input produced 0 chunks and raised nothing",
    )


def check_no_containment(chunks: Sequence[Any]) -> Violation | None:
    """Axiom 1/5 — no span strictly inside another; a unit paid for twice."""
    spans = [
        (int(getattr(c, "start_pos", 0)), int(getattr(c, "end_pos", 0)), i)
        for i, c in enumerate(chunks)
    ]
    nested: list[str] = []
    ordered = sorted(spans)
    for idx, (start, end, original) in enumerate(ordered):
        for other_start, other_end, other in ordered[:idx]:
            strictly_larger = (other_end - other_start) > (end - start)
            if other_start <= start and end <= other_end and strictly_larger:
                nested.append(f"#{original} inside #{other}")
                break
        if len(nested) >= 3:
            break
    if not nested:
        return None
    return Violation(
        Invariant.NO_CONTAINMENT,
        f"{len(nested)}+ nested span(s) — double-embedded content: "
        + ", ".join(nested),
    )


#: Stream equivalence is a *boundary-local* property, so it is checked over a
#: bounded prefix. Feeding a 60 KB document one character at a time costs
#: O(pieces x buffer) and takes minutes on the quadratic sentence path — the
#: property is fully exercised by a few KB, and a suite nobody can afford to run
#: is a suite that stops being run.
STREAM_PREFIX_UNITS = 4096
STREAM_PIECE_SIZES = (1, 7, 512)


def check_stream_equivalence(
    source: str,
    chunk_fn: Callable[[str], Sequence[Any]],
    stream_fn: Callable[[Iterator[str]], Sequence[Any]],
    *,
    prefix_units: int = STREAM_PREFIX_UNITS,
) -> Violation | None:
    """Streaming output must not depend on read granularity.

    Piece sizes are **varied down to a single character**: the audited fixture
    used inputs where every period was followed by a space and a capital, which
    is exactly the shape that hides the sentence-carry bug. Fuzzing over piece
    boundaries found 303 mismatches the fixture could not.
    """
    source = source[:prefix_units]
    baseline = [
        (getattr(c, "text", ""), getattr(c, "start_pos", 0), getattr(c, "end_pos", 0))
        for c in chunk_fn(source)
    ]
    for size in STREAM_PIECE_SIZES:
        pieces = [source[i : i + size] for i in range(0, len(source), size)] or [""]
        try:
            streamed = [
                (
                    getattr(c, "text", ""),
                    getattr(c, "start_pos", 0),
                    getattr(c, "end_pos", 0),
                )
                for c in stream_fn(iter(pieces))
            ]
        except Exception as exc:  # noqa: BLE001 - reported, not swallowed
            return Violation(
                Invariant.STREAM_EQUIVALENCE,
                f"piece_size={size} raised {type(exc).__name__}: {exc}",
            )
        if streamed != baseline:
            first = next(
                (
                    i
                    for i, (a, b) in enumerate(zip(streamed, baseline, strict=False))
                    if a != b
                ),
                min(len(streamed), len(baseline)),
            )
            return Violation(
                Invariant.STREAM_EQUIVALENCE,
                f"piece_size={size}: {len(streamed)} vs {len(baseline)} chunks, "
                f"first divergence at #{first}",
            )
    return None


def check_config_safety(
    build_and_run: Callable[[], Sequence[Any]], *, seconds: int = 5
) -> Violation | None:
    """A legal configuration must not hang.

    victor-rag's ``_chunk_text`` infinite-loops whenever ``min_chunk_size <
    chunk_overlap``; ProximaDB normalises overlap in ``__post_init__`` so the
    state is unreachable. Encoding the property keeps it that way on both sides.
    """
    try:
        with _wall_clock_budget(seconds):
            build_and_run()
    except _Timeout:
        return Violation(
            Invariant.CONFIG_SAFETY,
            f"did not terminate within {seconds}s on a legal configuration",
        )
    except Exception:  # noqa: BLE001
        # Raising is an acceptable answer to a hostile config; hanging is not.
        return None
    return None


def check_idempotence(
    chunks: Sequence[Any], rechunk: Callable[[str], Sequence[Any]]
) -> Violation | None:
    """Re-chunking a chunk at the same budget is a fixed point."""
    for index, chunk in enumerate(chunks[:8]):
        text = getattr(chunk, "text", "")
        if not text.strip():
            continue
        again = rechunk(text)
        if len(again) > 1:
            return Violation(
                Invariant.IDEMPOTENCE,
                f"chunk #{index} ({len(text)} chars) re-split into {len(again)}",
            )
    return None


ALL_INVARIANTS: tuple[Invariant, ...] = tuple(Invariant)
