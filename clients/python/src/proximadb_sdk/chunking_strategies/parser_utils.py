"""
Parser utilities and enhancements for code chunking.

This module provides:
- Parser family base classes to reduce duplication
- Parser instance caching with LRU eviction
- Robust error handling with fallback strategies
- Plugin architecture for external parsers
- Configuration validation
- Performance metrics and monitoring

Design Patterns Used:
- Template Method: Parser family base classes define parsing skeleton
- Singleton: Parser cache ensures single instance per language
- Strategy: Fallback strategies for error handling
- Factory: Plugin registry creates parser instances
- Decorator: Performance monitoring and error handling
"""

import hashlib
import logging
import re
import threading
import time
from abc import ABC, abstractmethod
from contextlib import contextmanager
from dataclasses import dataclass, field
from enum import Enum, auto
from functools import lru_cache, wraps
from typing import (
    Any,
    Callable,
    Dict,
    Generic,
    List,
    Optional,
    Set,
    Tuple,
    Type,
    TypeVar,
    Union,
)
from weakref import WeakValueDictionary

# Configure logging
logger = logging.getLogger(__name__)


# =============================================================================
# Error Handling
# =============================================================================


class ParserError(Exception):
    """Base exception for parser errors"""

    def __init__(self, message: str, language: str = None, file_path: str = None):
        self.language = language
        self.file_path = file_path
        super().__init__(message)


class ParserInitializationError(ParserError):
    """Raised when parser fails to initialize"""

    pass


class ParseError(ParserError):
    """Raised when parsing fails"""

    def __init__(self, message: str, line: int = None, column: int = None, **kwargs):
        self.line = line
        self.column = column
        super().__init__(message, **kwargs)


class UnsupportedLanguageError(ParserError):
    """Raised when language is not supported"""

    pass


# =============================================================================
# Fallback Strategy Pattern
# =============================================================================


class FallbackStrategy(Enum):
    """Strategies for handling parser failures"""

    NONE = auto()  # No fallback, raise exception
    REGEX = auto()  # Use regex-based parsing
    SEMANTIC = auto()  # Fall back to semantic text chunking
    EMPTY = auto()  # Return empty result
    PARTIAL = auto()  # Return partial results on error


@dataclass
class FallbackConfig:
    """Configuration for fallback behavior"""

    strategy: FallbackStrategy = FallbackStrategy.REGEX
    max_retries: int = 1
    retry_delay_ms: int = 100
    log_errors: bool = True
    collect_metrics: bool = True


# =============================================================================
# Performance Metrics
# =============================================================================


@dataclass
class ParserMetrics:
    """Metrics collected during parsing"""

    language: str
    file_path: str
    parse_time_ms: float = 0.0
    symbol_count: int = 0
    relation_count: int = 0
    error_count: int = 0
    fallback_used: bool = False
    cache_hit: bool = False
    tree_sitter_available: bool = False

    def to_dict(self) -> Dict[str, Any]:
        return {
            "language": self.language,
            "file_path": self.file_path,
            "parse_time_ms": round(self.parse_time_ms, 2),
            "symbol_count": self.symbol_count,
            "relation_count": self.relation_count,
            "error_count": self.error_count,
            "fallback_used": self.fallback_used,
            "cache_hit": self.cache_hit,
            "tree_sitter_available": self.tree_sitter_available,
        }


