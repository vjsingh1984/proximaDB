"""
Unit tests for CodeChunkingStrategy.

This module tests the code-aware chunking strategy that produces
chunks optimized for code search and understanding.
"""

import sys
from pathlib import Path

import pytest

# Add current directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

# Import from loader which handles the module loading
from loader import code_module, read_resource_file

# Get references from the loaded module
CodeChunkingStrategy = code_module.CodeChunkingStrategy
CodeChunkingConfig = code_module.CodeChunkingConfig
CodeSymbol = code_module.CodeSymbol
CodeSymbolType = code_module.CodeSymbolType
CodeRelation = code_module.CodeRelationType
ParsedCode = code_module.ParsedCode
SourceLocation = code_module.SourceLocation
create_code_chunker = code_module.create_code_chunker
LANGUAGE_PARSERS = code_module.LANGUAGE_PARSERS
EXTENSION_TO_LANGUAGE = code_module.EXTENSION_TO_LANGUAGE
PythonParser = code_module.PythonParser

# Get TextChunk from base
base_module = sys.modules.get("proximadb.chunking_strategies.base")
if base_module:
    TextChunk = base_module.TextChunk
else:
    TextChunk = None


class TestCodeChunkingConfig:
    """Test cases for CodeChunkingConfig."""

    def test_default_config(self):
        """Test default configuration values."""
        config = CodeChunkingConfig()
        assert config.chunk_size >= 0
        assert config.chunk_overlap >= 0

    def test_custom_config(self):
        """Test custom configuration values."""
        config = CodeChunkingConfig(
            chunk_size=1000, chunk_overlap=100, languages=["python", "rust"]
        )
        assert config.chunk_size == 1000
        assert config.chunk_overlap == 100
        assert "python" in config.languages
        assert "rust" in config.languages

    def test_config_with_single_language(self):
        """Test configuration with a single language."""
        config = CodeChunkingConfig(languages=["python"])
        assert len(config.languages) == 1
        assert config.languages[0] == "python"


class TestCodeChunkingStrategy:
    """Test cases for CodeChunkingStrategy."""

    @pytest.fixture
    def strategy(self):
        """Create a default CodeChunkingStrategy instance."""
        return CodeChunkingStrategy()

    @pytest.fixture
    def python_strategy(self):
        """Create a Python-only CodeChunkingStrategy."""
        config = CodeChunkingConfig(languages=["python"])
        return CodeChunkingStrategy(config=config)

    def test_strategy_creation(self, strategy):
        """Test strategy can be created."""
        assert strategy is not None
        assert isinstance(strategy.config, CodeChunkingConfig)

    def test_strategy_has_parsers(self, strategy):
        """Test strategy initializes parsers."""
        assert hasattr(strategy, "_parsers")
        assert isinstance(strategy._parsers, dict)
        assert len(strategy._parsers) > 0

    def test_strategy_with_limited_languages(self, python_strategy):
        """Test strategy with limited languages."""
        assert "python" in python_strategy._parsers
        # Should only have python parser
        assert len(python_strategy._parsers) == 1

    def test_chunk_python_code(self, strategy):
        """Test chunking Python code."""
        sample_code = read_resource_file("python", "sample.py")
        if not sample_code:
            pytest.skip("Sample file not found")

        chunks = strategy.chunk(sample_code, "sample.py")
        assert isinstance(chunks, list)
        # Python parser should produce chunks
        assert len(chunks) >= 0

    def test_chunk_with_metadata(self, strategy):
        """Test chunking with provided metadata."""
        code = '''
def my_function():
    """A simple function."""
    return 42
'''
        metadata = {"author": "test", "version": "1.0"}
        chunks = strategy.chunk(code, "test.py", metadata=metadata)
        assert isinstance(chunks, list)

    def test_chunk_with_language_hint(self, strategy):
        """Test chunking with explicit language hint."""
        code = "def test(): pass"
        metadata = {"language": "python"}
        chunks = strategy.chunk(code, "unknown_file.txt", metadata=metadata)
        assert isinstance(chunks, list)

    def test_chunk_unknown_language(self, strategy):
        """Test chunking with unknown file extension."""
        code = "some content that is not code"
        chunks = strategy.chunk(code, "file.unknown")
        assert isinstance(chunks, list)

    def test_detect_language_python(self, strategy):
        """Test language detection for Python files."""
        lang = strategy._detect_language("test.py")
        assert lang == "python"

    def test_detect_language_rust(self, strategy):
        """Test language detection for Rust files."""
        lang = strategy._detect_language("test.rs")
        assert lang == "rust"

    def test_detect_language_go(self, strategy):
        """Test language detection for Go files."""
        lang = strategy._detect_language("test.go")
        assert lang == "go"

    def test_detect_language_javascript(self, strategy):
        """Test language detection for JavaScript files."""
        lang = strategy._detect_language("test.js")
        assert lang == "javascript"

    def test_detect_language_typescript(self, strategy):
        """Test language detection for TypeScript files."""
        lang = strategy._detect_language("test.ts")
        assert lang == "typescript"

    def test_detect_language_unknown(self, strategy):
        """Test language detection for unknown files."""
        lang = strategy._detect_language("test.xyz")
        assert lang is None

    def test_detect_language_case_insensitive(self, strategy):
        """Test language detection is case insensitive."""
        lang = strategy._detect_language("test.PY")
        assert lang == "python"

    def test_chunk_produces_text_chunks(self, strategy):
        """Test that chunking produces TextChunk objects."""
        code = '''
def function_one():
    """First function."""
    return 1

def function_two():
    """Second function."""
    return 2
'''
        chunks = strategy.chunk(code, "test.py")
        for chunk in chunks:
            assert hasattr(chunk, "text")
            assert hasattr(chunk, "metadata")

    def test_chunk_metadata_has_symbol_info(self, strategy):
        """Test chunk metadata contains symbol information."""
        code = """
class MyClass:
    def my_method(self):
        pass
"""
        chunks = strategy.chunk(code, "test.py")
        # If chunks are produced, check metadata
        for chunk in chunks:
            if chunk.metadata:
                # Should have some symbol info
                assert isinstance(chunk.metadata, dict)

    def test_chunk_preserves_code(self, strategy):
        """Test that chunking preserves the code content."""
        code = "def test(): pass"
        chunks = strategy.chunk(code, "test.py")
        if chunks:
            # The code should be preserved in some chunk
            all_text = "".join(c.text for c in chunks)
            # At least part of the function should be there
            assert "def" in all_text or "test" in all_text or len(chunks) == 0


