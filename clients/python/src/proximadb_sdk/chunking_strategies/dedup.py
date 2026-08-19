"""Near-duplicate chunk detection — the only item that *reduces* cost.

TD-CHUNK-3 item 2. Every other capability in that TD trades cost for quality;
this one removes spend outright. A duplicated chunk is paid for **once in
embedding (KEU) and forever in storage (KSU)**, and it crowds a diverse result
out of every retrieval that matches it.

Real corpora are full of them: connector-sourced documents (Confluence,
SharePoint, Drive) repeat boilerplate headers, footers, legal notices and
templated sections across every page, and anvaiops ingests exactly those through
20+ connectors.

Why this is lexical and not semantic
------------------------------------
"Topic-aware" invites an embedding-based design. That would defeat the purpose.
**To skip paying KEU for a chunk you must decide to skip it BEFORE embedding
it** — a detector that needs embeddings has already spent the money it exists to
save. So detection is lexical, runs on the raw text, and costs no model call.

It is also the safer half of the problem. Lexical near-duplicates are copies:
boilerplate, repeated sections, templated text. Semantic near-duplicates include
*paraphrases*, and two paragraphs that say a similar thing in different words are
very often both worth keeping. Dropping those is a silent content loss, which is
the failure this whole program exists to remove. Paraphrase-level merging, if it
is ever wanted, is a separate decision with a separate rubric.

The partition stays total
-------------------------
ADR-091 axiom 1 says chunking is a total partition. Dedup deliberately removes
chunks, which looks like a direct contradiction and is not: dedup is a
**selection over** the partition, applied after segmentation and before
embedding. The segmentation is still total; what changes is which chunks are
*materialised*. Every removal is recorded on the representative that absorbed it
(``duplicate_spans``), so the map from document to stored content stays complete
and reversible — nothing vanishes without a trace, which is what makes this a
cost optimisation rather than data loss.
"""

from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass, field
from typing import Any

_WORD = re.compile(r"\w+", re.UNICODE)

#: Number of MinHash permutations. 128 is the usual accuracy/cost point: the
#: expected error on an estimated Jaccard is ~1/sqrt(128) ≈ 0.09, which is fine
#: for *candidate generation* because every candidate is then verified exactly.
SIGNATURE_SIZE = 128

#: Rows per band. Bands x rows must equal SIGNATURE_SIZE. With 32 bands of 4,
#: a pair at Jaccard 0.9 is a candidate with probability 1-(1-0.9^4)^32 ≈ 1.0,
#: while a pair at 0.5 is one with probability ≈ 0.87 — deliberately generous,
#: since a false candidate costs one exact comparison and a missed one costs a
#: duplicate we keep paying for.
BAND_ROWS = 4


def _stable_hash(value: str) -> int:
    """A hash that is identical across processes.

    Python's built-in ``hash()`` for ``str`` is salted per process
    (``PYTHONHASHSEED``), so using it here would make dedup — and therefore the
    chunk count, and therefore the bill — differ between two runs on identical
    input. Determinism is not optional for something that decides what gets
    stored.
    """
    return int.from_bytes(
        hashlib.blake2b(value.encode("utf-8"), digest_size=8).digest(), "big"
    )


def shingles(text: str, size: int = 5) -> frozenset[int]:
    """Hashed word n-grams — the set whose overlap defines similarity.

    Word-level rather than character-level: character shingles call two chunks
    similar because they share common substrings, which over-triggers on prose.
    Short texts fall back to their whole word list so they are still comparable
    rather than silently empty.
    """
    words = _WORD.findall(text.lower())
    if not words:
        return frozenset()
    if len(words) <= size:
        return frozenset({_stable_hash(" ".join(words))})
    return frozenset(
        _stable_hash(" ".join(words[i : i + size]))
        for i in range(len(words) - size + 1)
    )


def jaccard(left: frozenset[int], right: frozenset[int]) -> float:
    """Exact Jaccard similarity. The authority; MinHash only nominates."""
    if not left and not right:
        return 1.0
    if not left or not right:
        return 0.0
    return len(left & right) / len(left | right)


