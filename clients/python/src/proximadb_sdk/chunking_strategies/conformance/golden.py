"""Golden output digests — the oracle for behaviour-preserving refactors.

The conformance invariants prove chunking is *correct*. They cannot prove a
refactor was *neutral*: a change that moves a boundary while keeping every
invariant satisfied is invisible to them. That is exactly the risk in a
mechanical change spread over dozens of call sites, where one transcription slip
produces output that is still valid and still wrong.

So this records what the chunkers actually emit, per (chunker, corpus entry), and
lets a later run assert byte-identity.

What is and is not in the digest
--------------------------------
**In:** ``text``, ``start_pos``, ``end_pos`` — the observable output, and the
things a consumer stores.

**Out:** metadata. It is deliberately excluded because it legitimately grows
(``offset_basis`` and ``offset_contract`` were added mid-slice, and a measure
field is coming). Including it would make the snapshot fail for benign reasons,
and a snapshot that cries wolf stops being read.

Like ``BASELINE`` in the conformance test, this is **generated, never
hand-edited** — it records what the code does, not what anyone believes it does.
A deliberate behaviour change regenerates it in the same commit, which is what
makes "did this refactor move anything?" a question with an answer.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Sequence
from typing import Any

from .corpus import CorpusEntry
from .runner import ChunkerAdapter

#: Bumped only when the digest RECIPE changes (never when output changes), so a
#: stale golden file fails loudly instead of comparing incomparable things.
DIGEST_RECIPE_VERSION = 1


def case_digest(chunks: Sequence[Any]) -> str:
    """Stable digest of one chunker's output for one document."""
    hasher = hashlib.sha256()
    hasher.update(f"v{DIGEST_RECIPE_VERSION}\n".encode())
    for chunk in chunks:
        start = int(getattr(chunk, "start_pos", -1))
        end = int(getattr(chunk, "end_pos", -1))
        text = getattr(chunk, "text", "")
        # Length-prefix the text so no concatenation of different chunk
        # boundaries can collide with another.
        hasher.update(f"{start}:{end}:{len(text)}:".encode())
        hasher.update(text.encode("utf-8"))
        hasher.update(b"\n")
    return hasher.hexdigest()


def case_key(chunker_name: str, corpus_name: str) -> str:
    return f"{chunker_name}|{corpus_name}"


def sweep_digests(
    adapters: Sequence[ChunkerAdapter], entries: Sequence[CorpusEntry]
) -> dict[str, str]:
    """Digest every (chunker, corpus entry) pair, in a stable order.

    A chunker that raises records ``"ERROR:<type>"`` rather than being skipped —
    a refactor that turns a crash into output, or output into a crash, is a
    behaviour change and must show up here.
    """
    digests: dict[str, str] = {}
    for adapter in adapters:
        for entry in entries:
            key = case_key(adapter.name, entry.name)
            try:
                digests[key] = case_digest(list(adapter.chunk(entry.text)))
            except Exception as exc:  # noqa: BLE001 - recorded, not swallowed
                digests[key] = f"ERROR:{type(exc).__name__}"
    return digests


def render_golden(digests: dict[str, str]) -> str:
    """Serialise for committing — sorted keys, so diffs are readable."""
    return json.dumps(
        {"recipe_version": DIGEST_RECIPE_VERSION, "cases": digests},
        indent=2,
        sort_keys=True,
    )


def load_golden(payload: str) -> dict[str, str]:
    """Parse a committed golden file, refusing a stale recipe."""
    data = json.loads(payload)
    recorded = data.get("recipe_version")
    if recorded != DIGEST_RECIPE_VERSION:
        raise ValueError(
            f"golden file was written with digest recipe v{recorded}, but this "
            f"code computes v{DIGEST_RECIPE_VERSION}; regenerate it deliberately "
            "rather than comparing incomparable digests"
        )
    return dict(data["cases"])


def diff_digests(
    expected: dict[str, str], actual: dict[str, str]
) -> tuple[list[str], list[str], list[str]]:
    """Return (changed, missing, added) case keys — the reviewable summary."""
    changed = sorted(
        key for key in expected.keys() & actual.keys() if expected[key] != actual[key]
    )
    missing = sorted(expected.keys() - actual.keys())
    added = sorted(actual.keys() - expected.keys())
    return changed, missing, added