class MetricsCollector:
    """Collects and aggregates parser metrics"""

    _instance = None
    _lock = threading.Lock()

    def __new__(cls):
        if cls._instance is None:
            with cls._lock:
                if cls._instance is None:
                    cls._instance = super().__new__(cls)
                    cls._instance._metrics: List[ParserMetrics] = []
                    cls._instance._enabled = True
        return cls._instance

    def record(self, metrics: ParserMetrics):
        """Record metrics for a parse operation"""
        if self._enabled:
            self._metrics.append(metrics)

    def get_summary(self) -> Dict[str, Any]:
        """Get aggregated metrics summary"""
        if not self._metrics:
            return {}

        by_language: Dict[str, List[ParserMetrics]] = {}
        for m in self._metrics:
            if m.language not in by_language:
                by_language[m.language] = []
            by_language[m.language].append(m)

        summary = {}
        for lang, metrics in by_language.items():
            summary[lang] = {
                "total_parses": len(metrics),
                "avg_parse_time_ms": sum(m.parse_time_ms for m in metrics)
                / len(metrics),
                "total_symbols": sum(m.symbol_count for m in metrics),
                "total_relations": sum(m.relation_count for m in metrics),
                "error_rate": sum(1 for m in metrics if m.error_count > 0)
                / len(metrics),
                "fallback_rate": sum(1 for m in metrics if m.fallback_used)
                / len(metrics),
                "cache_hit_rate": sum(1 for m in metrics if m.cache_hit) / len(metrics),
            }
        return summary

    def clear(self):
        """Clear collected metrics"""
        self._metrics.clear()

    def enable(self):
        self._enabled = True

    def disable(self):
        self._enabled = False


def get_metrics_collector() -> MetricsCollector:
    """Get the singleton metrics collector"""
    return MetricsCollector()


# =============================================================================
# Parser Caching
# =============================================================================


class ParserCache:
    """
    Thread-safe LRU cache for parser instances.

    Ensures single parser instance per language to avoid
    repeated initialization overhead.
    """

    _instance = None
    _lock = threading.Lock()

    def __new__(cls, max_size: int = 32):
        if cls._instance is None:
            with cls._lock:
                if cls._instance is None:
                    cls._instance = super().__new__(cls)
                    cls._instance._cache: Dict[str, Any] = {}
                    cls._instance._access_order: List[str] = []
                    cls._instance._max_size = max_size
                    cls._instance._cache_lock = threading.RLock()
        return cls._instance

    def get(self, language: str) -> Optional[Any]:
        """Get cached parser instance"""
        with self._cache_lock:
            if language in self._cache:
                # Update access order (LRU)
                self._access_order.remove(language)
                self._access_order.append(language)
                return self._cache[language]
            return None

    def put(self, language: str, parser: Any):
        """Cache parser instance"""
        with self._cache_lock:
            # Remove from access order if already present
            if language in self._cache:
                if language in self._access_order:
                    self._access_order.remove(language)
            elif len(self._cache) >= self._max_size:
                # Evict least recently used
                if self._access_order:
                    lru_key = self._access_order.pop(0)
                    if lru_key in self._cache:
                        del self._cache[lru_key]
                    logger.debug(f"Evicted parser from cache: {lru_key}")

            self._cache[language] = parser
            self._access_order.append(language)

    def clear(self):
        """Clear the cache"""
        with self._cache_lock:
            self._cache.clear()
            self._access_order.clear()

    def size(self) -> int:
        """Get current cache size"""
        return len(self._cache)

    def contains(self, language: str) -> bool:
        """Check if parser is cached"""
        return language in self._cache


def get_parser_cache() -> ParserCache:
    """Get the singleton parser cache"""
    return ParserCache()


# =============================================================================
# Decorators for Error Handling and Metrics
# =============================================================================


def with_metrics(func: Callable) -> Callable:
    """Decorator to collect parsing metrics"""

    @wraps(func)
    def wrapper(self, content: str, file_path: str, *args, **kwargs):
        collector = get_metrics_collector()
        start_time = time.perf_counter()

        metrics = ParserMetrics(
            language=getattr(self, "language", "unknown"),
            file_path=file_path,
            tree_sitter_available=getattr(self, "_parser", None) is not None,
        )

        try:
            result = func(self, content, file_path, *args, **kwargs)

            # Extract metrics from result
            if hasattr(result, "symbols"):
                metrics.symbol_count = len(result.symbols)
            if hasattr(result, "relations"):
                metrics.relation_count = len(result.relations)

            return result
        except Exception as e:
            metrics.error_count += 1
            raise
        finally:
            metrics.parse_time_ms = (time.perf_counter() - start_time) * 1000
            collector.record(metrics)

    return wrapper