def _signature(shingle_set: frozenset[int]) -> tuple[int, ...]:
    """MinHash signature: the minimum of each of SIGNATURE_SIZE permutations."""
    if not shingle_set:
        return tuple([0] * SIGNATURE_SIZE)
    return tuple(
        min((value * (2 * i + 1) + i) & 0xFFFFFFFFFFFFFFFF for value in shingle_set)
        for i in range(SIGNATURE_SIZE)
    )


def _candidate_pairs(signatures: list[tuple[int, ...]]) -> set[tuple[int, int]]:
    """Index pairs sharing a band — the near-linear replacement for O(n^2).

    Comparing every chunk against every other is fine for a 30-chunk document
    and quadratic disaster for a 20 000-chunk one, which is the size real
    connector ingests reach. Banded MinHash makes candidate generation scale
    with the number of chunks rather than its square.
    """
    candidates: set[tuple[int, int]] = set()
    bands = SIGNATURE_SIZE // BAND_ROWS
    for band in range(bands):
        buckets: dict[tuple[int, ...], list[int]] = {}
        start = band * BAND_ROWS
        for index, signature in enumerate(signatures):
            key = signature[start : start + BAND_ROWS]
            buckets.setdefault(key, []).append(index)
        for members in buckets.values():
            if len(members) < 2:
                continue
            for i, left in enumerate(members):
                for right in members[i + 1 :]:
                    candidates.add((left, right))
    return candidates


@dataclass
class DedupResult:
    """What dedup decided, in full — kept, removed, and why."""

    kept: list[Any] = field(default_factory=list)
    removed: list[Any] = field(default_factory=list)

    @property
    def removed_count(self) -> int:
        return len(self.removed)

    @property
    def reduction(self) -> float:
        """Fraction of chunks eliminated — the KEU and KSU saving."""
        total = len(self.kept) + len(self.removed)
        return self.removed_count / total if total else 0.0

    def summary(self) -> str:
        return (
            f"kept={len(self.kept)} removed={self.removed_count} "
            f"reduction={self.reduction:.1%}"
        )


def deduplicate(
    chunks: list[Any], *, threshold: float = 0.9, shingle_size: int = 5
) -> DedupResult:
    """Keep one representative per near-duplicate group.

    ``threshold`` is deliberately high. The cost of keeping a duplicate is a
    little money; the cost of dropping a distinct chunk is content that can
    never be retrieved, and no downstream layer can detect that it is missing.
    Those are not symmetric, so the default errs toward keeping.

    **First occurrence wins.** Order is the document's own, so the representative
    is the earliest — which for boilerplate is the one whose surrounding context
    is most likely to be the real occurrence rather than a repeat.

    Removal is always *recorded*: the representative gains
    ``duplicates_absorbed`` and ``duplicate_spans``, so a reader can always tell
    that a region was covered by an identical chunk rather than lost.
    """
    if len(chunks) < 2:
        return DedupResult(kept=list(chunks))

    shingle_sets = [shingles(getattr(c, "text", ""), shingle_size) for c in chunks]
    signatures = [_signature(s) for s in shingle_sets]

    # Group by representative. Compare only against representatives, never
    # transitively chain: A~B and B~C does not make A~C, and chaining is how a
    # dedup pass quietly collapses a whole document into one chunk.
    representative_of: dict[int, int] = {}
    for left, right in sorted(_candidate_pairs(signatures)):
        if right in representative_of or left in representative_of:
            continue
        if jaccard(shingle_sets[left], shingle_sets[right]) >= threshold:
            representative_of[right] = left

    result = DedupResult()
    absorbed: dict[int, list[Any]] = {}
    for index, chunk in enumerate(chunks):
        owner = representative_of.get(index)
        if owner is None:
            result.kept.append(chunk)
        else:
            absorbed.setdefault(owner, []).append(chunk)
            result.removed.append(chunk)

    for owner, duplicates in absorbed.items():
        metadata = getattr(chunks[owner], "metadata", None)
        if isinstance(metadata, dict):
            metadata["duplicates_absorbed"] = len(duplicates)
            metadata["duplicate_spans"] = [
                (int(getattr(d, "start_pos", 0)), int(getattr(d, "end_pos", 0)))
                for d in duplicates
            ]
    return result
