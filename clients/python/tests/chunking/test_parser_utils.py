"""
Unit tests for parser utilities module.

This module tests:
- Error classes
- Fallback strategies
- Metrics collection
- Parser caching
- Plugin registry
- Configuration validation
- Parser base classes
"""

import pytest
import sys
import time
import threading
from pathlib import Path
from unittest.mock import Mock, patch, MagicMock
from dataclasses import dataclass

# Add current directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

# Import from loader which handles the module loading
from loader import code_module, RESOURCES_DIR, read_resource_file

# Import parser utilities
parser_utils = sys.modules.get("proximadb.chunking_strategies.parser_utils")
if parser_utils is None:
    # Load it manually
    import importlib.util

    src_path = Path(__file__).parent.parent.parent / "src"
    spec = importlib.util.spec_from_file_location(
        "proximadb.chunking_strategies.parser_utils",
        str(src_path / "proximadb" / "chunking_strategies" / "parser_utils.py"),
    )
    parser_utils = importlib.util.module_from_spec(spec)
    sys.modules["proximadb.chunking_strategies.parser_utils"] = parser_utils
    spec.loader.exec_module(parser_utils)

# Get references
ParserError = parser_utils.ParserError
ParserInitializationError = parser_utils.ParserInitializationError
ParseError = parser_utils.ParseError
UnsupportedLanguageError = parser_utils.UnsupportedLanguageError
FallbackStrategy = parser_utils.FallbackStrategy
FallbackConfig = parser_utils.FallbackConfig
ParserMetrics = parser_utils.ParserMetrics
MetricsCollector = parser_utils.MetricsCollector
get_metrics_collector = parser_utils.get_metrics_collector
ParserCache = parser_utils.ParserCache
get_parser_cache = parser_utils.get_parser_cache
ParserPlugin = parser_utils.ParserPlugin
ParserPluginRegistry = parser_utils.ParserPluginRegistry
get_plugin_registry = parser_utils.get_plugin_registry
ValidationResult = parser_utils.ValidationResult
ConfigValidator = parser_utils.ConfigValidator
BaseLanguageParser = parser_utils.BaseLanguageParser
CFamilyParser = parser_utils.CFamilyParser
JVMFamilyParser = parser_utils.JVMFamilyParser
DynamicLanguageParser = parser_utils.DynamicLanguageParser
detect_language_from_content = parser_utils.detect_language_from_content


class TestParserErrors:
    """Test parser error classes."""

    def test_parser_error_basic(self):
        """Test basic ParserError."""
        error = ParserError("Test error")
        assert str(error) == "Test error"
        assert error.language is None
        assert error.file_path is None

    def test_parser_error_with_context(self):
        """Test ParserError with context."""
        error = ParserError(
            "Parse failed", language="python", file_path="/path/to/file.py"
        )
        assert error.language == "python"
        assert error.file_path == "/path/to/file.py"

    def test_parser_initialization_error(self):
        """Test ParserInitializationError."""
        error = ParserInitializationError("Tree-sitter not available", language="rust")
        assert isinstance(error, ParserError)
        assert error.language == "rust"

    def test_parse_error_with_location(self):
        """Test ParseError with line/column info."""
        error = ParseError(
            "Syntax error", line=42, column=10, language="python", file_path="test.py"
        )
        assert error.line == 42
        assert error.column == 10
        assert error.language == "python"

    def test_unsupported_language_error(self):
        """Test UnsupportedLanguageError."""
        error = UnsupportedLanguageError("Language not supported", language="brainfuck")
        assert isinstance(error, ParserError)
        assert error.language == "brainfuck"