def with_fallback(fallback_config: FallbackConfig = None) -> Callable:
    """Decorator to add fallback behavior on parse errors"""
    config = fallback_config or FallbackConfig()

    def decorator(func: Callable) -> Callable:
        @wraps(func)
        def wrapper(self, content: str, file_path: str, *args, **kwargs):
            last_error = None

            for attempt in range(config.max_retries + 1):
                try:
                    return func(self, content, file_path, *args, **kwargs)
                except Exception as e:
                    last_error = e
                    if config.log_errors:
                        logger.warning(
                            f"Parse attempt {attempt + 1} failed for {file_path}: {e}"
                        )

                    if attempt < config.max_retries:
                        time.sleep(config.retry_delay_ms / 1000)

            # All retries exhausted, apply fallback strategy
            if config.strategy == FallbackStrategy.NONE:
                raise last_error

            elif config.strategy == FallbackStrategy.REGEX:
                return self._fallback_regex_parse(content, file_path)

            elif config.strategy == FallbackStrategy.SEMANTIC:
                return self._fallback_semantic_parse(content, file_path)

            elif config.strategy == FallbackStrategy.EMPTY:
                return self._create_empty_result(file_path)

            elif config.strategy == FallbackStrategy.PARTIAL:
                # Try to return whatever was parsed before error
                if hasattr(self, "_partial_result"):
                    return self._partial_result
                return self._create_empty_result(file_path)

            raise last_error

        return wrapper

    return decorator


def cached_parser(func: Callable) -> Callable:
    """Decorator to use cached parser instances"""

    @wraps(func)
    def wrapper(self, *args, **kwargs):
        cache = get_parser_cache()
        language = getattr(self, "language", "unknown")

        cached = cache.get(language)
        if cached is not None:
            # Use cached parser instance
            self._parser = cached._parser
            self._language = cached._language
            return func(self, *args, **kwargs)

        result = func(self, *args, **kwargs)
        cache.put(language, self)
        return result

    return wrapper


# =============================================================================
# Parser Family Base Classes
# =============================================================================


class BaseLanguageParser(ABC):
    """
    Enhanced base class for language parsers with common functionality.

    Provides:
    - Tree-sitter initialization with fallback
    - Common regex patterns
    - Metrics collection
    - Error handling
    """

    def __init__(self):
        self._parser = None
        self._language_binding = None
        self._fallback_config = FallbackConfig()
        self._init_tree_sitter()

    @property
    @abstractmethod
    def language(self) -> str:
        """Language identifier"""
        pass

    @property
    @abstractmethod
    def file_extensions(self) -> List[str]:
        """Supported file extensions"""
        pass

    @property
    def tree_sitter_language_name(self) -> str:
        """Tree-sitter language name (may differ from language property)"""
        return self.language

    def _init_tree_sitter(self):
        """Initialize tree-sitter parser"""
        try:
            from tree_sitter_language_pack import get_language, get_parser

            self._parser = get_parser(self.tree_sitter_language_name)
            self._language_binding = get_language(self.tree_sitter_language_name)
            logger.debug(f"Tree-sitter initialized for {self.language}")
        except ImportError:
            logger.info(
                f"Tree-sitter not available for {self.language}, using regex fallback"
            )
            self._parser = None
            self._language_binding = None
        except Exception as e:
            logger.warning(f"Tree-sitter init failed for {self.language}: {e}")
            self._parser = None
            self._language_binding = None

    @property
    def has_tree_sitter(self) -> bool:
        """Check if tree-sitter is available"""
        return self._parser is not None

    @abstractmethod
    def parse(self, content: str, file_path: str) -> "ParsedCode":
        """Parse content and extract symbols/relations"""
        pass

    @abstractmethod
    def _fallback_regex_parse(self, content: str, file_path: str) -> "ParsedCode":
        """Fallback parsing using regex patterns"""
        pass

    def _fallback_semantic_parse(self, content: str, file_path: str) -> "ParsedCode":
        """Fallback to semantic text chunking"""
        # Import here to avoid circular dependency
        from .code import ParsedCode

        return ParsedCode(
            file_path=file_path,
            language=self.language,
            symbols=[],
            relations=[],
            imports=[],
            content_hash=hashlib.sha256(content.encode()).hexdigest(),
        )

    def _create_empty_result(self, file_path: str) -> "ParsedCode":
        """Create empty parse result"""
        from .code import ParsedCode

        return ParsedCode(
            file_path=file_path,
            language=self.language,
            symbols=[],
            relations=[],
            imports=[],
            content_hash="",
        )

    def _compute_content_hash(self, content: str) -> str:
        """Compute hash of content for change detection"""
        return hashlib.sha256(content.encode("utf-8")).hexdigest()


