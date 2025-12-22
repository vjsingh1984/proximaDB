"""
Module loader for code chunking tests.

This module handles loading the chunking modules without triggering protobuf imports.
"""

import sys
import types
import importlib.util
from pathlib import Path

# Setup src path
src_path = Path(__file__).parent.parent.parent / "src"
if str(src_path) not in sys.path:
    sys.path.insert(0, str(src_path))

# Path to test resources
RESOURCES_DIR = Path(__file__).parent / "resources"


def _load_module(name: str, file_name: str, parent_module: types.ModuleType):
    """Helper to load a chunking_strategies module."""
    # Check if module exists and has __file__ attribute (indicating it was loaded from a file)
    # If it's a mock module (no __file__), we need to load the real one
    existing = sys.modules.get(name)
    if existing is not None and hasattr(existing, '__file__') and existing.__file__ is not None:
        return existing

    # Use proximadb_sdk (actual package location) instead of proximadb
    spec = importlib.util.spec_from_file_location(
        name,
        str(src_path / 'proximadb_sdk' / 'chunking_strategies' / file_name)
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)

    # Set on parent
    attr_name = file_name.replace('.py', '')
    setattr(parent_module, attr_name, module)

    return module


def _load_root_module(name: str, file_name: str, parent_module: types.ModuleType):
    """Helper to load a module from proximadb_sdk root."""
    # Check if module exists and has __file__ attribute (indicating it was loaded from a file)
    # If it's a mock module (no __file__), we need to load the real one
    existing = sys.modules.get(name)
    if existing is not None and hasattr(existing, '__file__') and existing.__file__ is not None:
        return existing

    # Use proximadb_sdk (actual package location) instead of proximadb
    spec = importlib.util.spec_from_file_location(
        name,
        str(src_path / 'proximadb_sdk' / file_name)
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)

    # Set on parent
    attr_name = file_name.replace('.py', '')
    setattr(parent_module, attr_name, module)

    return module


def _setup_modules():
    """Load chunking modules without triggering protobuf imports."""
    # Clean up any mock modules that might have been created by other test files
    # Mock modules don't have __file__ set, so we can detect and remove them
    mock_module_patterns = ['proximadb.chunking_strategies.', 'proximadb.document_processor', 'proximadb.document_pipeline']
    for mod_name in list(sys.modules.keys()):
        for pattern in mock_module_patterns:
            if mod_name.startswith(pattern) or mod_name == pattern.rstrip('.'):
                mod = sys.modules[mod_name]
                if not hasattr(mod, '__file__') or mod.__file__ is None:
                    del sys.modules[mod_name]
                    break

    # Create minimal package structure with proper __path__ for relative imports
    if 'proximadb' not in sys.modules:
        proximadb = types.ModuleType('proximadb')
        # Set __path__ so Python can resolve subpackage imports
        proximadb.__path__ = [str(src_path / 'proximadb_sdk')]
        proximadb.__package__ = 'proximadb'
        sys.modules['proximadb'] = proximadb
    else:
        proximadb = sys.modules['proximadb']

    if 'proximadb.chunking_strategies' not in sys.modules:
        chunking_strategies = types.ModuleType('proximadb.chunking_strategies')
        # Set __path__ for the chunking_strategies subpackage
        chunking_strategies.__path__ = [str(src_path / 'proximadb_sdk' / 'chunking_strategies')]
        chunking_strategies.__package__ = 'proximadb.chunking_strategies'
        sys.modules['proximadb.chunking_strategies'] = chunking_strategies
        proximadb.chunking_strategies = chunking_strategies
    else:
        chunking_strategies = sys.modules['proximadb.chunking_strategies']

    # Load modules in dependency order
    # 1. Base module (no dependencies)
    _load_module('proximadb.chunking_strategies.base', 'base.py', chunking_strategies)

    # 2. Strategy modules (depend on base)
    _load_module('proximadb.chunking_strategies.sliding_window', 'sliding_window.py', chunking_strategies)
    _load_module('proximadb.chunking_strategies.sentence', 'sentence.py', chunking_strategies)
    _load_module('proximadb.chunking_strategies.paragraph', 'paragraph.py', chunking_strategies)
    _load_module('proximadb.chunking_strategies.semantic', 'semantic.py', chunking_strategies)
    _load_module('proximadb.chunking_strategies.recursive', 'recursive.py', chunking_strategies)
    _load_module('proximadb.chunking_strategies.fixed_size', 'fixed_size.py', chunking_strategies)

    # 3. Code module (depends on base, semantic)
    _load_module('proximadb.chunking_strategies.code', 'code.py', chunking_strategies)

    # 4. Parser utilities (standalone)
    _load_module('proximadb.chunking_strategies.parser_utils', 'parser_utils.py', chunking_strategies)

    # 5. Document parsers (depends on parser_utils)
    _load_module('proximadb.chunking_strategies.document_parsers', 'document_parsers.py', chunking_strategies)

    # 6. Factory (depends on all strategies)
    _load_module('proximadb.chunking_strategies.factory', 'factory.py', chunking_strategies)

    # 7. Pipeline (depends on factory, parser_utils)
    _load_module('proximadb.chunking_strategies.pipeline', 'pipeline.py', chunking_strategies)

    # Load code_knowledge module if not already loaded
    if 'proximadb.code_knowledge' not in sys.modules:
        code_knowledge_spec = importlib.util.spec_from_file_location(
            'proximadb.code_knowledge',
            str(src_path / 'proximadb_sdk' / 'code_knowledge.py')
        )
        code_knowledge_module = importlib.util.module_from_spec(code_knowledge_spec)
        sys.modules['proximadb.code_knowledge'] = code_knowledge_module
        code_knowledge_spec.loader.exec_module(code_knowledge_module)
        proximadb.code_knowledge = code_knowledge_module

    # Load document processor and pipeline from proximadb root
    _load_root_module('proximadb.document_processor', 'document_processor.py', proximadb)
    _load_root_module('proximadb.document_pipeline', 'document_pipeline.py', proximadb)

    return sys.modules['proximadb.chunking_strategies.code']


# Setup modules on import
code_module = _setup_modules()


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