class TestFallbackStrategy:
    """Test fallback strategy enum and config."""

    def test_fallback_strategy_values(self):
        """Test all fallback strategy values exist."""
        assert FallbackStrategy.NONE
        assert FallbackStrategy.REGEX
        assert FallbackStrategy.SEMANTIC
        assert FallbackStrategy.EMPTY
        assert FallbackStrategy.PARTIAL

    def test_fallback_config_defaults(self):
        """Test FallbackConfig default values."""
        config = FallbackConfig()
        assert config.strategy == FallbackStrategy.REGEX
        assert config.max_retries == 1
        assert config.retry_delay_ms == 100
        assert config.log_errors is True
        assert config.collect_metrics is True

    def test_fallback_config_custom(self):
        """Test FallbackConfig with custom values."""
        config = FallbackConfig(
            strategy=FallbackStrategy.SEMANTIC,
            max_retries=3,
            retry_delay_ms=500,
            log_errors=False,
        )
        assert config.strategy == FallbackStrategy.SEMANTIC
        assert config.max_retries == 3
        assert config.retry_delay_ms == 500
        assert config.log_errors is False


class TestParserMetrics:
    """Test parser metrics collection."""

    def test_metrics_creation(self):
        """Test ParserMetrics creation."""
        metrics = ParserMetrics(language="python", file_path="test.py")
        assert metrics.language == "python"
        assert metrics.file_path == "test.py"
        assert metrics.parse_time_ms == 0.0
        assert metrics.symbol_count == 0
        assert metrics.error_count == 0

    def test_metrics_with_values(self):
        """Test ParserMetrics with values."""
        metrics = ParserMetrics(
            language="rust",
            file_path="test.rs",
            parse_time_ms=15.5,
            symbol_count=10,
            relation_count=5,
            fallback_used=True,
            tree_sitter_available=True,
        )
        assert metrics.parse_time_ms == 15.5
        assert metrics.symbol_count == 10
        assert metrics.relation_count == 5
        assert metrics.fallback_used is True

    def test_metrics_to_dict(self):
        """Test ParserMetrics to_dict method."""
        metrics = ParserMetrics(
            language="go", file_path="test.go", parse_time_ms=12.345, symbol_count=5
        )
        d = metrics.to_dict()
        assert isinstance(d, dict)
        assert d["language"] == "go"
        assert d["file_path"] == "test.go"
        assert d["parse_time_ms"] == 12.35  # Rounded
        assert d["symbol_count"] == 5


class TestMetricsCollector:
    """Test MetricsCollector singleton."""

    def setup_method(self):
        """Clear metrics before each test."""
        collector = get_metrics_collector()
        collector.clear()

    def test_singleton_pattern(self):
        """Test MetricsCollector is singleton."""
        collector1 = get_metrics_collector()
        collector2 = get_metrics_collector()
        assert collector1 is collector2

    def test_record_metrics(self):
        """Test recording metrics."""
        collector = get_metrics_collector()
        metrics = ParserMetrics(language="python", file_path="test.py")
        collector.record(metrics)

        summary = collector.get_summary()
        assert "python" in summary

    def test_get_summary(self):
        """Test getting metrics summary."""
        collector = get_metrics_collector()

        # Record multiple metrics
        for i in range(5):
            metrics = ParserMetrics(
                language="python",
                file_path=f"test{i}.py",
                parse_time_ms=10.0 + i,
                symbol_count=i * 2,
            )
            collector.record(metrics)

        summary = collector.get_summary()
        assert summary["python"]["total_parses"] == 5
        assert summary["python"]["avg_parse_time_ms"] == 12.0  # (10+11+12+13+14)/5
        assert summary["python"]["total_symbols"] == 20  # 0+2+4+6+8

    def test_clear_metrics(self):
        """Test clearing metrics."""
        collector = get_metrics_collector()
        collector.record(ParserMetrics(language="python", file_path="test.py"))
        collector.clear()

        summary = collector.get_summary()
        assert summary == {}

    def test_enable_disable(self):
        """Test enabling/disabling metrics collection."""
        collector = get_metrics_collector()
        collector.disable()
        collector.record(ParserMetrics(language="python", file_path="test.py"))

        summary = collector.get_summary()
        assert summary == {}

        collector.enable()
        collector.record(ParserMetrics(language="python", file_path="test.py"))
        summary = collector.get_summary()
        assert "python" in summary