class CFamilyParser(BaseLanguageParser):
    """
    Base parser for C-family languages (C, C++, C#, Java, JavaScript, TypeScript).

    Provides common patterns for:
    - Braced block detection
    - Function/method parsing
    - Class/struct/interface parsing
    - Preprocessor handling (C/C++)
    """

    # Common regex patterns for C-family languages
    BLOCK_START = re.compile(r"\{")
    BLOCK_END = re.compile(r"\}")

    FUNCTION_PATTERN = re.compile(
        r"(?:(?:public|private|protected|static|async|virtual|override|abstract)\s+)*"
        r"(?:[\w<>\[\],\s]+)\s+"  # Return type
        r"(\w+)\s*"  # Function name
        r"\([^)]*\)\s*"  # Parameters
        r"(?:throws\s+[\w,\s]+)?\s*"  # Throws clause (Java)
        r"\{",
        re.MULTILINE,
    )

    CLASS_PATTERN = re.compile(
        r"(?:(?:public|private|protected|abstract|final|static)\s+)*"
        r"(?:class|struct|interface|enum)\s+"
        r"(\w+)",
        re.MULTILINE,
    )

    def _find_matching_brace(self, content: str, start: int) -> int:
        """Find the matching closing brace for an opening brace"""
        depth = 1
        i = start + 1
        in_string = False
        string_char = None

        while i < len(content) and depth > 0:
            char = content[i]

            # Handle strings
            if char in "\"'`" and (i == 0 or content[i - 1] != "\\"):
                if not in_string:
                    in_string = True
                    string_char = char
                elif char == string_char:
                    in_string = False

            if not in_string:
                if char == "{":
                    depth += 1
                elif char == "}":
                    depth -= 1

            i += 1

        return i if depth == 0 else -1

    def _extract_block(self, content: str, start: int) -> Tuple[str, int]:
        """Extract a braced block starting at given position"""
        brace_pos = content.find("{", start)
        if brace_pos == -1:
            return "", -1

        end = self._find_matching_brace(content, brace_pos)
        if end == -1:
            return "", -1

        return content[brace_pos:end], end


