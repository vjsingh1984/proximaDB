"""
Pytest configuration and fixtures for code chunking tests.
"""

import pytest
import sys
import types
import importlib.util
from pathlib import Path
import tempfile
import os

# Setup src path
src_path = Path(__file__).parent.parent.parent / "src"
if str(src_path) not in sys.path:
    sys.path.insert(0, str(src_path))


def _setup_modules():
    """Load chunking modules without triggering protobuf imports."""
    # Create minimal package structure
    if 'proximadb' not in sys.modules:
        proximadb = types.ModuleType('proximadb')
        sys.modules['proximadb'] = proximadb
    else:
        proximadb = sys.modules['proximadb']

    if 'proximadb.chunking_strategies' not in sys.modules:
        chunking_strategies = types.ModuleType('proximadb.chunking_strategies')
        sys.modules['proximadb.chunking_strategies'] = chunking_strategies
        proximadb.chunking_strategies = chunking_strategies
    else:
        chunking_strategies = sys.modules['proximadb.chunking_strategies']

    # Load base module if not already loaded
    if 'proximadb.chunking_strategies.base' not in sys.modules:
        base_spec = importlib.util.spec_from_file_location(
            'proximadb.chunking_strategies.base',
            str(src_path / 'proximadb' / 'chunking_strategies' / 'base.py')
        )
        base_module = importlib.util.module_from_spec(base_spec)
        sys.modules['proximadb.chunking_strategies.base'] = base_module
        base_spec.loader.exec_module(base_module)
        chunking_strategies.base = base_module

    # Load semantic module if not already loaded (needed for fallback)
    if 'proximadb.chunking_strategies.semantic' not in sys.modules:
        semantic_spec = importlib.util.spec_from_file_location(
            'proximadb.chunking_strategies.semantic',
            str(src_path / 'proximadb' / 'chunking_strategies' / 'semantic.py')
        )
        semantic_module = importlib.util.module_from_spec(semantic_spec)
        sys.modules['proximadb.chunking_strategies.semantic'] = semantic_module
        semantic_spec.loader.exec_module(semantic_module)
        chunking_strategies.semantic = semantic_module

    # Load code module if not already loaded
    if 'proximadb.chunking_strategies.code' not in sys.modules:
        code_spec = importlib.util.spec_from_file_location(
            'proximadb.chunking_strategies.code',
            str(src_path / 'proximadb' / 'chunking_strategies' / 'code.py')
        )
        code_module = importlib.util.module_from_spec(code_spec)
        sys.modules['proximadb.chunking_strategies.code'] = code_module
        code_spec.loader.exec_module(code_module)
        chunking_strategies.code = code_module

    # Load code_knowledge module if not already loaded
    if 'proximadb.code_knowledge' not in sys.modules:
        code_knowledge_spec = importlib.util.spec_from_file_location(
            'proximadb.code_knowledge',
            str(src_path / 'proximadb' / 'code_knowledge.py')
        )
        code_knowledge_module = importlib.util.module_from_spec(code_knowledge_spec)
        sys.modules['proximadb.code_knowledge'] = code_knowledge_module
        code_knowledge_spec.loader.exec_module(code_knowledge_module)
        proximadb.code_knowledge = code_knowledge_module

    return sys.modules['proximadb.chunking_strategies.code']


# Setup modules on import
code_module = _setup_modules()


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
        return filepath.read_text(encoding='utf-8')
    return None


@pytest.fixture(scope="session")
def rust_sample():
    """Return the Rust sample code."""
    filepath = RESOURCES_DIR / "rust" / "sample.rs"
    if filepath.exists():
        return filepath.read_text(encoding='utf-8')
    return None


@pytest.fixture(scope="session")
def go_sample():
    """Return the Go sample code."""
    filepath = RESOURCES_DIR / "go" / "sample.go"
    if filepath.exists():
        return filepath.read_text(encoding='utf-8')
    return None


@pytest.fixture(scope="session")
def java_sample():
    """Return the Java sample code."""
    filepath = RESOURCES_DIR / "java" / "Sample.java"
    if filepath.exists():
        return filepath.read_text(encoding='utf-8')
    return None


@pytest.fixture(scope="session")
def javascript_sample():
    """Return the JavaScript sample code."""
    filepath = RESOURCES_DIR / "javascript" / "sample.js"
    if filepath.exists():
        return filepath.read_text(encoding='utf-8')
    return None


@pytest.fixture(scope="session")
def typescript_sample():
    """Return the TypeScript sample code."""
    filepath = RESOURCES_DIR / "typescript" / "sample.ts"
    if filepath.exists():
        return filepath.read_text(encoding='utf-8')
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
        return filepath.read_text(encoding='utf-8')
    return ""


# Export the helper function
__all__ = ['read_resource_file', 'RESOURCES_DIR']
