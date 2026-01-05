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

from .base import ChunkingConfig, ChunkingStrategy, ChunkingStrategyInterface, TextChunk

# Code-aware chunking
from .code import (  # Parser classes - Primary languages; Parser classes - Additional languages; Registry functions; Constants
    EXTENSION_TO_LANGUAGE,
    LANGUAGE_PARSERS,
    BashParser,
    CodeChunkingConfig,
    CodeChunkingStrategy,
    CodeRelation,
    CodeRelationType,
    CodeSymbol,
    CodeSymbolType,
    CppParser,
    CSharpParser,
    ElixirParser,
    GoParser,
    HaskellParser,
    JavaParser,
    JavaScriptParser,
    JsonParser,
    KotlinParser,
    LanguageParser,
    LuaParser,
    ParsedCode,
    PerlParser,
    PhpParser,
    PythonParser,
    RubyParser,
    RustParser,
    ScalaParser,
    SourceLocation,
    SqlParser,
    SwiftParser,
    XmlParser,
    YamlParser,
    create_code_chunker,
    get_supported_extensions,
    get_supported_languages,
    register_file_extension,
    register_language_parser,
)

# Document and binary parsers (OCR, reverse engineering)
from .document_parsers import (  # Enums; Data structures; Tool detection; Parsers; Factory functions
    BinaryAnalysis,
    BinaryParser,
    BinaryParserConfig,
    BinarySymbol,
    BinaryType,
    DocumentParser,
    DocumentType,
    OCRConfig,
    OCRResult,
    ToolDetector,
    create_binary_parser,
    create_document_parser,
    get_available_tools,
)
from .factory import ChunkingStrategyFactory, get_chunking_strategy
from .paragraph import ParagraphStrategy

# Parser utilities (enhanced design patterns)
from .parser_utils import (  # Errors; Fallback strategies; Metrics; Cache; Decorators; Parser base classes; Plugin system; Validation; Utilities
    BaseLanguageParser,
    CFamilyParser,
    ConfigValidator,
    DynamicLanguageParser,
    FallbackConfig,
    FallbackStrategy,
    FunctionalLanguageParser,
    JVMFamilyParser,
    MarkupParser,
    MetricsCollector,
    ParseError,
    ParserCache,
    ParserError,
    ParserInitializationError,
    ParserMetrics,
    ParserPlugin,
    ParserPluginRegistry,
    UnsupportedLanguageError,
    ValidationResult,
    cached_parser,
    detect_language_from_content,
    get_metrics_collector,
    get_parser_cache,
    get_plugin_registry,
    parser_context,
    with_fallback,
    with_metrics,
)

# Unified pipeline (orchestration, batch processing, streaming)
from .pipeline import (  # Configuration; Pipeline stages; Core components; Factory functions; Context managers
    BatchEmbedder,
    BatchResult,
    ChunkingPipeline,
    EnrichmentStage,
    ErrorHandling,
    FilterStage,
    PipelineConfig,
    PipelineResult,
    PipelineStage,
    ProcessingMode,
    ProgressTracker,
    ValidationStage,
    async_pipeline_context,
    create_code_pipeline,
    create_document_pipeline,
    create_pipeline,
    pipeline_context,
)
from .recursive import RecursiveStrategy
from .semantic import SemanticStrategy
from .sentence import SentenceStrategy
from .sliding_window import SlidingWindowStrategy

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