class JVMFamilyParser(CFamilyParser):
    """
    Base parser for JVM languages (Java, Kotlin, Scala, Groovy).

    Extends C-family with:
    - Package declarations
    - Import statements
    - Annotations
    - Generics handling
    """

    PACKAGE_PATTERN = re.compile(r"package\s+([\w.]+)\s*;?")
    IMPORT_PATTERN = re.compile(r"import\s+(?:static\s+)?([\w.*]+)\s*;?")
    ANNOTATION_PATTERN = re.compile(r"@(\w+)(?:\([^)]*\))?")

    def _extract_package(self, content: str) -> Optional[str]:
        """Extract package declaration"""
        match = self.PACKAGE_PATTERN.search(content)
        return match.group(1) if match else None

    def _extract_imports(self, content: str) -> List[str]:
        """Extract import statements"""
        return [m.group(1) for m in self.IMPORT_PATTERN.finditer(content)]

    def _extract_annotations(self, content: str, pos: int) -> List[str]:
        """Extract annotations before a symbol"""
        # Look backwards from pos for annotations
        annotations = []
        lines = content[:pos].split("\n")
        for line in reversed(lines[-5:]):  # Check last 5 lines
            line = line.strip()
            if line.startswith("@"):
                match = self.ANNOTATION_PATTERN.match(line)
                if match:
                    annotations.append(match.group(1))
            elif line and not line.startswith("//") and not line.startswith("/*"):
                break
        return list(reversed(annotations))


class DynamicLanguageParser(BaseLanguageParser):
    """
    Base parser for dynamic languages (Python, Ruby, JavaScript, PHP).

    Provides patterns for:
    - Indentation-based blocks (Python, Ruby)
    - Dynamic method definition
    - Module/mixin handling
    """

    # Python-style patterns
    PYTHON_FUNCTION_PATTERN = re.compile(
        r"^(\s*)(?:async\s+)?def\s+(\w+)\s*\([^)]*\)\s*(?:->.*?)?:", re.MULTILINE
    )
    PYTHON_CLASS_PATTERN = re.compile(
        r"^(\s*)class\s+(\w+)(?:\([^)]*\))?\s*:", re.MULTILINE
    )

    # Ruby-style patterns
    RUBY_METHOD_PATTERN = re.compile(
        r"^(\s*)def\s+(?:self\.)?(\w+[?!=]?)", re.MULTILINE
    )
    RUBY_CLASS_PATTERN = re.compile(r"^(\s*)class\s+(\w+)(?:\s*<\s*\w+)?", re.MULTILINE)

    def _find_indentation_block_end(
        self, content: str, start: int, base_indent: int
    ) -> int:
        """Find end of indentation-based block"""
        lines = content[start:].split("\n")
        end_offset = 0

        for i, line in enumerate(lines[1:], 1):
            # Skip empty lines and comments
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                end_offset += len(line) + 1
                continue

            # Check indentation
            current_indent = len(line) - len(line.lstrip())
            if current_indent <= base_indent and stripped:
                break

            end_offset += len(line) + 1

        return start + end_offset


class FunctionalLanguageParser(BaseLanguageParser):
    """
    Base parser for functional languages (Haskell, Elixir, Erlang, OCaml).

    Provides patterns for:
    - Function definitions with pattern matching
    - Type signatures
    - Module definitions
    """

    # Haskell patterns
    HASKELL_FUNCTION_PATTERN = re.compile(
        r"^(\w+)\s*::\s*(.+?)$", re.MULTILINE  # Type signature
    )
    HASKELL_MODULE_PATTERN = re.compile(r"^module\s+([\w.]+)", re.MULTILINE)

    # Elixir patterns
    ELIXIR_MODULE_PATTERN = re.compile(r"defmodule\s+([\w.]+)", re.MULTILINE)
    ELIXIR_FUNCTION_PATTERN = re.compile(r"def[p]?\s+(\w+)", re.MULTILINE)


class MarkupParser(BaseLanguageParser):
    """
    Base parser for markup/data languages (XML, JSON, YAML, TOML).

    Provides patterns for:
    - Nested structure extraction
    - Key-value parsing
    - Schema detection
    """

    def _extract_keys(self, content: str) -> List[str]:
        """Extract top-level keys from structured data"""
        raise NotImplementedError

    def _build_hierarchy(self, content: str) -> Dict[str, Any]:
        """Build hierarchical representation"""
        raise NotImplementedError


# =============================================================================
# Plugin Architecture
# =============================================================================


