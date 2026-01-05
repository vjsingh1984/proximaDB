"""
ProximaDB Python Client SDK v1.0

A modern, async-first Python client for ProximaDB vector database.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

# Version information
__version__ = "1.0.0"
__author__ = "ProximaDB Contributors"

# Protocol adapters
from .adapters import (
    BaseProtocolAdapter,
    create_adapter,
)

# Authentication
from .auth import (
    AuthConfig,
    AuthMethod,
    AuthResult,
    Permission,
    ProximaDBAuth,
)

# Builders
from .builders import (
    CollectionBuilder,
    InsertBuilder,
    SearchBuilder,
)
from .client_v1 import ProximaDBClientV1
from .config import (
    ClientConfig,
    ConnectionConfig,
    LogLevel,
    PortMode,
    Protocol,
    TLSConfig,
    load_config,
    load_config_file,
)

# Ultra-efficient enum packing (75% storage savings)
from .enum_packing import (  # Enum classes; Packing functions; Helper functions
    ContentCategory,
    DataSource,
    ExtractionMethod,
    LanguageCode,
    ProcessingStatus,
    QualityLevel,
    create_processing_info,
    create_source_content,
    create_text_content,
    pack_language_code,
    pack_processing_enums,
    pack_source_attributes,
    storage_efficiency_analysis,
    unpack_language_code,
    unpack_processing_enums,
    unpack_source_attributes,
)

# Exceptions
from .exceptions import (
    AuthenticationError,
    AuthorizationError,
    BatchError,
    CollectionExistsError,
    CollectionNotFoundError,
    ConfigurationError,
)
from .exceptions import IndexError as ProximaIndexError
from .exceptions import (
    InvalidVectorError,
    NetworkError,
    ProximaDBError,
    QuotaExceededError,
    RateLimitError,
    ServerError,
    StreamingError,
    TimeoutError,
    TransportError,
    ValidationError,
    VectorDimensionError,
    VectorNotFoundError,
    WALError,
    map_grpc_error,
    map_http_error,
)

# Filter API
from .filters import (
    FilterBuilder,
    FilterCondition,
    FilterGroup,
    FilterOp,
    LogicalOp,
    and_filters,
    eq,
    gt,
    in_list,
    lt,
    or_filters,
)

# Models
from .models import (
    Collection,
    CollectionConfig,
    CollectionInfo,
    CollectionStats,
    CompressionType,
    DistanceMetric,
    FilterableColumn,
    FilterableDataType,
    FilterDict,
    FlushConfig,
    HealthStatus,
    IndexConfiguration,
)
from .models import (
    IndexingAlgorithm,  # Collection models; Vector models; Operation responses; Enums; Quantization; Search optimization; Additional models; Type aliases
)
from .models import IndexingAlgorithm as IndexType  # Alias for compatibility
from .models import (
    MetadataDict,
    OperationMetrics,
    QuantizationConfig,
    QuantizationHint,
    QuantizationType,
    SearchOptimization,
    SearchResult,
    ServerCapabilities,
    StorageConfig,
    StorageEngine,
    VectorArray,
    VectorOperationResponse,
    VectorRecord,
)

# v2 Models - ProximaRecord and typed schema support
from .models_v2 import (
    ColumnDataType,
    ColumnDefinition,
    FilterBuilderV2,
)
from .models_v2 import (
    FilterOperator as FilterOperatorV2,  # Core record type; Typed values; Text storage; Text column convenience functions; Schema; Typed filters; Search
)
from .models_v2 import (
    ProximaRecord,
    RecordSchema,
    SchemaEnforcement,
    SearchRequestV2,
    TextColumnConfig,
    TextField,
    TextStorageStrategy,
    TypedFilterCondition,
    TypedValue,
    create_text_column_schema,
    text_column,
)

# Centralized proto type conversion
from .proto_conversion import (
    ProtoConverter,
    distance_metric_to_int,
    distance_metric_to_str,
    index_type_to_int,
    index_type_to_str,
    storage_engine_to_int,
    storage_engine_to_str,
)

# Core client and configuration
from .unified_client import ProximaDBClient
from .unified_client_v2 import ProximaDBClient as ProximaDBClientV2
from .unified_client_v2 import connect_embedded


# Convenience factory functions
def connect(url: str = None, **kwargs) -> ProximaDBClient:
    """Create a ProximaDB client with automatic protocol detection.

    Uses unified port mode by default (single port for all protocols).
    """
    if url:
        kwargs["url"] = url
    return ProximaDBClient(**kwargs)


def connect_rest(url: str = None, **kwargs) -> ProximaDBClient:
    """Create a REST-based ProximaDB client"""
    if url:
        kwargs["url"] = url
    kwargs["protocol"] = Protocol.REST
    return ProximaDBClient(**kwargs)


def connect_grpc(url: str = None, **kwargs) -> ProximaDBClient:
    """Create a gRPC-based ProximaDB client"""
    if url:
        kwargs["url"] = url
    kwargs["protocol"] = Protocol.GRPC
    return ProximaDBClient(**kwargs)


def connect_unified(url: str = None, **kwargs) -> ProximaDBClient:
    """Create a ProximaDB client for unified port mode (recommended).

    In unified mode, a single URL is used for all protocols (REST, gRPC,
    Arrow Flight) and the server automatically detects and routes requests.

    Example:
        client = connect_unified("http://localhost:5678")
    """
    if url:
        kwargs["url"] = url
    kwargs["port_mode"] = PortMode.UNIFIED
    return ProximaDBClient(**kwargs)


def connect_legacy(url: str = None, **kwargs) -> ProximaDBClient:
    """Create a ProximaDB client for legacy multi-port mode.

    Use this for older deployments with separate ports for REST (5678)
    and gRPC (5679).
    """
    if url:
        kwargs["url"] = url
    kwargs["port_mode"] = PortMode.MULTI
    return ProximaDBClient(**kwargs)


def connect_arrow_flight(url: str = None, **kwargs) -> ProximaDBClient:
    """Create a ProximaDB client using Arrow Flight for bulk data transfer.

    Arrow Flight is optimized for high-throughput operations:
    - Large batch vector inserts (millions of vectors)
    - Bulk data export/import
    - Streaming large result sets

    In unified mode (default), uses same port as REST/gRPC.
    In multi-port mode, uses port 5680.

    Requires: pip install pyarrow
    """
    if url:
        kwargs["url"] = url
    kwargs["protocol"] = Protocol.ARROW_FLIGHT
    return ProximaDBClient(**kwargs)


# Text chunking utilities (if available)
try:
    from .chunking import (
        ChunkingConfig,
        ChunkingStrategy,
        TextChunk,
        TextChunker,
        chunk_by_paragraphs,
        chunk_by_sentences,
        chunk_sliding_window,
        create_chunker,
        prepare_vector_records,
    )

    _chunking_available = True
except ImportError:
    _chunking_available = False

# Code-aware chunking and knowledge builder (if available)
try:
    from .chunking_strategies import (
        CodeChunkingConfig,
        CodeChunkingStrategy,
        CodeRelation,
        CodeRelationType,
        CodeSymbol,
        CodeSymbolType,
        create_code_chunker,
        get_supported_extensions,
        get_supported_languages,
        register_language_parser,
    )

    _code_chunking_available = True
except ImportError:
    _code_chunking_available = False

# Code knowledge builder (if available)
try:
    from .code_knowledge import (
        CodeIndexConfig,
        CodeKnowledgeBuilder,
        CodeSearchResult,
        IndexingResult,
        create_code_knowledge_store,
    )

    _code_knowledge_available = True
except ImportError:
    _code_knowledge_available = False

# Additional config classes
try:
    from .config import CompressionConfig, RetryConfig
    from .resilience import (
        AdvancedRetryPolicy,
        CircuitBreakerPolicy,
        NetworkRetryPolicy,
        ResilienceConfig,
        RetryStrategy,
    )

    _advanced_config_available = True
except (ImportError, AttributeError):
    _advanced_config_available = False

__all__ = [
    # Core
    "ProximaDBClient",
    "ProximaDBClientV1",
    "ClientConfig",
    "LogLevel",
    "ConnectionConfig",
    "TLSConfig",
    "load_config",
    "load_config_file",
    "Protocol",
    "PortMode",
    # Models
    "Collection",
    "CollectionConfig",
    "CollectionStats",
    "CollectionInfo",
    "VectorRecord",
    "SearchResult",
    "OperationMetrics",
    "VectorOperationResponse",
    "HealthStatus",
    "DistanceMetric",
    "StorageEngine",
    "IndexingAlgorithm",
    "IndexType",
    "QuantizationConfig",
    "QuantizationType",
    "QuantizationHint",
    "SearchOptimization",
    "IndexConfiguration",
    "FlushConfig",
    "StorageConfig",
    "CompressionType",
    "FilterableColumn",
    "FilterableDataType",
    "ServerCapabilities",
    "VectorArray",
    "MetadataDict",
    "FilterDict",
    # v2 Models - ProximaRecord and typed schema support
    "ProximaRecord",
    "TypedValue",
    "ColumnDataType",
    "TextField",
    "TextStorageStrategy",
    "TextColumnConfig",
    "create_text_column_schema",
    "text_column",
    "RecordSchema",
    "ColumnDefinition",
    "SchemaEnforcement",
    "FilterBuilderV2",
    "TypedFilterCondition",
    "FilterOperatorV2",
    "SearchRequestV2",
    # Exceptions
    "ProximaDBError",
    "CollectionNotFoundError",
    "CollectionExistsError",
    "VectorNotFoundError",
    "VectorDimensionError",
    "InvalidVectorError",
    "AuthenticationError",
    "AuthorizationError",
    "RateLimitError",
    "QuotaExceededError",
    "ValidationError",
    "ServerError",
    "NetworkError",
    "TransportError",
    "TimeoutError",
    "ConfigurationError",
    "ProximaIndexError",
    "BatchError",
    "WALError",
    "StreamingError",
    "map_http_error",
    "map_grpc_error",
    # Filter API
    "FilterBuilder",
    "FilterOp",
    "LogicalOp",
    "FilterCondition",
    "FilterGroup",
    "eq",
    "gt",
    "lt",
    "in_list",
    "and_filters",
    "or_filters",
    # Builders
    "SearchBuilder",
    "CollectionBuilder",
    "InsertBuilder",
    # Authentication
    "ProximaDBAuth",
    "AuthConfig",
    "AuthMethod",
    "AuthResult",
    "Permission",
    # Factory functions
    "connect",
    "connect_rest",
    "connect_grpc",
    "connect_unified",
    "connect_legacy",
    "connect_arrow_flight",
    # Proto type conversion
    "ProtoConverter",
    "distance_metric_to_int",
    "distance_metric_to_str",
    "storage_engine_to_int",
    "storage_engine_to_str",
    "index_type_to_int",
    "index_type_to_str",
]

# Add chunking exports if available
if _chunking_available:
    __all__.extend(
        [
            "TextChunker",
            "ChunkingStrategy",
            "ChunkingConfig",
            "TextChunk",
            "create_chunker",
            "chunk_by_sentences",
            "chunk_by_paragraphs",
            "chunk_sliding_window",
            "prepare_vector_records",
        ]
    )

# Add code chunking exports if available
if _code_chunking_available:
    __all__.extend(
        [
            "CodeChunkingStrategy",
            "CodeChunkingConfig",
            "CodeSymbol",
            "CodeSymbolType",
            "CodeRelation",
            "CodeRelationType",
            "create_code_chunker",
            "get_supported_languages",
            "get_supported_extensions",
            "register_language_parser",
        ]
    )

# Add code knowledge builder exports if available
if _code_knowledge_available:
    __all__.extend(
        [
            "CodeKnowledgeBuilder",
            "CodeIndexConfig",
            "CodeSearchResult",
            "IndexingResult",
            "create_code_knowledge_store",
        ]
    )

# Advanced features (batching, streaming, cache, circuit breaker)
try:
    from .batching import (
        BatchConfig,
        BatchMetrics,
        BatchRequest,
        BatchStrategy,
        Pipeline,
        RequestBatcher,
        batch_insert_vectors,
        create_vector_batcher,
    )

    _batching_available = True
except ImportError:
    _batching_available = False

# Streaming functionality removed - use core SDK methods instead
_streaming_available = False

try:
    from .cache import (
        CacheMetrics,
        CacheStrategy,
        SmartCache,
    )

    _cache_available = True
except ImportError:
    _cache_available = False

try:
    from .circuit_breaker import (
        CircuitBreaker,
        ResilientClient,
        RetryMechanism,
        circuit_breaker,
        create_resilient_client,
        resilient,
        retry,
    )

    _circuit_breaker_available = True
except ImportError:
    _circuit_breaker_available = False

# Add advanced config if available
if _advanced_config_available:
    __all__.extend(
        [
            "RetryConfig",
            "CompressionConfig",
            # Resilience patterns
            "NetworkRetryPolicy",
            "AdvancedRetryPolicy",
            "CircuitBreakerPolicy",
            "ResilienceConfig",
            "RetryStrategy",
        ]
    )

# Add advanced feature exports
if _batching_available:
    __all__.extend(
        [
            "RequestBatcher",
            "BatchStrategy",
            "BatchConfig",
            "BatchRequest",
            "BatchMetrics",
            "Pipeline",
            "create_vector_batcher",
            "batch_insert_vectors",
        ]
    )

# Streaming functionality removed - use core SDK methods instead

if _cache_available:
    __all__.extend(
        [
            "SmartCache",
            "CacheStrategy",
            "CacheMetrics",
        ]
    )

if _circuit_breaker_available:
    __all__.extend(
        [
            "CircuitBreaker",
            "RetryMechanism",
            "ResilientClient",
            "circuit_breaker",
            "retry",
            "resilient",
            "create_resilient_client",
        ]
    )

# Backwards compatibility aliases
IndexConfig = IndexConfiguration  # Alias for backwards compatibility
Vector = VectorRecord  # Alias for backwards compatibility

__all__.extend(
    [
        "IndexConfig",
        "Vector",
    ]
)

# Graph Analytics
try:
    from .graph_analytics import (
        AlgorithmConfig,
        AlgorithmResult,
        GraphAlgorithm,
        GraphAnalytics,
        GraphPattern,
        PatternElement,
        PatternMatchMode,
        PatternMatchResult,
        RelationshipPattern,
        SemanticTraversalConfig,
        SemanticTraversalResult,
        TraversalDirection,
        node,
        relationship,
    )

    _graph_analytics_available = True
except ImportError:
    _graph_analytics_available = False

if _graph_analytics_available:
    __all__.extend(
        [
            "GraphAnalytics",
            "AlgorithmConfig",
            "SemanticTraversalConfig",
            "GraphPattern",
            "PatternElement",
            "RelationshipPattern",
            "AlgorithmResult",
            "SemanticTraversalResult",
            "PatternMatchResult",
            "GraphAlgorithm",
            "TraversalDirection",
            "PatternMatchMode",
            "node",
            "relationship",
        ]
    )

# Observability (OpenTelemetry, Prometheus, Tracing)
try:
    from .observability import LogLevel as ObsLogLevel
    from .observability import (
        MetricDefinition,
        MetricsCollector,
        MetricType,
        Observability,
        Span,
        SpanContext,
        StructuredLogger,
        Tracer,
        metered,
        traced,
    )

    _observability_available = True
except ImportError:
    _observability_available = False

if _observability_available:
    __all__.extend(
        [
            "Observability",
            "MetricsCollector",
            "Tracer",
            "StructuredLogger",
            "MetricDefinition",
            "SpanContext",
            "Span",
            "MetricType",
            "ObsLogLevel",
            "traced",
            "metered",
        ]
    )

# AutoML (Engine Selection, Workload Prediction, Optimization)
try:
    from .automl import (
        AutoML,
        EngineRecommendation,
        EngineSelector,
        HyperparameterConfig,
        HyperparameterOptimizer,
        OptimizationGoal,
        OptimizationResult,
        WorkloadCharacteristics,
        WorkloadPredictor,
        WorkloadType,
    )

    _automl_available = True
except ImportError:
    _automl_available = False

if _automl_available:
    __all__.extend(
        [
            "AutoML",
            "WorkloadPredictor",
            "EngineSelector",
            "HyperparameterOptimizer",
            "WorkloadCharacteristics",
            "EngineRecommendation",
            "HyperparameterConfig",
            "OptimizationResult",
            "WorkloadType",
            "OptimizationGoal",
        ]
    )

# Embedded mode and embedding models
try:
    from .embedded import (  # Core embedded classes; Embedding model classes; Protocols
        AsyncEmbeddingFunction,
        BaseEmbeddingModel,
        BatchEmbeddingFunction,
        EmbeddedCollection,
        EmbeddedConfig,
        EmbeddedMultiModalQueryExecutor,
        EmbeddedProximaDB,
        EmbeddingFunction,
        FunctionEmbeddingModel,
        OllamaEmbeddingModel,
        OpenAIEmbeddingModel,
        SentenceTransformerModel,
        connect_embedded,
        create_embedding_model,
    )

    _embedded_available = True
except ImportError:
    _embedded_available = False

if _embedded_available:
    __all__.extend(
        [
            # Embedded mode
            "EmbeddedProximaDB",
            "EmbeddedCollection",
            "EmbeddedConfig",
            "EmbeddedMultiModalQueryExecutor",
            "connect_embedded",
            # Embedding models
            "BaseEmbeddingModel",
            "SentenceTransformerModel",
            "OllamaEmbeddingModel",
            "OpenAIEmbeddingModel",
            "FunctionEmbeddingModel",
            "create_embedding_model",
            # Protocols
            "EmbeddingFunction",
            "AsyncEmbeddingFunction",
            "BatchEmbeddingFunction",
        ]
    )

# Multi-Modal Query API (Phase 13.4)
try:
    from .multimodal_query import (  # Core classes; Query components; Enums; Cross-Modal Reranking; Learned Fusion (ML-based); Convenience functions
        CrossModalReranker,
        DocumentQueryComponent,
        FeatureExtractor,
        FeedbackSignal,
        FeedbackType,
        FusionFeatures,
        FusionModelType,
        FusionStrategy,
        GraphQueryComponent,
        JoinType,
        LearnedFusion,
        LearnedFusionConfig,
        LogQueryComponent,
        MetricQueryComponent,
        MultiModalQuery,
        MultiModalQueryBuilder,
        MultiModalQueryExecutor,
        MultiModalQueryResult,
        QueryContext,
        QueryIntent,
        QueryType,
        RerankConfig,
        RerankedResult,
        RerankExplanation,
        ScoreComponent,
        SemanticJoin,
        TemporalPreference,
        TimeDecayFunction,
        TrainingMetrics,
        TrainingSample,
        VectorQueryComponent,
        knowledge_graph_search,
        logs_with_context,
        semantic_search_with_graph,
    )

    _multimodal_query_available = True
except ImportError:
    _multimodal_query_available = False

if _multimodal_query_available:
    __all__.extend(
        [
            # Core classes
            "MultiModalQueryBuilder",
            "MultiModalQuery",
            "MultiModalQueryResult",
            "MultiModalQueryExecutor",
            # Query components
            "VectorQueryComponent",
            "GraphQueryComponent",
            "DocumentQueryComponent",
            "LogQueryComponent",
            "MetricQueryComponent",
            "SemanticJoin",
            # Enums
            "QueryType",
            "FusionStrategy",
            "JoinType",
            "TimeDecayFunction",
            # Cross-Modal Reranking
            "CrossModalReranker",
            "RerankConfig",
            "QueryContext",
            "QueryIntent",
            "TemporalPreference",
            "ScoreComponent",
            "RerankExplanation",
            "RerankedResult",
            # Learned Fusion (ML-based)
            "LearnedFusion",
            "LearnedFusionConfig",
            "FusionModelType",
            "FusionFeatures",
            "FeedbackType",
            "FeedbackSignal",
            "TrainingSample",
            "TrainingMetrics",
            "FeatureExtractor",
            # Convenience functions
            "semantic_search_with_graph",
            "knowledge_graph_search",
            "logs_with_context",
        ]
    )

# Security Module (Phase 13.5)
try:
    from .security import (  # OAuth2; RBAC; Security Context; Audit; mTLS
        AuditEvent,
        AuditEventType,
        AuditLogger,
        MTLSConfig,
        OAuth2Config,
        OAuth2Error,
        OAuth2GrantType,
        OAuth2Provider,
        OAuth2TokenManager,
        OAuth2TokenResponse,
        RBACManager,
        Role,
        RoleDefinition,
        SecurityContext,
        SecurityManager,
        clear_security_context,
        get_current_security_context,
        security_context,
        set_security_context,
    )

    _security_available = True
except ImportError:
    _security_available = False

if _security_available:
    __all__.extend(
        [
            # OAuth2
            "OAuth2TokenManager",
            "OAuth2Config",
            "OAuth2TokenResponse",
            "OAuth2GrantType",
            "OAuth2Provider",
            "OAuth2Error",
            # RBAC
            "RBACManager",
            "RoleDefinition",
            "Role",
            # Security Context
            "SecurityContext",
            "SecurityManager",
            "security_context",
            "get_current_security_context",
            "set_security_context",
            "clear_security_context",
            # Audit
            "AuditLogger",
            "AuditEvent",
            "AuditEventType",
            # mTLS
            "MTLSConfig",
        ]
    )

# Arrow Export (PyArrow, Polars, DuckDB interop)
try:
    from .arrow_export import (
        ArrowExportClient,
        FileFormat,
        FileInfo,
        connect_arrow,
        read_proximadb_collection,
        read_proximadb_file,
    )

    _arrow_export_available = True
except ImportError:
    _arrow_export_available = False

if _arrow_export_available:
    __all__.extend(
        [
            "ArrowExportClient",
            "FileFormat",
            "FileInfo",
            "connect_arrow",
            "read_proximadb_file",
            "read_proximadb_collection",
        ]
    )
