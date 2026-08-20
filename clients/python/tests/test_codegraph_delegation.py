"""TD-CG2 (ADR-029): the in-SDK code chunker delegates to the shared
``victor-codegraph`` package, which is now its only implementation.

The delegation tests are guarded with ``importorskip`` so the suite stays green
where the optional extra isn't installed (e.g. ProximaDB CI), and assert real
delegation where it is.
"""

from __future__ import annotations

import warnings

import pytest

from proximadb_sdk.chunking_strategies.code import (
    CodeChunkingConfig,
    CodeChunkingStrategy,
)

SAMPLE = "def a():\n    return b()\n\n\ndef b():\n    return 1\n"


def test_code_chunker_no_longer_warns_deprecated():
    """The deprecation was retired with the thing it deprecated (TD-CG2 S5).

    The warning was ADR-029's steer off a duplicate parser toward the shared
    package. S4 deleted that parser, so this class IS the consumption ADR-029
    asked for -- and a DeprecationWarning means "this will be removed", which is
    now false for the only supported code-chunking entry point in the SDK.
    A warning nobody can act on trains callers to filter warnings.
    """
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        CodeChunkingStrategy()
    assert not [
        w
        for w in caught
        if issubclass(w.category, DeprecationWarning)
        and "victor-codegraph" in str(w.message)
    ]


def test_chunk_still_returns_textchunks():
    # Works on either path (legacy or delegated): the public contract is unchanged.
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        chunks = CodeChunkingStrategy().chunk(SAMPLE, "m.py", {"language": "python"})
    assert chunks
    assert all(c.metadata.get("chunking_strategy") == "code" for c in chunks)


# --- Delegation path (requires the `codegraph` extra) --------------------------------

victor_codegraph = pytest.importorskip("victor_codegraph")


def _chunk(src, source_id="m.py", language="python", **cfg):
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        strat = CodeChunkingStrategy(CodeChunkingConfig(**cfg))
        return strat.chunk(src, source_id, {"language": language})


def test_delegates_to_victor_codegraph():
    chunks = _chunk(SAMPLE)
    assert chunks
    assert all(c.metadata.get("source") == "victor_codegraph" for c in chunks)
    names = {c.metadata.get("simple_name") for c in chunks}
    assert {"a", "b"} <= names


def test_size_cap_flows_through_delegation():
    # The legacy code.py had no size-capping; via delegation an oversized symbol splits.
    big = "def f():\n" + "\n".join(f"    a{i} = {i}" for i in range(300)) + "\n"
    chunks = _chunk(big, source_id="big.py", chunk_size=400)
    assert len(chunks) > 1