class ParserPlugin:
    """
    Plugin interface for external parsers.

    Allows registration of custom parsers without modifying core code.
    """

    def __init__(
        self,
        name: str,
        parser_class: Type[BaseLanguageParser],
        languages: List[str],
        extensions: List[str],
        priority: int = 0,
        metadata: Optional[Dict[str, Any]] = None,
    ):
        self.name = name
        self.parser_class = parser_class
        self.languages = languages
        self.extensions = extensions
        self.priority = priority  # Higher priority = preferred
        self.metadata = metadata or {}
        self._instance: Optional[BaseLanguageParser] = None

    def get_parser(self) -> BaseLanguageParser:
        """Get or create parser instance"""
        if self._instance is None:
            self._instance = self.parser_class()
        return self._instance

    def supports_language(self, language: str) -> bool:
        """Check if plugin supports given language"""
        return language.lower() in [l.lower() for l in self.languages]

    def supports_extension(self, extension: str) -> bool:
        """Check if plugin supports given file extension"""
        ext = extension.lower()
        if not ext.startswith("."):
            ext = "." + ext
        return ext in [e.lower() for e in self.extensions]


class ParserPluginRegistry:
    """
    Registry for parser plugins.

    Manages plugin registration, discovery, and instantiation.
    Thread-safe singleton pattern.
    """

    _instance = None
    _lock = threading.Lock()

    def __new__(cls):
        if cls._instance is None:
            with cls._lock:
                if cls._instance is None:
                    cls._instance = super().__new__(cls)
                    cls._instance._plugins: Dict[str, ParserPlugin] = {}
                    cls._instance._language_index: Dict[str, List[str]] = {}
                    cls._instance._extension_index: Dict[str, List[str]] = {}
        return cls._instance

    def register(self, plugin: ParserPlugin) -> bool:
        """
        Register a parser plugin.

        Returns True if registration successful, False if plugin already exists.
        """
        if plugin.name in self._plugins:
            logger.warning(f"Plugin {plugin.name} already registered")
            return False

        self._plugins[plugin.name] = plugin

        # Index by language
        for lang in plugin.languages:
            lang_lower = lang.lower()
            if lang_lower not in self._language_index:
                self._language_index[lang_lower] = []
            self._language_index[lang_lower].append(plugin.name)
            # Sort by priority
            self._language_index[lang_lower].sort(
                key=lambda n: self._plugins[n].priority, reverse=True
            )

        # Index by extension
        for ext in plugin.extensions:
            ext_lower = ext.lower()
            if not ext_lower.startswith("."):
                ext_lower = "." + ext_lower
            if ext_lower not in self._extension_index:
                self._extension_index[ext_lower] = []
            self._extension_index[ext_lower].append(plugin.name)
            self._extension_index[ext_lower].sort(
                key=lambda n: self._plugins[n].priority, reverse=True
            )

        logger.info(f"Registered parser plugin: {plugin.name}")
        return True

    def unregister(self, name: str) -> bool:
        """Unregister a parser plugin"""
        if name not in self._plugins:
            return False

        plugin = self._plugins[name]

        # Remove from language index
        for lang in plugin.languages:
            lang_lower = lang.lower()
            if lang_lower in self._language_index:
                if name in self._language_index[lang_lower]:
                    self._language_index[lang_lower].remove(name)
                # Clean up empty entries
                if not self._language_index[lang_lower]:
                    del self._language_index[lang_lower]

        # Remove from extension index
        for ext in plugin.extensions:
            ext_lower = ext.lower() if ext.startswith(".") else "." + ext.lower()
            if ext_lower in self._extension_index:
                if name in self._extension_index[ext_lower]:
                    self._extension_index[ext_lower].remove(name)
                # Clean up empty entries
                if not self._extension_index[ext_lower]:
                    del self._extension_index[ext_lower]

        del self._plugins[name]
        logger.info(f"Unregistered parser plugin: {name}")
        return True

    def get_parser_for_language(self, language: str) -> Optional[BaseLanguageParser]:
        """Get best available parser for language"""
        lang_lower = language.lower()
        if lang_lower not in self._language_index:
            return None

        plugin_names = self._language_index[lang_lower]
        if not plugin_names:
            return None

        # Return highest priority parser
        return self._plugins[plugin_names[0]].get_parser()

    def get_parser_for_extension(self, extension: str) -> Optional[BaseLanguageParser]:
        """Get best available parser for file extension"""
        ext_lower = extension.lower()
        if not ext_lower.startswith("."):
            ext_lower = "." + ext_lower

        if ext_lower not in self._extension_index:
            return None

        plugin_names = self._extension_index[ext_lower]
        if not plugin_names:
            return None

        return self._plugins[plugin_names[0]].get_parser()

    def list_plugins(self) -> List[Dict[str, Any]]:
        """List all registered plugins"""
        return [
            {
                "name": p.name,
                "languages": p.languages,
                "extensions": p.extensions,
                "priority": p.priority,
                "metadata": p.metadata,
            }
            for p in self._plugins.values()
        ]

    def get_supported_languages(self) -> Set[str]:
        """Get all supported languages"""
        return set(self._language_index.keys())

    def get_supported_extensions(self) -> Set[str]:
        """Get all supported extensions"""
        return set(self._extension_index.keys())