class TestParserCache:
    """Test parser cache."""

    def setup_method(self):
        """Clear cache before each test."""
        cache = get_parser_cache()
        cache.clear()

    def test_singleton_pattern(self):
        """Test ParserCache is singleton."""
        cache1 = get_parser_cache()
        cache2 = get_parser_cache()
        assert cache1 is cache2

    def test_put_and_get(self):
        """Test putting and getting parsers."""
        cache = get_parser_cache()
        mock_parser = Mock()

        cache.put("python", mock_parser)
        result = cache.get("python")

        assert result is mock_parser

    def test_get_nonexistent(self):
        """Test getting nonexistent parser."""
        cache = get_parser_cache()
        result = cache.get("nonexistent")
        assert result is None

    def test_contains(self):
        """Test contains check."""
        cache = get_parser_cache()
        cache.put("python", Mock())

        assert cache.contains("python")
        assert not cache.contains("rust")

    def test_size(self):
        """Test cache size."""
        cache = get_parser_cache()
        cache.put("python", Mock())
        cache.put("rust", Mock())

        assert cache.size() == 2

    def test_clear(self):
        """Test clearing cache."""
        cache = get_parser_cache()
        cache.put("python", Mock())
        cache.put("rust", Mock())
        cache.clear()

        assert cache.size() == 0
        assert cache.get("python") is None

    def test_lru_eviction(self):
        """Test LRU eviction when cache is full."""
        # Create a cache with small max size
        cache = get_parser_cache()
        cache._max_size = 3
        cache.clear()

        # Fill cache
        cache.put("a", Mock())
        cache.put("b", Mock())
        cache.put("c", Mock())

        # Access 'a' to make it recently used
        cache.get("a")

        # Add new item, should evict 'b' (least recently used)
        cache.put("d", Mock())

        assert cache.contains("a")  # Recently accessed
        assert not cache.contains("b")  # Evicted
        assert cache.contains("c")
        assert cache.contains("d")

    def test_thread_safety(self):
        """Test thread-safe cache access."""
        cache = get_parser_cache()
        cache.clear()
        cache._max_size = 100  # Ensure no eviction during test

        results = []
        lock = threading.Lock()

        def cache_operation(lang):
            mock = Mock()
            cache.put(lang, mock)
            time.sleep(0.001)
            result = cache.get(lang)
            with lock:
                results.append(result is not None)

        threads = [
            threading.Thread(target=cache_operation, args=(f"threadsafe{i}",))
            for i in range(10)
        ]

        for t in threads:
            t.start()
        for t in threads:
            t.join()

        assert all(results)


class TestParserPlugin:
    """Test ParserPlugin class."""

    def test_plugin_creation(self):
        """Test creating a plugin."""
        mock_parser_class = Mock()

        plugin = ParserPlugin(
            name="test-plugin",
            parser_class=mock_parser_class,
            languages=["python", "cython"],
            extensions=[".py", ".pyx"],
            priority=10,
            metadata={"version": "1.0"},
        )

        assert plugin.name == "test-plugin"
        assert plugin.languages == ["python", "cython"]
        assert plugin.extensions == [".py", ".pyx"]
        assert plugin.priority == 10
        assert plugin.metadata["version"] == "1.0"

    def test_supports_language(self):
        """Test language support check."""
        plugin = ParserPlugin(
            name="test",
            parser_class=Mock(),
            languages=["Python", "CYTHON"],
            extensions=[".py"],
        )

        assert plugin.supports_language("python")
        assert plugin.supports_language("Python")
        assert plugin.supports_language("CYTHON")
        assert not plugin.supports_language("rust")

    def test_supports_extension(self):
        """Test extension support check."""
        plugin = ParserPlugin(
            name="test",
            parser_class=Mock(),
            languages=["python"],
            extensions=[".py", ".pyi"],
        )

        assert plugin.supports_extension(".py")
        assert plugin.supports_extension("py")  # Without dot
        assert plugin.supports_extension(".PY")  # Case insensitive
        assert not plugin.supports_extension(".rs")

    def test_get_parser(self):
        """Test getting parser instance."""
        mock_parser_class = Mock(return_value=Mock())

        plugin = ParserPlugin(
            name="test",
            parser_class=mock_parser_class,
            languages=["python"],
            extensions=[".py"],
        )

        parser1 = plugin.get_parser()
        parser2 = plugin.get_parser()

        # Should return same instance (lazy initialization)
        assert parser1 is parser2
        mock_parser_class.assert_called_once()


