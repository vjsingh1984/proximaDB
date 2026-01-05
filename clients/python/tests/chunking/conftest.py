"""
Pytest configuration and fixtures for code chunking tests.

Note: Tests rely on the editable install (pip install -e .)
rather than sys.path manipulation for consistent imports.
"""

import pytest
import tempfile
from pathlib import Path


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