def get_plugin_registry() -> ParserPluginRegistry:
    """Get the singleton plugin registry"""
    return ParserPluginRegistry()


# =============================================================================
# Configuration Validation
# =============================================================================


@dataclass
class ValidationResult:
    """Result of configuration validation"""

    valid: bool
    errors: List[str] = field(default_factory=list)
    warnings: List[str] = field(default_factory=list)


class ConfigValidator:
    """Validates chunking configuration"""

    @staticmethod
    def validate_chunk_size(
        chunk_size: int, min_chunk_size: int = 0, max_chunk_size: int = 100000
    ) -> ValidationResult:
        """Validate chunk size configuration"""
        result = ValidationResult(valid=True)

        if chunk_size < min_chunk_size:
            result.valid = False
            result.errors.append(
                f"chunk_size ({chunk_size}) must be >= min_chunk_size ({min_chunk_size})"
            )

        if chunk_size > max_chunk_size:
            result.valid = False
            result.errors.append(
                f"chunk_size ({chunk_size}) must be <= max_chunk_size ({max_chunk_size})"
            )

        if chunk_size < 100:
            result.warnings.append(
                f"chunk_size ({chunk_size}) is very small, may result in too many chunks"
            )

        if chunk_size > 10000:
            result.warnings.append(
                f"chunk_size ({chunk_size}) is large, may affect embedding quality"
            )

        return result

    @staticmethod
    def validate_overlap(chunk_overlap: int, chunk_size: int) -> ValidationResult:
        """Validate chunk overlap configuration"""
        result = ValidationResult(valid=True)

        if chunk_overlap < 0:
            result.valid = False
            result.errors.append(f"chunk_overlap ({chunk_overlap}) must be >= 0")

        if chunk_overlap >= chunk_size:
            result.valid = False
            result.errors.append(
                f"chunk_overlap ({chunk_overlap}) must be < chunk_size ({chunk_size})"
            )

        if chunk_overlap > chunk_size * 0.5:
            result.warnings.append(
                f"chunk_overlap ({chunk_overlap}) is >50% of chunk_size, "
                "may cause high redundancy"
            )

        return result

    @staticmethod
    def validate_languages(languages: List[str]) -> ValidationResult:
        """Validate language configuration"""
        result = ValidationResult(valid=True)
        registry = get_plugin_registry()
        supported = registry.get_supported_languages()

        for lang in languages:
            if lang.lower() not in supported:
                result.warnings.append(f"Language '{lang}' may not be fully supported")

        return result

    @classmethod
    def validate_config(cls, config: Any) -> ValidationResult:
        """Validate complete configuration"""
        result = ValidationResult(valid=True)

        # Validate chunk size if present
        if hasattr(config, "chunk_size"):
            min_size = getattr(config, "min_chunk_size", 0)
            max_size = getattr(config, "max_chunk_size", 100000)
            size_result = cls.validate_chunk_size(config.chunk_size, min_size, max_size)
            if not size_result.valid:
                result.valid = False
            result.errors.extend(size_result.errors)
            result.warnings.extend(size_result.warnings)

        # Validate overlap if present
        if hasattr(config, "chunk_overlap") and hasattr(config, "chunk_size"):
            overlap_result = cls.validate_overlap(
                config.chunk_overlap, config.chunk_size
            )
            if not overlap_result.valid:
                result.valid = False
            result.errors.extend(overlap_result.errors)
            result.warnings.extend(overlap_result.warnings)

        # Validate languages if present
        if hasattr(config, "languages") and config.languages:
            lang_result = cls.validate_languages(config.languages)
            result.warnings.extend(lang_result.warnings)

        return result