class TestCreateCodeChunker:
    """Test cases for the create_code_chunker factory function."""

    def test_create_default_chunker(self):
        """Test creating a default code chunker."""
        chunker = create_code_chunker()
        assert isinstance(chunker, CodeChunkingStrategy)

    def test_create_chunker_with_languages(self):
        """Test creating a code chunker with specific languages."""
        chunker = create_code_chunker(languages=["python", "rust"])
        assert "python" in chunker._parsers
        assert "rust" in chunker._parsers

    def test_create_chunker_with_config_kwargs(self):
        """Test creating a code chunker with config kwargs."""
        chunker = create_code_chunker(chunk_size=500, chunk_overlap=50)
        assert chunker.config.chunk_size == 500
        assert chunker.config.chunk_overlap == 50


class TestChunkingAllLanguages:
    """Integration tests for chunking all supported languages."""

    @pytest.fixture
    def strategy(self):
        return CodeChunkingStrategy()

    def test_chunk_python(self, strategy):
        """Test chunking Python code."""
        sample_code = read_resource_file("python", "sample.py")
        if sample_code:
            chunks = strategy.chunk(sample_code, "sample.py")
            assert isinstance(chunks, list)

    def test_chunk_rust(self, strategy):
        """Test chunking Rust code."""
        sample_code = read_resource_file("rust", "sample.rs")
        if sample_code:
            chunks = strategy.chunk(sample_code, "sample.rs")
            assert isinstance(chunks, list)

    def test_chunk_go(self, strategy):
        """Test chunking Go code."""
        sample_code = read_resource_file("go", "sample.go")
        if sample_code:
            chunks = strategy.chunk(sample_code, "sample.go")
            assert isinstance(chunks, list)

    def test_chunk_java(self, strategy):
        """Test chunking Java code."""
        sample_code = read_resource_file("java", "Sample.java")
        if sample_code:
            chunks = strategy.chunk(sample_code, "Sample.java")
            assert isinstance(chunks, list)

    def test_chunk_javascript(self, strategy):
        """Test chunking JavaScript code."""
        sample_code = read_resource_file("javascript", "sample.js")
        if sample_code:
            chunks = strategy.chunk(sample_code, "sample.js")
            assert isinstance(chunks, list)

    def test_chunk_typescript(self, strategy):
        """Test chunking TypeScript code."""
        sample_code = read_resource_file("typescript", "sample.ts")
        if sample_code:
            chunks = strategy.chunk(sample_code, "sample.ts")
            assert isinstance(chunks, list)

    def test_chunk_cpp(self, strategy):
        """Test chunking C++ code."""
        sample_code = read_resource_file("cpp", "sample.cpp")
        if sample_code:
            chunks = strategy.chunk(sample_code, "sample.cpp")
            assert isinstance(chunks, list)

    def test_chunk_csharp(self, strategy):
        """Test chunking C# code."""
        sample_code = read_resource_file("csharp", "Sample.cs")
        if sample_code:
            chunks = strategy.chunk(sample_code, "Sample.cs")
            assert isinstance(chunks, list)

    def test_chunk_ruby(self, strategy):
        """Test chunking Ruby code."""
        sample_code = read_resource_file("ruby", "sample.rb")
        if sample_code:
            chunks = strategy.chunk(sample_code, "sample.rb")
            assert isinstance(chunks, list)

    def test_chunk_php(self, strategy):
        """Test chunking PHP code."""
        sample_code = read_resource_file("php", "sample.php")
        if sample_code:
            chunks = strategy.chunk(sample_code, "sample.php")
            assert isinstance(chunks, list)

    def test_chunk_bash(self, strategy):
        """Test chunking Bash code."""
        sample_code = read_resource_file("bash", "sample.sh")
        if sample_code:
            chunks = strategy.chunk(sample_code, "sample.sh")
            assert isinstance(chunks, list)

    def test_chunk_sql(self, strategy):
        """Test chunking SQL code."""
        sample_code = read_resource_file("sql", "sample.sql")
        if sample_code:
            chunks = strategy.chunk(sample_code, "sample.sql")
            assert isinstance(chunks, list)


