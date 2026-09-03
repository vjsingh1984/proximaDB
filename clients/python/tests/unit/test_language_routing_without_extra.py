"""Routing and attribution must not depend on the optional `codegraph` extra.

TD-CG2 made the DERIVED extension map empty when the extra is absent, so the SDK
could not advertise languages nothing could parse. Correct for the capability
question — and it was also wired to two questions that have nothing to do with
parser availability:

    routing      -- should this file be considered at all?
    attribution  -- what language is this file written in?

With the map empty, `code_knowledge.index_file` found no language for any
extension and returned `files_skipped=1`. Indexing a repository produced
`files_processed=0` with no error: a success-shaped result and an empty
collection. That is the same silent-loss defect TD-CG2 was filed against, one
layer up.

These tests hold the separation. They are written to pass with OR without the
extra, because the property under test is exactly that the answer does not
depend on it.
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock

from proximadb_sdk.chunking_strategies.code import (
    STATIC_EXTENSION_TO_LANGUAGE,
    static_language_for,
    static_supported_extensions,
)

try:  # the environment fact these tests are parameterised on
    import victor_codegraph  # noqa: F401

    CODEGRAPH = True
except ImportError:
    CODEGRAPH = False


def test_static_table_is_populated_regardless_of_the_extra():
    """The property whose absence caused the regression.

    Nothing asserted this, so an empty map read as "no languages" everywhere
    instead of "no parser installed".
    """
    assert len(STATIC_EXTENSION_TO_LANGUAGE) >= 20
    assert static_language_for(".py") == "python"
    assert static_language_for(".RS") == "rust", "must be case-insensitive"
    assert static_language_for(".nope") is None


def test_routing_is_wide_enough_to_find_source_files(tmp_path: Path):
    """Discovery must not collapse to nothing without the extra."""
    exts = set(static_supported_extensions())
    assert {".py", ".rs", ".go", ".java", ".ts"} <= exts


def test_indexing_a_python_file_never_silently_skips_it():
    """The regression, stated as behaviour rather than as a table lookup.

    Without the extra this must FAIL LOUDLY -- `files_failed` with an error that
    names the missing extra -- and never `files_processed=0, files_skipped=1`,
    which reads to a caller as "nothing to do here".

    The client mock mirrors the shape `test_code_knowledge_cov` uses. Inlined
    rather than imported across test files: CI runs each file in its own
    process, so a cross-file import is a fragility with no upside here.
    """
    from proximadb_sdk.code_knowledge import CodeIndexConfig, CodeKnowledgeBuilder

    client = MagicMock()
    client.list_collections = AsyncMock(return_value=[])
    client.create_collection = AsyncMock(return_value=None)
    client.list_graphs = AsyncMock(return_value=[])
    client.create_graph = AsyncMock(return_value=None)
    collection = MagicMock()
    collection.insert_records = AsyncMock(return_value=None)
    collection.insert = AsyncMock(return_value=None)
    client.get_collection = AsyncMock(return_value=collection)
    client.collection = MagicMock(return_value=collection)

    builder = CodeKnowledgeBuilder(client, config=CodeIndexConfig(vector_dimension=8))

    loop = asyncio.new_event_loop()
    try:
        result = loop.run_until_complete(
            builder.index_file("/tmp/example.py", content="def foo():\n    return 1\n")
        )
    finally:
        loop.close()

    assert result.files_skipped == 0, (
        "a .py file was skipped as if it were an unknown file type; that is the "
        "silent-loss regression this module exists to prevent"
    )
    if not CODEGRAPH:
        assert result.files_failed == 1
        assert result.errors, "a failure with no reason is indistinguishable from a bug"
        assert any(
            "codegraph" in str(e).lower() for e in result.errors
        ), f"the error must name the missing extra; got {result.errors}"


def test_language_attribution_does_not_require_a_parser():
    """Naming a file's language is not a claim to parse it."""
    from proximadb_sdk.chunking_strategies import parser_utils as pu

    result = pu.ConfigValidator.validate_languages(["python", "unknownlang"])
    assert result.valid is True
    assert not any(
        "python" in w for w in result.warnings
    ), "python is a real language whether or not a parser is installed"
    assert any("unknownlang" in w for w in result.warnings)