# =============================================================================
# Utility Functions
# =============================================================================


@contextmanager
def parser_context(language: str):
    """
    Context manager for parser operations.

    Handles parser acquisition, metrics, and cleanup.
    """
    cache = get_parser_cache()
    collector = get_metrics_collector()
    start_time = time.perf_counter()

    parser = cache.get(language)
    cache_hit = parser is not None

    try:
        yield parser
    finally:
        elapsed = (time.perf_counter() - start_time) * 1000
        logger.debug(
            f"Parser context for {language}: "
            f"cache_hit={cache_hit}, elapsed={elapsed:.2f}ms"
        )


def detect_language_from_content(content: str) -> Optional[str]:
    """
    Attempt to detect language from content patterns.

    Uses heuristics like shebangs, common patterns, etc.
    """
    lines = content.split("\n", 10)[:10]  # Check first 10 lines

    # Check shebang
    if lines and lines[0].startswith("#!"):
        shebang = lines[0].lower()
        if "python" in shebang:
            return "python"
        elif "node" in shebang or "deno" in shebang:
            return "javascript"
        elif "ruby" in shebang:
            return "ruby"
        elif "perl" in shebang:
            return "perl"
        elif "bash" in shebang or "sh" in shebang:
            return "bash"

    # Check for common patterns
    content_lower = content[:2000].lower()  # Check first 2KB

    if "package main" in content_lower and "func " in content_lower:
        return "go"
    elif (
        "fn main()" in content_lower
        or "fn " in content_lower
        and "-> " in content_lower
    ):
        return "rust"
    elif "public class " in content_lower or "public interface " in content_lower:
        return "java"
    elif "def " in content_lower and ":" in content_lower:
        return "python"
    elif "function " in content_lower or "const " in content_lower:
        return "javascript"

    return None


# =============================================================================
# Exports
# =============================================================================

__all__ = [
    # Errors
    "ParserError",
    "ParserInitializationError",
    "ParseError",
    "UnsupportedLanguageError",
    # Fallback
    "FallbackStrategy",
    "FallbackConfig",
    # Metrics
    "ParserMetrics",
    "MetricsCollector",
    "get_metrics_collector",
    # Cache
    "ParserCache",
    "get_parser_cache",
    # Decorators
    "with_metrics",
    "with_fallback",
    "cached_parser",
    # Parser base classes
    "BaseLanguageParser",
    "CFamilyParser",
    "JVMFamilyParser",
    "DynamicLanguageParser",
    "FunctionalLanguageParser",
    "MarkupParser",
    # Plugin system
    "ParserPlugin",
    "ParserPluginRegistry",
    "get_plugin_registry",
    # Validation
    "ValidationResult",
    "ConfigValidator",
    # Utilities
    "parser_context",
    "detect_language_from_content",
]
