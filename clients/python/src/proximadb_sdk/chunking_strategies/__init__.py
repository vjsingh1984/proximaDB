"""
Pluggable chunking strategies for ProximaDB Python SDK

This package provides a clean interface for text chunking with proper separation of concerns:
- Each chunking strategy in its own module
- No embedding logic mixed with chunking
- Metadata generation is purely about chunk properties
- Extensible interface for custom strategies

For code-aware chunking, use the CodeChunkingStrategy which provides:
- AST-based parsing using tree-sitter for 20+ languages
- Symbol extraction (functions, classes, methods, etc.)
- Relationship extraction (calls, imports, inheritance)
- Pluggable architecture for adding new language support
- Parser caching and performance metrics
- Robust error handling with fallback strategies
"""

from .base import ChunkingStrategy, ChunkingStrategyInterface, TextChunk, ChunkingConfig
from .sliding_window import SlidingWindowStrategy
from .sentence import SentenceStrategy
from .paragraph import ParagraphStrategy
from .semantic import SemanticStrategy
from .recursive import RecursiveStrategy
from .factory import ChunkingStrategyFactory, get_chunking_strategy

# Parser utilities (enhanced design patterns)
from .parser_utils import (
    # Errors
    ParserError,
    ParserInitializationError,
    ParseError,
    UnsupportedLanguageError,
    # Fallback strategies
    FallbackStrategy,
    FallbackConfig,
    # Metrics
    ParserMetrics,
    MetricsCollector,
    get_metrics_collector,
    # Cache
    ParserCache,
    get_parser_cache,
    # Decorators
    with_metrics,
    with_fallback,
    cached_parser,
    # Parser base classes
    BaseLanguageParser,
    CFamilyParser,
    JVMFamilyParser,
    DynamicLanguageParser,
    FunctionalLanguageParser,
    MarkupParser,
    # Plugin system
    ParserPlugin,
    ParserPluginRegistry,
    get_plugin_registry,
    # Validation
    ValidationResult,
    ConfigValidator,
    # Utilities
    parser_context,
    detect_language_from_content,
)

# Code-aware chunking
from .code import (
    CodeChunkingStrategy,
    CodeChunkingConfig,
    CodeSymbol,
    CodeSymbolType,
    CodeRelation,
    CodeRelationType,
    ParsedCode,
    SourceLocation,
    LanguageParser,
    # Parser classes - Primary languages
    PythonParser,
    RustParser,
    GoParser,
    JavaParser,
    JavaScriptParser,
    CppParser,
    RubyParser,
    # Parser classes - Additional languages
    CSharpParser,
    PhpParser,
    SwiftParser,
    KotlinParser,
    ScalaParser,
    BashParser,
    SqlParser,
    YamlParser,
    JsonParser,
    XmlParser,
    PerlParser,
    LuaParser,
    HaskellParser,
    ElixirParser,
    # Registry functions
    register_language_parser,
    register_file_extension,
    get_supported_languages,
    get_supported_extensions,
    create_code_chunker,
    # Constants
    LANGUAGE_PARSERS,
    EXTENSION_TO_LANGUAGE,
)

# Document and binary parsers (OCR, reverse engineering)
from .document_parsers import (
    # Enums
    BinaryType,
    DocumentType,
    # Data structures
    BinarySymbol,
    BinaryAnalysis,
    OCRResult,
    BinaryParserConfig,
    OCRConfig,
    # Tool detection
    ToolDetector,
    # Parsers
    BinaryParser,
    DocumentParser,
    # Factory functions
    create_binary_parser,
    create_document_parser,
    get_available_tools,
)

# Unified pipeline (orchestration, batch processing, streaming)
from .pipeline import (
    # Configuration
    ProcessingMode,
    ErrorHandling,
    PipelineConfig,
    PipelineResult,
    BatchResult,
    # Pipeline stages
    PipelineStage,
    ValidationStage,
    EnrichmentStage,
    FilterStage,
    # Core components
    BatchEmbedder,
    ProgressTracker,
    ChunkingPipeline,
    # Factory functions
    create_pipeline,
    create_code_pipeline,
    create_document_pipeline,
    # Context managers
    pipeline_context,
    async_pipeline_context,
)