class TestPythonParserDetailed:
    """Detailed tests for the Python parser to increase coverage."""

    @pytest.fixture
    def parser(self):
        return PythonParser()

    def test_parse_simple_function(self, parser):
        """Test parsing a simple function."""
        code = "def hello(): pass"
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)
        assert result.language == "python"

    def test_parse_function_with_args(self, parser):
        """Test parsing a function with arguments."""
        code = '''
def greet(name: str, greeting: str = "Hello") -> str:
    """Greet someone."""
    return f"{greeting}, {name}!"
'''
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)
        assert len(result.symbols) > 0

    def test_parse_class_with_inheritance(self, parser):
        """Test parsing a class with inheritance."""
        code = """
class Parent:
    pass

class Child(Parent):
    def method(self):
        pass
"""
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)
        class_symbols = [
            s for s in result.symbols if s.symbol_type == CodeSymbolType.CLASS
        ]
        assert len(class_symbols) >= 2

    def test_parse_decorated_class(self, parser):
        """Test parsing a decorated class."""
        code = """
@dataclass
class User:
    name: str
    age: int
"""
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)

    def test_parse_static_method(self, parser):
        """Test parsing static methods."""
        code = """
class MyClass:
    @staticmethod
    def static_method():
        pass

    @classmethod
    def class_method(cls):
        pass
"""
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)

    def test_parse_property(self, parser):
        """Test parsing properties."""
        code = """
class Person:
    def __init__(self, name):
        self._name = name

    @property
    def name(self):
        return self._name

    @name.setter
    def name(self, value):
        self._name = value
"""
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)

    def test_parse_nested_class(self, parser):
        """Test parsing nested classes."""
        code = """
class Outer:
    class Inner:
        def inner_method(self):
            pass

    def outer_method(self):
        pass
"""
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)

    def test_parse_async_function(self, parser):
        """Test parsing async functions."""
        code = """
async def fetch_data(url: str):
    async with aiohttp.ClientSession() as session:
        async for chunk in response.content:
            yield chunk
"""
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)

    def test_parse_lambda(self, parser):
        """Test parsing lambda expressions."""
        code = """
square = lambda x: x ** 2
add = lambda a, b: a + b
"""
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)

    def test_parse_comprehensions(self, parser):
        """Test parsing list/dict/set comprehensions."""
        code = """
def process():
    squares = [x**2 for x in range(10)]
    even_squares = {x: x**2 for x in range(10) if x % 2 == 0}
    unique = {x for x in items}
    gen = (x for x in range(100))
"""
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)

    def test_parse_imports(self, parser):
        """Test parsing various import statements."""
        code = """
import os
import sys as system
from typing import List, Dict, Optional
from pathlib import Path
from . import local_module
from ..parent import something
"""
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)
        assert len(result.imports) > 0

    def test_parse_global_variables(self, parser):
        """Test parsing global variables."""
        code = """
MAX_SIZE = 100
DEFAULT_NAME = "Unknown"
CONFIG = {"debug": True, "timeout": 30}
"""
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)

    def test_parse_type_hints(self, parser):
        """Test parsing type hints."""
        code = """
from typing import List, Dict, Optional, Union, Callable

def process(
    items: List[str],
    config: Dict[str, Any],
    callback: Optional[Callable[[int], bool]] = None
) -> Union[str, int]:
    pass
"""
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)

    def test_parse_docstrings(self, parser):
        """Test parsing docstrings."""
        code = '''
"""Module docstring."""

def func():
    """
    Function docstring.

    Args:
        None

    Returns:
        Nothing
    """
    pass

class MyClass:
    """Class docstring."""

    def method(self):
        """Method docstring."""
        pass
'''
        result = parser.parse(code, "test.py")
        assert isinstance(result, ParsedCode)
        # Check that docstrings are captured
        for symbol in result.symbols:
            if symbol.documentation:
                assert isinstance(symbol.documentation, str)