class TestParserPluginRegistry:
    """Test ParserPluginRegistry."""

    def setup_method(self):
        """Get fresh registry state."""
        # Note: Registry is singleton, so we need to unregister test plugins
        registry = get_plugin_registry()
        for name in list(registry._plugins.keys()):
            if name.startswith("test-"):
                registry.unregister(name)

    def test_singleton_pattern(self):
        """Test registry is singleton."""
        reg1 = get_plugin_registry()
        reg2 = get_plugin_registry()
        assert reg1 is reg2

    def test_register_plugin(self):
        """Test registering a plugin."""
        registry = get_plugin_registry()
        plugin = ParserPlugin(
            name="test-register",
            parser_class=Mock(),
            languages=["testlang"],
            extensions=[".test"],
        )

        result = registry.register(plugin)
        assert result is True

        # Verify plugin is registered
        plugins = registry.list_plugins()
        names = [p["name"] for p in plugins]
        assert "test-register" in names

    def test_register_duplicate(self):
        """Test registering duplicate plugin fails."""
        registry = get_plugin_registry()
        plugin = ParserPlugin(
            name="test-duplicate",
            parser_class=Mock(),
            languages=["testlang"],
            extensions=[".test"],
        )

        registry.register(plugin)
        result = registry.register(plugin)

        assert result is False

    def test_unregister_plugin(self):
        """Test unregistering a plugin."""
        registry = get_plugin_registry()
        plugin = ParserPlugin(
            name="test-unregister",
            parser_class=Mock(),
            languages=["testlang"],
            extensions=[".test"],
        )

        registry.register(plugin)
        result = registry.unregister("test-unregister")

        assert result is True
        assert "testlang" not in registry.get_supported_languages()

    def test_unregister_nonexistent(self):
        """Test unregistering nonexistent plugin."""
        registry = get_plugin_registry()
        result = registry.unregister("nonexistent-plugin")
        assert result is False

    def test_get_parser_for_language(self):
        """Test getting parser by language."""
        registry = get_plugin_registry()
        mock_parser = Mock()

        plugin = ParserPlugin(
            name="test-by-lang",
            parser_class=Mock(return_value=mock_parser),
            languages=["testbylang"],
            extensions=[".tbl"],
        )
        registry.register(plugin)

        parser = registry.get_parser_for_language("testbylang")
        assert parser is mock_parser

    def test_get_parser_for_extension(self):
        """Test getting parser by extension."""
        registry = get_plugin_registry()
        mock_parser = Mock()

        plugin = ParserPlugin(
            name="test-by-ext",
            parser_class=Mock(return_value=mock_parser),
            languages=["testbyext"],
            extensions=[".tbe"],
        )
        registry.register(plugin)

        parser = registry.get_parser_for_extension(".tbe")
        assert parser is mock_parser

    def test_priority_ordering(self):
        """Test plugins are ordered by priority."""
        registry = get_plugin_registry()

        low_priority_parser = Mock()
        high_priority_parser = Mock()

        low_plugin = ParserPlugin(
            name="test-low-priority",
            parser_class=Mock(return_value=low_priority_parser),
            languages=["prioritylang"],
            extensions=[".pri"],
            priority=1,
        )
        high_plugin = ParserPlugin(
            name="test-high-priority",
            parser_class=Mock(return_value=high_priority_parser),
            languages=["prioritylang"],
            extensions=[".pri"],
            priority=10,
        )

        registry.register(low_plugin)
        registry.register(high_plugin)

        # High priority should be returned
        parser = registry.get_parser_for_language("prioritylang")
        assert parser is high_priority_parser


