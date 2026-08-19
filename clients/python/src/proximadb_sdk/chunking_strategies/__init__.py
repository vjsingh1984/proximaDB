"""
Pluggable chunking strategies for ProximaDB Python SDK

This package provides a clean interface for text chunking with proper separation of concerns:
- Each chunking strategy in its own module
- No embedding logic mixed with chunking
- Metadata generation is purely about chunk properties
- Extensible interface for custom strategies

For code-aware chunking, use the CodeChunkingStrategy which provides:
- AST-based symbol extraction for the languages in
  `code.SYMBOL_EXTRACTING_LANGUAGES` (16, measured and asserted in CI), delegated
  to the shared `victor-codegraph` package. Other detected languages are still
  chunked and covered, but carry no symbol metadata -- see
  `code.COVERED_WITHOUT_SYMBOLS`.
- Symbol extraction (functions, classes, methods, etc.)
- Relationship extraction (calls, imports, inheritance)
- Pluggable architecture for adding new language support
- Parser caching and performance metrics
- Robust error handling with fallback strategies
"""

from .base import (
    ChunkingConfig,
    ChunkingStrategy,
    ChunkingStrategyInterface,
    TextChunk,
    config_kwargs,
)
from .boundaries import (
    Boundary,
    BoundaryKind,
    BoundarySource,
    CompositeBoundarySource,
    HeadingBoundarySource,
    StrategyBoundarySource,
    annotate_heading_paths,
    merge_boundaries,
)

# Code-aware chunking
from .code import (
    COVERED_WITHOUT_SYMBOLS,
    EXTENSION_TO_LANGUAGE,
    SYMBOL_EXTRACTING_LANGUAGES,
    CodeChunkingConfig,
    CodeChunkingStrategy,
    CodeRelation,
    CodeRelationType,
    CodeSymbol,
    CodeSymbolType,
    ParsedCode,
    SourceLocation,
    create_code_chunker,
    get_supported_extensions,
    get_supported_languages,
)
from .contracts import (
    CompositeInputContract,
    InputRenderer,
    InputRole,
    OverflowPolicy,
    ResolvedInputContract,
    ShortChunkPolicy,
    TokenBudget,
    TokenCounter,
)
from .dedup import DedupResult, deduplicate, jaccard, shingles

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
from .parser_utils import (  # Errors; Metrics; Parser base class; Validation
    BaseLanguageParser,
    ConfigValidator,
    MetricsCollector,
    ParseError,
    ParserError,
    ParserMetrics,
    ValidationResult,
    get_metrics_collector,
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
from .semantic_embedding import SemanticEmbeddingStrategy
from .sentence import SentenceStrategy
from .sizing import (
    Absolute,
    Fraction,
    Of,
    ResolvedSizing,
    SizingPolicy,
)
from .sliding_window import SlidingWindowStrategy
from .structure import (
    Heading,
    HeadingOutline,
    find_headings,
    protected_spans,
)
from .token_budget import TokenBudgetStrategy
from .tokenizers import HuggingFaceTokenCounter

__all__ = [
    # Base classes
    "ChunkingStrategy",
    "ChunkingStrategyInterface",
    "TextChunk",
    "ChunkingConfig",
    "TokenBudget",
    "TokenCounter",
    "InputRole",
    "InputRenderer",
    "OverflowPolicy",
    "ShortChunkPolicy",
    "ResolvedInputContract",
    "CompositeInputContract",
    "TokenBudgetStrategy",
    "HuggingFaceTokenCounter",
    # Text chunking strategies
    "SlidingWindowStrategy",
    "SentenceStrategy",
    # Deduplication (TD-CHUNK-3 item 2)
    "deduplicate",
    "DedupResult",
    "shingles",
    "jaccard",
    # Boundary sources (ADR-091 D2)
    "Boundary",
    "BoundaryKind",
    "BoundarySource",
    "StrategyBoundarySource",
    "CompositeBoundarySource",
    "HeadingBoundarySource",
    "annotate_heading_paths",
    "merge_boundaries",
    # Document structure (shared detection)
    "Heading",
    "HeadingOutline",
    "find_headings",
    "protected_spans",
    # Sizing (declarative budget)
    "SizingPolicy",
    "ResolvedSizing",
    "Absolute",
    "Fraction",
    "Of",
    "ParagraphStrategy",
    "SemanticStrategy",
    "SemanticEmbeddingStrategy",
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
    # Language parsers - Primary
    # Language parsers - Additional
    # Plugin functions
    "get_supported_languages",
    "get_supported_extensions",
    "create_code_chunker",
    # Registry constants
    "EXTENSION_TO_LANGUAGE",
    "SYMBOL_EXTRACTING_LANGUAGES",
    "COVERED_WITHOUT_SYMBOLS",
    # Parser utilities - Errors
    "ParserError",
    "ParseError",
    # Parser utilities - Metrics
    "ParserMetrics",
    "MetricsCollector",
    "get_metrics_collector",
    "with_metrics",
    # Parser utilities - Base class
    "BaseLanguageParser",
    # Parser utilities - Validation
    "ValidationResult",
    "ConfigValidator",
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