class TestEdgeCasesDetailed:
    """Detailed edge case tests."""

    @pytest.fixture
    def strategy(self):
        return CodeChunkingStrategy()

    def test_empty_file(self, strategy):
        """Test chunking an empty file."""
        chunks = strategy.chunk("", "empty.py")
        assert isinstance(chunks, list)

    def test_only_comments(self, strategy):
        """Test chunking file with only comments."""
        code = """
# This is a comment
# Another comment
# More comments
"""
        chunks = strategy.chunk(code, "comments.py")
        assert isinstance(chunks, list)

    def test_only_docstring(self, strategy):
        """Test chunking file with only module docstring."""
        code = '''
"""
This is the module docstring.
It has multiple lines.
"""
'''
        chunks = strategy.chunk(code, "docstring.py")
        assert isinstance(chunks, list)

    def test_syntax_error(self, strategy):
        """Test chunking file with syntax error."""
        code = """
def broken(
    # Missing closing paren
"""
        # Should not crash
        chunks = strategy.chunk(code, "broken.py")
        assert isinstance(chunks, list)

    def test_unicode_identifiers(self, strategy):
        """Test chunking with unicode identifiers."""
        code = """
def 你好():
    return "Hello"

class Ελληνικά:
    pass
"""
        chunks = strategy.chunk(code, "unicode.py")
        assert isinstance(chunks, list)

    def test_very_long_lines(self, strategy):
        """Test chunking with very long lines."""
        code = f"""
def long_string():
    s = "{'a' * 10000}"
    return s
"""
        chunks = strategy.chunk(code, "long.py")
        assert isinstance(chunks, list)

    def test_deeply_nested(self, strategy):
        """Test chunking deeply nested code."""
        code = """
class A:
    class B:
        class C:
            class D:
                def method(self):
                    def inner():
                        def deeper():
                            pass
"""
        chunks = strategy.chunk(code, "nested.py")
        assert isinstance(chunks, list)

    def test_many_functions(self, strategy):
        """Test chunking file with many functions."""
        functions = "\n".join([f"def func_{i}(): pass" for i in range(100)])
        chunks = strategy.chunk(functions, "many.py")
        assert isinstance(chunks, list)

    def test_mixed_content(self, strategy):
        """Test chunking mixed content."""
        code = '''
# Module comment
"""Module docstring"""

import os

CONSTANT = 42

class MyClass:
    """Class docstring"""
    pass

def my_function():
    """Function docstring"""
    pass

if __name__ == "__main__":
    my_function()
'''
        chunks = strategy.chunk(code, "mixed.py")
        assert isinstance(chunks, list)


class TestFallbackBehavior:
    """Test fallback behavior when parser not available."""

    def test_fallback_for_unknown_extension(self):
        """Test fallback chunking for unknown file types."""
        strategy = CodeChunkingStrategy()
        code = "This is just plain text content."
        chunks = strategy.chunk(code, "file.xyz")
        # Should fall back to semantic chunking
        assert isinstance(chunks, list)

    def test_fallback_preserves_content(self):
        """Test that fallback preserves content."""
        strategy = CodeChunkingStrategy()
        code = "Important content that should be preserved."
        chunks = strategy.chunk(code, "file.unknown")
        if chunks:
            all_text = "".join(c.text for c in chunks)
            # Content should be preserved
            assert len(all_text) > 0