class TestConfigValidator:
    """Test configuration validation."""

    def test_validate_chunk_size_valid(self):
        """Test valid chunk size."""
        result = ConfigValidator.validate_chunk_size(1000)
        assert result.valid is True
        assert len(result.errors) == 0

    def test_validate_chunk_size_too_small(self):
        """Test chunk size below minimum."""
        result = ConfigValidator.validate_chunk_size(50, min_chunk_size=100)
        assert result.valid is False
        assert any("must be >=" in e for e in result.errors)

    def test_validate_chunk_size_too_large(self):
        """Test chunk size above maximum."""
        result = ConfigValidator.validate_chunk_size(200000, max_chunk_size=100000)
        assert result.valid is False
        assert any("must be <=" in e for e in result.errors)

    def test_validate_chunk_size_warning_small(self):
        """Test warning for very small chunk size."""
        result = ConfigValidator.validate_chunk_size(50)
        assert result.valid is True
        assert any("very small" in w for w in result.warnings)

    def test_validate_chunk_size_warning_large(self):
        """Test warning for large chunk size."""
        result = ConfigValidator.validate_chunk_size(15000)
        assert result.valid is True
        assert any("large" in w for w in result.warnings)

    def test_validate_overlap_valid(self):
        """Test valid overlap configuration."""
        result = ConfigValidator.validate_overlap(100, 1000)
        assert result.valid is True

    def test_validate_overlap_negative(self):
        """Test negative overlap."""
        result = ConfigValidator.validate_overlap(-10, 1000)
        assert result.valid is False

    def test_validate_overlap_too_large(self):
        """Test overlap >= chunk_size."""
        result = ConfigValidator.validate_overlap(1000, 1000)
        assert result.valid is False

    def test_validate_overlap_warning_high(self):
        """Test warning for high overlap percentage."""
        result = ConfigValidator.validate_overlap(600, 1000)
        assert result.valid is True
        assert any(">50%" in w for w in result.warnings)

    def test_validate_config_complete(self):
        """Test complete config validation."""

        @dataclass
        class MockConfig:
            chunk_size: int = 1000
            chunk_overlap: int = 100
            min_chunk_size: int = 50
            max_chunk_size: int = 10000

        config = MockConfig()
        result = ConfigValidator.validate_config(config)
        assert result.valid is True


class TestValidationResult:
    """Test ValidationResult class."""

    def test_valid_result(self):
        """Test valid result."""
        result = ValidationResult(valid=True)
        assert result.valid is True
        assert result.errors == []
        assert result.warnings == []

    def test_invalid_result(self):
        """Test invalid result with errors."""
        result = ValidationResult(valid=False, errors=["Error 1", "Error 2"])
        assert result.valid is False
        assert len(result.errors) == 2

    def test_result_with_warnings(self):
        """Test result with warnings."""
        result = ValidationResult(valid=True, warnings=["Warning 1"])
        assert result.valid is True
        assert len(result.warnings) == 1


