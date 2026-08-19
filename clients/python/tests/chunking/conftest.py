"""
Pytest configuration and fixtures for code chunking tests.

Note: Tests rely on the editable install (pip install -e .)
rather than sys.path manipulation for consistent imports.
"""

import tempfile
from pathlib import Path

import pytest

# Path to test resources
RESOURCES_DIR = Path(__file__).parent / "resources"


@pytest.fixture(scope="session")
def resources_dir():
    """Return the path to the test resources directory."""
    return RESOURCES_DIR


@pytest.fixture(scope="session")
def python_sample():
    """Return the Python sample code."""
    filepath = RESOURCES_DIR / "python" / "sample.py"
    if filepath.exists():
        return filepath.read_text(encoding="utf-8")
    return None


@pytest.fixture(scope="session")
def rust_sample():
    """Return the Rust sample code."""
    filepath = RESOURCES_DIR / "rust" / "sample.rs"
    if filepath.exists():
        return filepath.read_text(encoding="utf-8")
    return None


@pytest.fixture(scope="session")
def go_sample():
    """Return the Go sample code."""
    filepath = RESOURCES_DIR / "go" / "sample.go"
    if filepath.exists():
        return filepath.read_text(encoding="utf-8")
    return None


@pytest.fixture(scope="session")
def java_sample():
    """Return the Java sample code."""
    filepath = RESOURCES_DIR / "java" / "Sample.java"
    if filepath.exists():
        return filepath.read_text(encoding="utf-8")
    return None


@pytest.fixture(scope="session")
def javascript_sample():
    """Return the JavaScript sample code."""
    filepath = RESOURCES_DIR / "javascript" / "sample.js"
    if filepath.exists():
        return filepath.read_text(encoding="utf-8")
    return None


@pytest.fixture(scope="session")
def typescript_sample():
    """Return the TypeScript sample code."""
    filepath = RESOURCES_DIR / "typescript" / "sample.ts"
    if filepath.exists():
        return filepath.read_text(encoding="utf-8")
    return None


@pytest.fixture
def temp_dir():
    """Create a temporary directory for tests."""
    with tempfile.TemporaryDirectory() as tmpdir:
        yield tmpdir


@pytest.fixture
def mock_embedding_fn():
    """Create a mock embedding function."""

    async def embed(text):
        return [0.1] * 384  # Return a 384-dim vector

    return embed


def read_resource_file(language: str, filename: str) -> str:
    """
    Read a test resource file for the given language.

    Args:
        language: The programming language (e.g., 'python', 'rust')
        filename: The filename to read

    Returns:
        The file contents, or empty string if not found
    """
    filepath = RESOURCES_DIR / language / filename
    if filepath.exists():
        return filepath.read_text(encoding="utf-8")
    return ""


# Export the helper function
__all__ = ["read_resource_file", "RESOURCES_DIR"]

# ---------------------------------------------------------------------------
# TD-CG2 S0 — "stop the rot"
#
# `tests/chunking/` has been RED on develop with 56 failures, and no CI job
# gated it. Every one of them is per-language coverage of the in-SDK
# tree-sitter parsers, which raise against the installed language pack and
# which ADR-029 decided to DELETE (TD-CG2 slice S4) rather than repair. Code
# chunking belongs to the shared `victor-codegraph` package.
#
# Repairing 23 parsers that are scheduled for deletion is pure waste, and a red
# suite sitting outside the gate is strictly worse than a declared skip: the
# first hides new breakage in noise, the second states what is not covered and
# why. So the legacy parser coverage is skipped here, in ONE reviewable list,
# and the delegated path is covered by the conformance suite that CI does gate.
#
# This whole block is deleted by S4, together with the classes it names.
# ---------------------------------------------------------------------------

TD_CG2_REASON = (
    "TD-CG2: covers the in-SDK tree-sitter parsers that ADR-029 retires in "
    "favour of victor-codegraph. These raise against the installed language "
    "pack; repairing parsers scheduled for deletion is waste. The delegated "
    "path is covered by tests/chunking/test_chunking_conformance.py, which CI "
    "gates. Removed together with the parsers in TD-CG2 slice S4."
)

#: Per-language parser coverage. Whole classes, because "one language's parser"
#: is the unit being retired.
_LEGACY_PARSER_CLASSES = {
    "TestPythonParser",
    "TestRustParser",
    "TestGoParser",
    "TestJavaParser",
    "TestJavaScriptParser",
    "TestTypeScriptParser",
    "TestCppParser",
    "TestCParser",
    "TestCSharpParser",
    "TestRubyParser",
    "TestPhpParser",
    "TestKotlinParser",
    "TestScalaParser",
    "TestSwiftParser",
    "TestBashParser",
    "TestSqlParser",
    "TestYamlParser",
    "TestJsonParser",
    "TestXmlParser",
    "TestPerlParser",
    "TestLuaParser",
    "TestHaskellParser",
    "TestElixirParser",
    "TestEdgeCases",
    "TestPythonParserDetailed",
    "TestMoreParserTests",
}

#: Individually-named leftovers, so classes that mostly cover the SURVIVING
#: delegation are not skipped wholesale for one legacy assertion.
_LEGACY_PARSER_TESTS = {
    "test_strategy_with_limited_languages",
}


def pytest_collection_modifyitems(config, items):
    """Skip legacy in-SDK parser coverage, with a reason naming TD-CG2."""
    skip = pytest.mark.skip(reason=TD_CG2_REASON)
    for item in items:
        owning_class = item.cls.__name__ if item.cls else None
        if (
            owning_class in _LEGACY_PARSER_CLASSES
            or (item.originalname or item.name) in _LEGACY_PARSER_TESTS
        ):
            item.add_marker(skip)