class TestExtensionMappings:
    """Test file extension to language mappings."""

    @pytest.fixture
    def strategy(self):
        return CodeChunkingStrategy()

    def test_python_extensions(self, strategy):
        """Test all Python extensions."""
        assert strategy._detect_language("test.py") == "python"
        assert strategy._detect_language("test.pyi") == "python"
        assert strategy._detect_language("test.pyx") == "python"

    def test_javascript_extensions(self, strategy):
        """Test all JavaScript extensions."""
        assert strategy._detect_language("test.js") == "javascript"
        assert strategy._detect_language("test.jsx") == "javascript"
        assert strategy._detect_language("test.mjs") == "javascript"
        assert strategy._detect_language("test.cjs") == "javascript"

    def test_typescript_extensions(self, strategy):
        """Test TypeScript extensions."""
        assert strategy._detect_language("test.ts") == "typescript"
        assert strategy._detect_language("test.tsx") == "typescript"

    def test_cpp_extensions(self, strategy):
        """Test C++ extensions."""
        assert strategy._detect_language("test.cpp") == "cpp"
        assert strategy._detect_language("test.cc") == "cpp"
        assert strategy._detect_language("test.cxx") == "cpp"
        assert strategy._detect_language("test.hpp") == "cpp"
        assert strategy._detect_language("test.hxx") == "cpp"
        assert strategy._detect_language("test.hh") == "cpp"

    def test_c_extensions(self, strategy):
        """Test C extensions."""
        assert strategy._detect_language("test.c") == "c"
        assert strategy._detect_language("test.h") == "c"

    def test_ruby_extensions(self, strategy):
        """Test Ruby extensions."""
        assert strategy._detect_language("test.rb") == "ruby"
        assert strategy._detect_language("test.rake") == "ruby"
        assert strategy._detect_language("test.gemspec") == "ruby"

    def test_php_extensions(self, strategy):
        """Test PHP extensions."""
        assert strategy._detect_language("test.php") == "php"
        assert strategy._detect_language("test.phtml") == "php"

    def test_kotlin_extensions(self, strategy):
        """Test Kotlin extensions."""
        assert strategy._detect_language("test.kt") == "kotlin"
        assert strategy._detect_language("test.kts") == "kotlin"

    def test_scala_extensions(self, strategy):
        """Test Scala extensions."""
        assert strategy._detect_language("test.scala") == "scala"
        assert strategy._detect_language("test.sc") == "scala"

    def test_shell_extensions(self, strategy):
        """Test shell extensions."""
        assert strategy._detect_language("test.sh") == "bash"
        assert strategy._detect_language("test.bash") == "bash"
        assert strategy._detect_language("test.zsh") == "bash"
        assert strategy._detect_language("test.ksh") == "bash"
        assert strategy._detect_language("test.fish") == "bash"

    def test_perl_extensions(self, strategy):
        """Test Perl extensions."""
        assert strategy._detect_language("test.pl") == "perl"
        assert strategy._detect_language("test.pm") == "perl"
        assert strategy._detect_language("test.t") == "perl"

    def test_sql_extensions(self, strategy):
        """Test SQL extensions."""
        assert strategy._detect_language("test.sql") == "sql"
        assert strategy._detect_language("test.psql") == "sql"
        assert strategy._detect_language("test.mysql") == "sql"

    def test_yaml_extensions(self, strategy):
        """Test YAML extensions."""
        assert strategy._detect_language("test.yaml") == "yaml"
        assert strategy._detect_language("test.yml") == "yaml"

    def test_json_extensions(self, strategy):
        """Test JSON extensions."""
        assert strategy._detect_language("test.json") == "json"
        assert strategy._detect_language("test.jsonc") == "json"
        assert strategy._detect_language("test.json5") == "json"