class TestDetectLanguageFromContent:
    """Test language detection from content."""

    def test_detect_python_shebang(self):
        """Test detecting Python from shebang."""
        content = "#!/usr/bin/env python3\ndef hello(): pass"
        lang = detect_language_from_content(content)
        assert lang == "python"

    def test_detect_node_shebang(self):
        """Test detecting JavaScript from Node shebang."""
        content = "#!/usr/bin/env node\nconsole.log('hello')"
        lang = detect_language_from_content(content)
        assert lang == "javascript"

    def test_detect_ruby_shebang(self):
        """Test detecting Ruby from shebang."""
        content = "#!/usr/bin/ruby\nputs 'hello'"
        lang = detect_language_from_content(content)
        assert lang == "ruby"

    def test_detect_bash_shebang(self):
        """Test detecting Bash from shebang."""
        content = "#!/bin/bash\necho 'hello'"
        lang = detect_language_from_content(content)
        assert lang == "bash"

    def test_detect_go_pattern(self):
        """Test detecting Go from patterns."""
        content = 'package main\n\nfunc main() {\n    fmt.Println("hello")\n}'
        lang = detect_language_from_content(content)
        assert lang == "go"

    def test_detect_rust_pattern(self):
        """Test detecting Rust from patterns."""
        content = 'fn main() {\n    println!("hello");\n}'
        lang = detect_language_from_content(content)
        assert lang == "rust"

    def test_detect_java_pattern(self):
        """Test detecting Java from patterns."""
        content = (
            "public class Main {\n    public static void main(String[] args) {}\n}"
        )
        lang = detect_language_from_content(content)
        assert lang == "java"

    def test_detect_python_pattern(self):
        """Test detecting Python from patterns."""
        content = "def hello():\n    print('hello')"
        lang = detect_language_from_content(content)
        assert lang == "python"

    def test_detect_unknown(self):
        """Test returning None for unknown content."""
        content = "some random text without code patterns"
        lang = detect_language_from_content(content)
        assert lang is None


class TestParserBaseClasses:
    """Test parser base classes."""

    def test_base_language_parser_abstract(self):
        """Test BaseLanguageParser is abstract."""
        with pytest.raises(TypeError):
            BaseLanguageParser()

    def test_cfamily_parser_abstract(self):
        """Test CFamilyParser is abstract."""
        with pytest.raises(TypeError):
            CFamilyParser()

    def test_jvm_family_parser_abstract(self):
        """Test JVMFamilyParser is abstract."""
        with pytest.raises(TypeError):
            JVMFamilyParser()

    def test_dynamic_language_parser_abstract(self):
        """Test DynamicLanguageParser is abstract."""
        with pytest.raises(TypeError):
            DynamicLanguageParser()


class TestCFamilyParserHelpers:
    """Test C-family parser helper methods."""

    def test_find_matching_brace(self):
        """Test finding matching brace."""

        class TestParser(CFamilyParser):
            @property
            def language(self):
                return "test"

            @property
            def file_extensions(self):
                return [".test"]

            def parse(self, content, file_path):
                pass

            def _fallback_regex_parse(self, content, file_path):
                pass

        parser = TestParser()
        content = "{ inner { nested } outer }"
        end = parser._find_matching_brace(content, 0)

        assert end == len(content)

    def test_find_matching_brace_with_strings(self):
        """Test matching brace ignores braces in strings."""

        class TestParser(CFamilyParser):
            @property
            def language(self):
                return "test"

            @property
            def file_extensions(self):
                return [".test"]

            def parse(self, content, file_path):
                pass

            def _fallback_regex_parse(self, content, file_path):
                pass

        parser = TestParser()
        content = '{ "string with { brace" }'
        end = parser._find_matching_brace(content, 0)

        assert content[end - 1] == "}"


class TestJVMFamilyParserHelpers:
    """Test JVM-family parser helper methods."""

    def test_extract_package(self):
        """Test package extraction."""

        class TestParser(JVMFamilyParser):
            @property
            def language(self):
                return "test"

            @property
            def file_extensions(self):
                return [".test"]

            def parse(self, content, file_path):
                pass

            def _fallback_regex_parse(self, content, file_path):
                pass

        parser = TestParser()
        content = "package com.example.myapp;\n\nclass Test {}"
        package = parser._extract_package(content)

        assert package == "com.example.myapp"

    def test_extract_imports(self):
        """Test import extraction."""

        class TestParser(JVMFamilyParser):
            @property
            def language(self):
                return "test"

            @property
            def file_extensions(self):
                return [".test"]

            def parse(self, content, file_path):
                pass

            def _fallback_regex_parse(self, content, file_path):
                pass

        parser = TestParser()
        content = """
import java.util.List;
import java.util.Map;
import static java.util.Collections.*;
"""
        imports = parser._extract_imports(content)

        assert "java.util.List" in imports
        assert "java.util.Map" in imports
        assert "java.util.Collections.*" in imports


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