__all__ = [
    # Base classes
    "ChunkingStrategy",
    "ChunkingStrategyInterface",
    "TextChunk",
    "ChunkingConfig",
    # Text chunking strategies
    "SlidingWindowStrategy",
    "SentenceStrategy",
    "ParagraphStrategy",
    "SemanticStrategy",
    "RecursiveStrategy",
    # Factory
    "ChunkingStrategyFactory",
    "get_chunking_strategy",
    # Code-aware chunking
    "CodeChunkingStrategy",
    "CodeChunkingConfig",
    "CodeSymbol",
    "CodeSymbolType",
    "CodeRelation",
    "CodeRelationType",
    "ParsedCode",
    "SourceLocation",
    "LanguageParser",
    # Language parsers - Primary
    "PythonParser",
    "RustParser",
    "GoParser",
    "JavaParser",
    "JavaScriptParser",
    "CppParser",
    "RubyParser",
    # Language parsers - Additional
    "CSharpParser",
    "PhpParser",
    "SwiftParser",
    "KotlinParser",
    "ScalaParser",
    "BashParser",
    "SqlParser",
    "YamlParser",
    "JsonParser",
    "XmlParser",
    "PerlParser",
    "LuaParser",
    "HaskellParser",
    "ElixirParser",
    # Plugin functions
    "register_language_parser",
    "register_file_extension",
    "get_supported_languages",
    "get_supported_extensions",
    "create_code_chunker",
    # Registry constants
    "LANGUAGE_PARSERS",
    "EXTENSION_TO_LANGUAGE",
    # Parser utilities - Errors
    "ParserError",
    "ParserInitializationError",
    "ParseError",
    "UnsupportedLanguageError",
    # Parser utilities - Fallback
    "FallbackStrategy",
    "FallbackConfig",
    # Parser utilities - Metrics
    "ParserMetrics",
    "MetricsCollector",
    "get_metrics_collector",
    # Parser utilities - Cache
    "ParserCache",
    "get_parser_cache",
    # Parser utilities - Decorators
    "with_metrics",
    "with_fallback",
    "cached_parser",
    # Parser utilities - Base classes
    "BaseLanguageParser",
    "CFamilyParser",
    "JVMFamilyParser",
    "DynamicLanguageParser",
    "FunctionalLanguageParser",
    "MarkupParser",
    # Parser utilities - Plugin system
    "ParserPlugin",
    "ParserPluginRegistry",
    "get_plugin_registry",
    # Parser utilities - Validation
    "ValidationResult",
    "ConfigValidator",
    # Parser utilities - Helpers
    "parser_context",
    "detect_language_from_content",
    # Document/Binary parsers
    "BinaryType",
    "DocumentType",
    "BinarySymbol",
    "BinaryAnalysis",
    "OCRResult",
    "BinaryParserConfig",
    "OCRConfig",
    "ToolDetector",
    "BinaryParser",
    "DocumentParser",
    "create_binary_parser",
    "create_document_parser",
    "get_available_tools",
    # Pipeline - Configuration
    "ProcessingMode",
    "ErrorHandling",
    "PipelineConfig",
    "PipelineResult",
    "BatchResult",
    # Pipeline - Stages
    "PipelineStage",
    "ValidationStage",
    "EnrichmentStage",
    "FilterStage",
    # Pipeline - Core
    "BatchEmbedder",
    "ProgressTracker",
    "ChunkingPipeline",
    # Pipeline - Factory functions
    "create_pipeline",
    "create_code_pipeline",
    "create_document_pipeline",
    # Pipeline - Context managers
    "pipeline_context",
    "async_pipeline_context",
]