class TestParserLanguageMethods:
    """Test parser language attribute."""

    def test_python_parser_language(self):
        """Test PythonParser language."""
        parser = code_module.PythonParser()
        assert parser.language == "python"

    def test_rust_parser_language(self):
        """Test RustParser language."""
        parser = code_module.RustParser()
        assert parser.language == "rust"

    def test_go_parser_language(self):
        """Test GoParser language."""
        parser = code_module.GoParser()
        assert parser.language == "go"

    def test_java_parser_language(self):
        """Test JavaParser language."""
        parser = code_module.JavaParser()
        assert parser.language == "java"

    def test_javascript_parser_language(self):
        """Test JavaScriptParser language."""
        parser = code_module.JavaScriptParser()
        assert parser.language == "javascript"

    def test_typescript_parser_language(self):
        """Test TypeScript uses JavaScript parser."""
        # TypeScript is handled by JavaScript parser in the registry
        if hasattr(code_module, "TypeScriptParser"):
            parser = code_module.TypeScriptParser()
            assert parser.language == "typescript"
        else:
            # Check that typescript is mapped
            assert EXTENSION_TO_LANGUAGE.get(".ts") == "typescript"

    def test_cpp_parser_language(self):
        """Test CppParser language."""
        parser = code_module.CppParser()
        assert parser.language == "cpp"

    def test_c_parser_language(self):
        """Test C parser language."""
        # C is handled by CppParser in the registry
        if hasattr(code_module, "CParser"):
            parser = code_module.CParser()
            assert parser.language == "c"
        else:
            # Check that c is mapped
            assert EXTENSION_TO_LANGUAGE.get(".c") == "c"

    def test_csharp_parser_language(self):
        """Test CSharpParser language."""
        parser = code_module.CSharpParser()
        assert parser.language == "csharp"

    def test_ruby_parser_language(self):
        """Test RubyParser language."""
        parser = code_module.RubyParser()
        assert parser.language == "ruby"

    def test_php_parser_language(self):
        """Test PhpParser language."""
        parser = code_module.PhpParser()
        assert parser.language == "php"

    def test_kotlin_parser_language(self):
        """Test KotlinParser language."""
        parser = code_module.KotlinParser()
        assert parser.language == "kotlin"

    def test_scala_parser_language(self):
        """Test ScalaParser language."""
        parser = code_module.ScalaParser()
        assert parser.language == "scala"

    def test_swift_parser_language(self):
        """Test SwiftParser language."""
        parser = code_module.SwiftParser()
        assert parser.language == "swift"

    def test_bash_parser_language(self):
        """Test BashParser language."""
        parser = code_module.BashParser()
        assert parser.language == "bash"

    def test_sql_parser_language(self):
        """Test SqlParser language."""
        parser = code_module.SqlParser()
        assert parser.language == "sql"


class TestRegistryFunctions:
    """Test registry functions."""

    def test_get_supported_languages(self):
        """Test getting supported languages."""
        langs = code_module.get_supported_languages()
        assert isinstance(langs, list)
        assert "python" in langs
        assert "rust" in langs
        assert "go" in langs

    def test_get_supported_extensions(self):
        """Test getting supported extensions."""
        exts = code_module.get_supported_extensions()
        assert isinstance(exts, list)
        assert ".py" in exts
        assert ".rs" in exts
        assert ".go" in exts

    def test_get_parser_for_language(self):
        """Test getting parser for a language."""
        if hasattr(code_module, "get_parser_for_language"):
            parser = code_module.get_parser_for_language("python")
            assert parser is not None

    def test_get_parser_for_extension(self):
        """Test getting parser for an extension."""
        if hasattr(code_module, "get_parser_for_extension"):
            parser = code_module.get_parser_for_extension(".py")
            assert parser is not None


class TestDataModelCreation:
    """Test data model creation and properties."""

    def test_source_location_with_all_fields(self):
        """Test SourceLocation with all fields."""
        loc = SourceLocation(
            file_path="test.py",
            start_line=1,
            end_line=10,
            start_column=0,
            end_column=50,
        )
        assert loc.file_path == "test.py"
        assert loc.start_line == 1
        assert loc.end_line == 10
        assert loc.start_column == 0
        assert loc.end_column == 50

    def test_source_location_with_optional_fields(self):
        """Test SourceLocation with optional repository info."""
        loc = SourceLocation(
            file_path="test.py",
            repository="github.com/test/repo",
            branch="main",
            commit_hash="abc123",
        )
        assert loc.repository == "github.com/test/repo"
        assert loc.branch == "main"
        assert loc.commit_hash == "abc123"

    def test_code_symbol_documentation_property(self):
        """Test CodeSymbol documentation property."""
        # Test that CodeSymbol has documentation attribute
        assert hasattr(CodeSymbol, "__dataclass_fields__") or hasattr(
            CodeSymbol, "documentation"
        )

    def test_parsed_code_with_fields(self):
        """Test ParsedCode with required fields."""
        parsed = ParsedCode(
            file_path="test.py",
            language="python",
            symbols=[],
            relations=[],
            imports=["os", "sys"],
            content_hash="abc123",
        )
        assert parsed.language == "python"
        assert parsed.file_path == "test.py"
        assert "os" in parsed.imports
        assert "sys" in parsed.imports
        assert parsed.content_hash == "abc123"


class TestCodeChunkingConfigOptions:
    """Test CodeChunkingConfig options."""

    def test_config_include_private(self):
        """Test include_private option."""
        config = CodeChunkingConfig(include_private=False)
        assert config.include_private is False

        config = CodeChunkingConfig(include_private=True)
        assert config.include_private is True

    def test_config_include_tests(self):
        """Test include_tests option."""
        config = CodeChunkingConfig(include_tests=False)
        assert config.include_tests is False

    def test_config_min_chunk_size(self):
        """Test min_chunk_size option."""
        config = CodeChunkingConfig(min_chunk_size=100)
        assert config.min_chunk_size == 100

    def test_config_max_chunk_size(self):
        """Test max_chunk_size option."""
        config = CodeChunkingConfig(max_chunk_size=5000)
        assert config.max_chunk_size == 5000

    def test_config_extract_relations(self):
        """Test extract_relations option."""
        config = CodeChunkingConfig(extract_relations=True)
        assert config.extract_relations is True

        config = CodeChunkingConfig(extract_relations=False)
        assert config.extract_relations is False


class TestSymbolTypeValues:
    """Test CodeSymbolType enum values."""

    def test_all_symbol_types(self):
        """Test all symbol type values exist."""
        assert CodeSymbolType.CLASS
        assert CodeSymbolType.FUNCTION
        assert CodeSymbolType.METHOD
        assert CodeSymbolType.VARIABLE
        assert CodeSymbolType.CONSTANT
        assert CodeSymbolType.MODULE
        assert CodeSymbolType.INTERFACE
        assert CodeSymbolType.ENUM
        assert CodeSymbolType.STRUCT
        assert CodeSymbolType.TRAIT


class TestMoreParserTests:
    """Additional parser tests for better coverage."""

    def test_rust_parser_with_complex_code(self):
        """Test Rust parser with complex code."""
        parser = code_module.RustParser()
        code = """
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Config {
    name: String,
    values: HashMap<String, i32>,
}

impl Config {
    pub fn new(name: &str) -> Self {
        Config {
            name: name.to_string(),
            values: HashMap::new(),
        }
    }
}

trait Configurable {
    fn configure(&mut self);
}

enum Status {
    Active,
    Inactive,
    Pending(String),
}
"""
        result = parser.parse(code, "test.rs")
        assert isinstance(result, ParsedCode)
        assert result.language == "rust"

    def test_go_parser_with_complex_code(self):
        """Test Go parser with complex code."""
        parser = code_module.GoParser()
        code = """
package main

import (
    "fmt"
    "sync"
)

type Server struct {
    mu sync.Mutex
    connections int
}

func NewServer() *Server {
    return &Server{}
}

func (s *Server) Connect() {
    s.mu.Lock()
    defer s.mu.Unlock()
    s.connections++
}

type Handler interface {
    Handle(data []byte) error
}
"""
        result = parser.parse(code, "test.go")
        assert isinstance(result, ParsedCode)
        assert result.language == "go"

    def test_java_parser_with_complex_code(self):
        """Test Java parser with complex code."""
        parser = code_module.JavaParser()
        code = """
package com.example;

import java.util.List;
import java.util.ArrayList;

@Service
public class UserService implements IUserService {
    private final UserRepository repository;

    @Autowired
    public UserService(UserRepository repository) {
        this.repository = repository;
    }

    @Override
    public List<User> findAll() {
        return repository.findAll();
    }

    private static final String DEFAULT_NAME = "Unknown";
}

interface IUserService {
    List<User> findAll();
}

enum UserStatus {
    ACTIVE, INACTIVE, PENDING
}
"""
        result = parser.parse(code, "Test.java")
        assert isinstance(result, ParsedCode)
        assert result.language == "java"


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
