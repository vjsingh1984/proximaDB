from google.protobuf import struct_pb2 as _struct_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class DistanceMetric(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    DISTANCE_METRIC_UNSPECIFIED: _ClassVar[DistanceMetric]
    COSINE: _ClassVar[DistanceMetric]
    EUCLIDEAN: _ClassVar[DistanceMetric]
    DOT_PRODUCT: _ClassVar[DistanceMetric]
    HAMMING: _ClassVar[DistanceMetric]
    MANHATTAN: _ClassVar[DistanceMetric]
    JACCARD: _ClassVar[DistanceMetric]
    CUSTOM: _ClassVar[DistanceMetric]
    CHEBYSHEV: _ClassVar[DistanceMetric]
    CANBERRA: _ClassVar[DistanceMetric]
    MINKOWSKI: _ClassVar[DistanceMetric]
    ANGULAR: _ClassVar[DistanceMetric]
    BRAY_CURTIS: _ClassVar[DistanceMetric]
    HELLINGER: _ClassVar[DistanceMetric]

class StorageEngine(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    STORAGE_ENGINE_UNSPECIFIED: _ClassVar[StorageEngine]
    VIPER: _ClassVar[StorageEngine]
    SST: _ClassVar[StorageEngine]
    MMAP: _ClassVar[StorageEngine]
    HYBRID: _ClassVar[StorageEngine]
    SWIFT: _ClassVar[StorageEngine]
    NOVA: _ClassVar[StorageEngine]

class IndexingAlgorithm(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    INDEXING_ALGORITHM_UNSPECIFIED: _ClassVar[IndexingAlgorithm]
    HNSW: _ClassVar[IndexingAlgorithm]
    IVF: _ClassVar[IndexingAlgorithm]
    PQ: _ClassVar[IndexingAlgorithm]
    FLAT: _ClassVar[IndexingAlgorithm]
    ANNOY: _ClassVar[IndexingAlgorithm]
    LSH: _ClassVar[IndexingAlgorithm]

class CollectionOperation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    COLLECTION_OPERATION_UNSPECIFIED: _ClassVar[CollectionOperation]
    COLLECTION_CREATE: _ClassVar[CollectionOperation]
    COLLECTION_UPDATE: _ClassVar[CollectionOperation]
    COLLECTION_GET: _ClassVar[CollectionOperation]
    COLLECTION_LIST: _ClassVar[CollectionOperation]
    COLLECTION_DELETE: _ClassVar[CollectionOperation]
    COLLECTION_MIGRATE: _ClassVar[CollectionOperation]
    COLLECTION_GET_ID_BY_NAME: _ClassVar[CollectionOperation]

class VectorOperation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    VECTOR_OPERATION_UNSPECIFIED: _ClassVar[VectorOperation]
    VECTOR_BATCH: _ClassVar[VectorOperation]
    VECTOR_SEARCH: _ClassVar[VectorOperation]
    VECTOR_GET: _ClassVar[VectorOperation]

class EmbeddingModelType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    EMBEDDING_MODEL_UNSPECIFIED: _ClassVar[EmbeddingModelType]
    OPENAI_TEXT_EMBEDDING_ADA_002: _ClassVar[EmbeddingModelType]
    OPENAI_TEXT_EMBEDDING_3_SMALL: _ClassVar[EmbeddingModelType]
    OPENAI_TEXT_EMBEDDING_3_LARGE: _ClassVar[EmbeddingModelType]
    SENTENCE_TRANSFORMERS_ALL_MINILM_L6_V2: _ClassVar[EmbeddingModelType]
    SENTENCE_TRANSFORMERS_ALL_MPNET_BASE_V2: _ClassVar[EmbeddingModelType]
    SENTENCE_TRANSFORMERS_MULTI_QA_MPNET_BASE_DOT_V1: _ClassVar[EmbeddingModelType]
    SENTENCE_TRANSFORMERS_ALL_DISTILROBERTA_V1: _ClassVar[EmbeddingModelType]
    SENTENCE_TRANSFORMERS_PARAPHRASE_MULTILINGUAL_MPNET_BASE_V2: _ClassVar[EmbeddingModelType]
    GOOGLE_USE_V4: _ClassVar[EmbeddingModelType]
    GOOGLE_USE_MULTILINGUAL_V3: _ClassVar[EmbeddingModelType]
    GOOGLE_USE_LITE: _ClassVar[EmbeddingModelType]
    COHERE_EMBED_ENGLISH_V3: _ClassVar[EmbeddingModelType]
    COHERE_EMBED_MULTILINGUAL_V3: _ClassVar[EmbeddingModelType]
    COHERE_EMBED_ENGLISH_LIGHT_V3: _ClassVar[EmbeddingModelType]
    ANTHROPIC_VOYAGE_2: _ClassVar[EmbeddingModelType]
    ANTHROPIC_VOYAGE_CODE_2: _ClassVar[EmbeddingModelType]
    MISTRAL_EMBED: _ClassVar[EmbeddingModelType]
    BGE_LARGE_EN_V1_5: _ClassVar[EmbeddingModelType]
    BGE_BASE_EN_V1_5: _ClassVar[EmbeddingModelType]
    BGE_SMALL_EN_V1_5: _ClassVar[EmbeddingModelType]
    BGE_M3: _ClassVar[EmbeddingModelType]
    E5_LARGE_V2: _ClassVar[EmbeddingModelType]
    E5_BASE_V2: _ClassVar[EmbeddingModelType]
    E5_SMALL_V2: _ClassVar[EmbeddingModelType]
    INSTRUCTOR_XL: _ClassVar[EmbeddingModelType]
    INSTRUCTOR_LARGE: _ClassVar[EmbeddingModelType]
    INSTRUCTOR_BASE: _ClassVar[EmbeddingModelType]
    CUSTOM_EMBEDDING_MODEL: _ClassVar[EmbeddingModelType]

class ContentCategory(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    CONTENT_CATEGORY_UNSPECIFIED: _ClassVar[ContentCategory]
    DOCUMENT: _ClassVar[ContentCategory]
    IMAGE: _ClassVar[ContentCategory]
    AUDIO: _ClassVar[ContentCategory]
    VIDEO: _ClassVar[ContentCategory]
    CODE: _ClassVar[ContentCategory]
    TABLE: _ClassVar[ContentCategory]
    CHART: _ClassVar[ContentCategory]
    EMAIL: _ClassVar[ContentCategory]
    WEBPAGE: _ClassVar[ContentCategory]
    SOCIAL_MEDIA: _ClassVar[ContentCategory]
    KNOWLEDGE_BASE: _ClassVar[ContentCategory]
    SCIENTIFIC: _ClassVar[ContentCategory]
    LEGAL: _ClassVar[ContentCategory]
    FINANCIAL: _ClassVar[ContentCategory]
    MEDICAL: _ClassVar[ContentCategory]

class QualityLevel(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    QUALITY_UNSPECIFIED: _ClassVar[QualityLevel]
    HIGH: _ClassVar[QualityLevel]
    MEDIUM: _ClassVar[QualityLevel]
    LOW: _ClassVar[QualityLevel]
    UNKNOWN: _ClassVar[QualityLevel]

class ProcessingStatus(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    PROCESSING_STATUS_UNSPECIFIED: _ClassVar[ProcessingStatus]
    RAW: _ClassVar[ProcessingStatus]
    PROCESSING: _ClassVar[ProcessingStatus]
    PROCESSED: _ClassVar[ProcessingStatus]
    FAILED: _ClassVar[ProcessingStatus]
    REQUIRES_REVIEW: _ClassVar[ProcessingStatus]
    APPROVED: _ClassVar[ProcessingStatus]
    DEPRECATED: _ClassVar[ProcessingStatus]

class LanguageCode(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    LANGUAGE_UNSPECIFIED: _ClassVar[LanguageCode]
    ENGLISH: _ClassVar[LanguageCode]
    SPANISH: _ClassVar[LanguageCode]
    FRENCH: _ClassVar[LanguageCode]
    GERMAN: _ClassVar[LanguageCode]
    ITALIAN: _ClassVar[LanguageCode]
    PORTUGUESE: _ClassVar[LanguageCode]
    RUSSIAN: _ClassVar[LanguageCode]
    CHINESE: _ClassVar[LanguageCode]
    JAPANESE: _ClassVar[LanguageCode]
    KOREAN: _ClassVar[LanguageCode]
    ARABIC: _ClassVar[LanguageCode]
    HINDI: _ClassVar[LanguageCode]
    DUTCH: _ClassVar[LanguageCode]
    SWEDISH: _ClassVar[LanguageCode]
    NORWEGIAN: _ClassVar[LanguageCode]
    DANISH: _ClassVar[LanguageCode]
    FINNISH: _ClassVar[LanguageCode]
    POLISH: _ClassVar[LanguageCode]
    CZECH: _ClassVar[LanguageCode]
    HUNGARIAN: _ClassVar[LanguageCode]
    TURKISH: _ClassVar[LanguageCode]
    GREEK: _ClassVar[LanguageCode]
    HEBREW: _ClassVar[LanguageCode]
    THAI: _ClassVar[LanguageCode]
    VIETNAMESE: _ClassVar[LanguageCode]
    INDONESIAN: _ClassVar[LanguageCode]
    MALAY: _ClassVar[LanguageCode]
    FILIPINO: _ClassVar[LanguageCode]
    CUSTOM_LANGUAGE: _ClassVar[LanguageCode]

class DataSource(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    DATA_SOURCE_UNSPECIFIED: _ClassVar[DataSource]
    USER_UPLOAD: _ClassVar[DataSource]
    API_INGESTION: _ClassVar[DataSource]
    WEB_SCRAPING: _ClassVar[DataSource]
    FILE_IMPORT: _ClassVar[DataSource]
    DATABASE_SYNC: _ClassVar[DataSource]
    THIRD_PARTY_API: _ClassVar[DataSource]
    BATCH_PROCESSING: _ClassVar[DataSource]
    REAL_TIME_STREAM: _ClassVar[DataSource]
    MIGRATION: _ClassVar[DataSource]
    BACKUP_RESTORE: _ClassVar[DataSource]

class ExtractionMethod(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    EXTRACTION_UNSPECIFIED: _ClassVar[ExtractionMethod]
    DIRECT_TEXT: _ClassVar[ExtractionMethod]
    OCR: _ClassVar[ExtractionMethod]
    ASR: _ClassVar[ExtractionMethod]
    PDF_PARSING: _ClassVar[ExtractionMethod]
    HTML_PARSING: _ClassVar[ExtractionMethod]
    DOCUMENT_PARSING: _ClassVar[ExtractionMethod]
    IMAGE_ANALYSIS: _ClassVar[ExtractionMethod]
    VIDEO_ANALYSIS: _ClassVar[ExtractionMethod]
    API_EXTRACTION: _ClassVar[ExtractionMethod]
    MANUAL_ENTRY: _ClassVar[ExtractionMethod]

class IndexUpdateMode(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    INDEX_UPDATE_MODE_UNSPECIFIED: _ClassVar[IndexUpdateMode]
    SYNCHRONOUS: _ClassVar[IndexUpdateMode]
    ASYNCHRONOUS: _ClassVar[IndexUpdateMode]
    HYBRID_MODE: _ClassVar[IndexUpdateMode]

class VectorRepresentation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    VECTOR_REPRESENTATION_UNSPECIFIED: _ClassVar[VectorRepresentation]
    FP32_ONLY: _ClassVar[VectorRepresentation]
    QUANTIZED_ONLY: _ClassVar[VectorRepresentation]
    BOTH: _ClassVar[VectorRepresentation]
    AUTO: _ClassVar[VectorRepresentation]

class RandomProjectionType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    GAUSSIAN: _ClassVar[RandomProjectionType]
    BINARY: _ClassVar[RandomProjectionType]
    SPARSE: _ClassVar[RandomProjectionType]

class CompressionAlgorithm(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    COMPRESSION_NONE: _ClassVar[CompressionAlgorithm]
    COMPRESSION_ZSTD: _ClassVar[CompressionAlgorithm]
    COMPRESSION_LZ4: _ClassVar[CompressionAlgorithm]
    COMPRESSION_SNAPPY: _ClassVar[CompressionAlgorithm]
    COMPRESSION_GZIP: _ClassVar[CompressionAlgorithm]
    COMPRESSION_BROTLI: _ClassVar[CompressionAlgorithm]
    COMPRESSION_BZIP2: _ClassVar[CompressionAlgorithm]
    COMPRESSION_DEFLATE: _ClassVar[CompressionAlgorithm]
    COMPRESSION_XZ: _ClassVar[CompressionAlgorithm]
    COMPRESSION_ZLIB: _ClassVar[CompressionAlgorithm]
    COMPRESSION_LZO: _ClassVar[CompressionAlgorithm]
    COMPRESSION_LZ4HC: _ClassVar[CompressionAlgorithm]
    COMPRESSION_LZMA: _ClassVar[CompressionAlgorithm]

class AccessPattern(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    ACCESS_PATTERN_UNKNOWN: _ClassVar[AccessPattern]
    ACCESS_PATTERN_WRITE_HEAVY: _ClassVar[AccessPattern]
    ACCESS_PATTERN_READ_HEAVY: _ClassVar[AccessPattern]
    ACCESS_PATTERN_BALANCED: _ClassVar[AccessPattern]
    ACCESS_PATTERN_ARCHIVE: _ClassVar[AccessPattern]

class DataDensity(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    DENSITY_UNKNOWN: _ClassVar[DataDensity]
    DENSITY_DENSE: _ClassVar[DataDensity]
    DENSITY_SPARSE: _ClassVar[DataDensity]
    DENSITY_MIXED: _ClassVar[DataDensity]

class StorageTier(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    STORAGE_TIER_UNSPECIFIED: _ClassVar[StorageTier]
    HOT: _ClassVar[StorageTier]
    WARM: _ClassVar[StorageTier]
    COOL: _ClassVar[StorageTier]
    COLD: _ClassVar[StorageTier]

class ChunkingStrategy(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    CHUNKING_STRATEGY_UNSPECIFIED: _ClassVar[ChunkingStrategy]
    FIXED_SIZE: _ClassVar[ChunkingStrategy]
    SEMANTIC: _ClassVar[ChunkingStrategy]
    SENTENCE: _ClassVar[ChunkingStrategy]
    PARAGRAPH: _ClassVar[ChunkingStrategy]
    CUSTOM_CHUNKING: _ClassVar[ChunkingStrategy]

class CacheEvictionPolicy(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    CACHE_EVICTION_UNSPECIFIED: _ClassVar[CacheEvictionPolicy]
    LRU: _ClassVar[CacheEvictionPolicy]
    LFU: _ClassVar[CacheEvictionPolicy]
    ARC: _ClassVar[CacheEvictionPolicy]
    RANDOM: _ClassVar[CacheEvictionPolicy]

class ColumnEncoding(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    ENCODING_AUTO: _ClassVar[ColumnEncoding]
    ENCODING_PLAIN: _ClassVar[ColumnEncoding]
    ENCODING_DICTIONARY: _ClassVar[ColumnEncoding]
    ENCODING_RLE: _ClassVar[ColumnEncoding]
    ENCODING_DELTA: _ClassVar[ColumnEncoding]
    ENCODING_BITPACKED: _ClassVar[ColumnEncoding]

class FilterableDataType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    FILTERABLE_DATA_TYPE_UNSPECIFIED: _ClassVar[FilterableDataType]
    FILTERABLE_STRING: _ClassVar[FilterableDataType]
    FILTERABLE_INTEGER: _ClassVar[FilterableDataType]
    FILTERABLE_FLOAT: _ClassVar[FilterableDataType]
    FILTERABLE_BOOLEAN: _ClassVar[FilterableDataType]
    FILTERABLE_DATETIME: _ClassVar[FilterableDataType]
    FILTERABLE_ARRAY_STRING: _ClassVar[FilterableDataType]
    FILTERABLE_ARRAY_INTEGER: _ClassVar[FilterableDataType]
    FILTERABLE_ARRAY_FLOAT: _ClassVar[FilterableDataType]

class FilterOperator(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    FILTER_OPERATOR_UNSPECIFIED: _ClassVar[FilterOperator]
    AND: _ClassVar[FilterOperator]
    OR: _ClassVar[FilterOperator]
    NOT: _ClassVar[FilterOperator]

class FilterOperation(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    FILTER_OPERATION_UNSPECIFIED: _ClassVar[FilterOperation]
    EQUALS: _ClassVar[FilterOperation]
    NOT_EQUALS: _ClassVar[FilterOperation]
    GREATER_THAN: _ClassVar[FilterOperation]
    LESS_THAN: _ClassVar[FilterOperation]
    GREATER_THAN_OR_EQUAL: _ClassVar[FilterOperation]
    LESS_THAN_OR_EQUAL: _ClassVar[FilterOperation]
    IN: _ClassVar[FilterOperation]
    NOT_IN: _ClassVar[FilterOperation]
    CONTAINS: _ClassVar[FilterOperation]
    STARTS_WITH: _ClassVar[FilterOperation]
    ENDS_WITH: _ClassVar[FilterOperation]
DISTANCE_METRIC_UNSPECIFIED: DistanceMetric
COSINE: DistanceMetric
EUCLIDEAN: DistanceMetric
DOT_PRODUCT: DistanceMetric
HAMMING: DistanceMetric
MANHATTAN: DistanceMetric
JACCARD: DistanceMetric
CUSTOM: DistanceMetric
CHEBYSHEV: DistanceMetric
CANBERRA: DistanceMetric
MINKOWSKI: DistanceMetric
ANGULAR: DistanceMetric
BRAY_CURTIS: DistanceMetric
HELLINGER: DistanceMetric
STORAGE_ENGINE_UNSPECIFIED: StorageEngine
VIPER: StorageEngine
SST: StorageEngine
MMAP: StorageEngine
HYBRID: StorageEngine
SWIFT: StorageEngine
NOVA: StorageEngine
INDEXING_ALGORITHM_UNSPECIFIED: IndexingAlgorithm
HNSW: IndexingAlgorithm
IVF: IndexingAlgorithm
PQ: IndexingAlgorithm
FLAT: IndexingAlgorithm
ANNOY: IndexingAlgorithm
LSH: IndexingAlgorithm
COLLECTION_OPERATION_UNSPECIFIED: CollectionOperation
COLLECTION_CREATE: CollectionOperation
COLLECTION_UPDATE: CollectionOperation
COLLECTION_GET: CollectionOperation
COLLECTION_LIST: CollectionOperation
COLLECTION_DELETE: CollectionOperation
COLLECTION_MIGRATE: CollectionOperation
COLLECTION_GET_ID_BY_NAME: CollectionOperation
VECTOR_OPERATION_UNSPECIFIED: VectorOperation
VECTOR_BATCH: VectorOperation
VECTOR_SEARCH: VectorOperation
VECTOR_GET: VectorOperation
EMBEDDING_MODEL_UNSPECIFIED: EmbeddingModelType
OPENAI_TEXT_EMBEDDING_ADA_002: EmbeddingModelType
OPENAI_TEXT_EMBEDDING_3_SMALL: EmbeddingModelType
OPENAI_TEXT_EMBEDDING_3_LARGE: EmbeddingModelType
SENTENCE_TRANSFORMERS_ALL_MINILM_L6_V2: EmbeddingModelType
SENTENCE_TRANSFORMERS_ALL_MPNET_BASE_V2: EmbeddingModelType
SENTENCE_TRANSFORMERS_MULTI_QA_MPNET_BASE_DOT_V1: EmbeddingModelType
SENTENCE_TRANSFORMERS_ALL_DISTILROBERTA_V1: EmbeddingModelType
SENTENCE_TRANSFORMERS_PARAPHRASE_MULTILINGUAL_MPNET_BASE_V2: EmbeddingModelType
GOOGLE_USE_V4: EmbeddingModelType
GOOGLE_USE_MULTILINGUAL_V3: EmbeddingModelType
GOOGLE_USE_LITE: EmbeddingModelType
COHERE_EMBED_ENGLISH_V3: EmbeddingModelType
COHERE_EMBED_MULTILINGUAL_V3: EmbeddingModelType
COHERE_EMBED_ENGLISH_LIGHT_V3: EmbeddingModelType
ANTHROPIC_VOYAGE_2: EmbeddingModelType
ANTHROPIC_VOYAGE_CODE_2: EmbeddingModelType
MISTRAL_EMBED: EmbeddingModelType
BGE_LARGE_EN_V1_5: EmbeddingModelType
BGE_BASE_EN_V1_5: EmbeddingModelType
BGE_SMALL_EN_V1_5: EmbeddingModelType
BGE_M3: EmbeddingModelType
E5_LARGE_V2: EmbeddingModelType
E5_BASE_V2: EmbeddingModelType
E5_SMALL_V2: EmbeddingModelType
INSTRUCTOR_XL: EmbeddingModelType
INSTRUCTOR_LARGE: EmbeddingModelType
INSTRUCTOR_BASE: EmbeddingModelType
CUSTOM_EMBEDDING_MODEL: EmbeddingModelType
CONTENT_CATEGORY_UNSPECIFIED: ContentCategory
DOCUMENT: ContentCategory
IMAGE: ContentCategory
AUDIO: ContentCategory
VIDEO: ContentCategory
CODE: ContentCategory
TABLE: ContentCategory
CHART: ContentCategory
EMAIL: ContentCategory
WEBPAGE: ContentCategory
SOCIAL_MEDIA: ContentCategory
KNOWLEDGE_BASE: ContentCategory
SCIENTIFIC: ContentCategory
LEGAL: ContentCategory
FINANCIAL: ContentCategory
MEDICAL: ContentCategory
QUALITY_UNSPECIFIED: QualityLevel
HIGH: QualityLevel
MEDIUM: QualityLevel
LOW: QualityLevel
UNKNOWN: QualityLevel
PROCESSING_STATUS_UNSPECIFIED: ProcessingStatus
RAW: ProcessingStatus
PROCESSING: ProcessingStatus
PROCESSED: ProcessingStatus
FAILED: ProcessingStatus
REQUIRES_REVIEW: ProcessingStatus
APPROVED: ProcessingStatus
DEPRECATED: ProcessingStatus
LANGUAGE_UNSPECIFIED: LanguageCode
ENGLISH: LanguageCode
SPANISH: LanguageCode
FRENCH: LanguageCode
GERMAN: LanguageCode
ITALIAN: LanguageCode
PORTUGUESE: LanguageCode
RUSSIAN: LanguageCode
CHINESE: LanguageCode
JAPANESE: LanguageCode
KOREAN: LanguageCode
ARABIC: LanguageCode
HINDI: LanguageCode
DUTCH: LanguageCode
SWEDISH: LanguageCode
NORWEGIAN: LanguageCode
DANISH: LanguageCode
FINNISH: LanguageCode
POLISH: LanguageCode
CZECH: LanguageCode
HUNGARIAN: LanguageCode
TURKISH: LanguageCode
GREEK: LanguageCode
HEBREW: LanguageCode
THAI: LanguageCode
VIETNAMESE: LanguageCode
INDONESIAN: LanguageCode
MALAY: LanguageCode
FILIPINO: LanguageCode
CUSTOM_LANGUAGE: LanguageCode
DATA_SOURCE_UNSPECIFIED: DataSource
USER_UPLOAD: DataSource
API_INGESTION: DataSource
WEB_SCRAPING: DataSource
FILE_IMPORT: DataSource
DATABASE_SYNC: DataSource
THIRD_PARTY_API: DataSource
BATCH_PROCESSING: DataSource
REAL_TIME_STREAM: DataSource
MIGRATION: DataSource
BACKUP_RESTORE: DataSource
EXTRACTION_UNSPECIFIED: ExtractionMethod
DIRECT_TEXT: ExtractionMethod
OCR: ExtractionMethod
ASR: ExtractionMethod
PDF_PARSING: ExtractionMethod
HTML_PARSING: ExtractionMethod
DOCUMENT_PARSING: ExtractionMethod
IMAGE_ANALYSIS: ExtractionMethod
VIDEO_ANALYSIS: ExtractionMethod
API_EXTRACTION: ExtractionMethod
MANUAL_ENTRY: ExtractionMethod
INDEX_UPDATE_MODE_UNSPECIFIED: IndexUpdateMode
SYNCHRONOUS: IndexUpdateMode
ASYNCHRONOUS: IndexUpdateMode
HYBRID_MODE: IndexUpdateMode
VECTOR_REPRESENTATION_UNSPECIFIED: VectorRepresentation
FP32_ONLY: VectorRepresentation
QUANTIZED_ONLY: VectorRepresentation
BOTH: VectorRepresentation
AUTO: VectorRepresentation
GAUSSIAN: RandomProjectionType
BINARY: RandomProjectionType
SPARSE: RandomProjectionType
COMPRESSION_NONE: CompressionAlgorithm
COMPRESSION_ZSTD: CompressionAlgorithm
COMPRESSION_LZ4: CompressionAlgorithm
COMPRESSION_SNAPPY: CompressionAlgorithm
COMPRESSION_GZIP: CompressionAlgorithm
COMPRESSION_BROTLI: CompressionAlgorithm
COMPRESSION_BZIP2: CompressionAlgorithm
COMPRESSION_DEFLATE: CompressionAlgorithm
COMPRESSION_XZ: CompressionAlgorithm
COMPRESSION_ZLIB: CompressionAlgorithm
COMPRESSION_LZO: CompressionAlgorithm
COMPRESSION_LZ4HC: CompressionAlgorithm
COMPRESSION_LZMA: CompressionAlgorithm
ACCESS_PATTERN_UNKNOWN: AccessPattern
ACCESS_PATTERN_WRITE_HEAVY: AccessPattern
ACCESS_PATTERN_READ_HEAVY: AccessPattern
ACCESS_PATTERN_BALANCED: AccessPattern
ACCESS_PATTERN_ARCHIVE: AccessPattern
DENSITY_UNKNOWN: DataDensity
DENSITY_DENSE: DataDensity
DENSITY_SPARSE: DataDensity
DENSITY_MIXED: DataDensity
STORAGE_TIER_UNSPECIFIED: StorageTier
HOT: StorageTier
WARM: StorageTier
COOL: StorageTier
COLD: StorageTier
CHUNKING_STRATEGY_UNSPECIFIED: ChunkingStrategy
FIXED_SIZE: ChunkingStrategy
SEMANTIC: ChunkingStrategy
SENTENCE: ChunkingStrategy
PARAGRAPH: ChunkingStrategy
CUSTOM_CHUNKING: ChunkingStrategy
CACHE_EVICTION_UNSPECIFIED: CacheEvictionPolicy
LRU: CacheEvictionPolicy
LFU: CacheEvictionPolicy
ARC: CacheEvictionPolicy
RANDOM: CacheEvictionPolicy
ENCODING_AUTO: ColumnEncoding
ENCODING_PLAIN: ColumnEncoding
ENCODING_DICTIONARY: ColumnEncoding
ENCODING_RLE: ColumnEncoding
ENCODING_DELTA: ColumnEncoding
ENCODING_BITPACKED: ColumnEncoding
FILTERABLE_DATA_TYPE_UNSPECIFIED: FilterableDataType
FILTERABLE_STRING: FilterableDataType
FILTERABLE_INTEGER: FilterableDataType
FILTERABLE_FLOAT: FilterableDataType
FILTERABLE_BOOLEAN: FilterableDataType
FILTERABLE_DATETIME: FilterableDataType
FILTERABLE_ARRAY_STRING: FilterableDataType
FILTERABLE_ARRAY_INTEGER: FilterableDataType
FILTERABLE_ARRAY_FLOAT: FilterableDataType
FILTER_OPERATOR_UNSPECIFIED: FilterOperator
AND: FilterOperator
OR: FilterOperator
NOT: FilterOperator
FILTER_OPERATION_UNSPECIFIED: FilterOperation
EQUALS: FilterOperation
NOT_EQUALS: FilterOperation
GREATER_THAN: FilterOperation
LESS_THAN: FilterOperation
GREATER_THAN_OR_EQUAL: FilterOperation
LESS_THAN_OR_EQUAL: FilterOperation
IN: FilterOperation
NOT_IN: FilterOperation
CONTAINS: FilterOperation
STARTS_WITH: FilterOperation
ENDS_WITH: FilterOperation

class MetadataItem(_message.Message):
    __slots__ = ("key", "string_value", "number_value", "bool_value")
    KEY_FIELD_NUMBER: _ClassVar[int]
    STRING_VALUE_FIELD_NUMBER: _ClassVar[int]
    NUMBER_VALUE_FIELD_NUMBER: _ClassVar[int]
    BOOL_VALUE_FIELD_NUMBER: _ClassVar[int]
    key: str
    string_value: str
    number_value: float
    bool_value: bool
    def __init__(self, key: _Optional[str] = ..., string_value: _Optional[str] = ..., number_value: _Optional[float] = ..., bool_value: bool = ...) -> None: ...

class EmbeddingModelRegistry(_message.Message):
    __slots__ = ("models",)
    class ModelsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: EmbeddingModelSpec
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[EmbeddingModelSpec, _Mapping]] = ...) -> None: ...
    MODELS_FIELD_NUMBER: _ClassVar[int]
    models: _containers.MessageMap[str, EmbeddingModelSpec]
    def __init__(self, models: _Optional[_Mapping[str, EmbeddingModelSpec]] = ...) -> None: ...

class EmbeddingModelSpec(_message.Message):
    __slots__ = ("model_type", "custom_model", "version", "provider", "dimension", "max_sequence_length", "normalization", "supports_batch")
    MODEL_TYPE_FIELD_NUMBER: _ClassVar[int]
    CUSTOM_MODEL_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    PROVIDER_FIELD_NUMBER: _ClassVar[int]
    DIMENSION_FIELD_NUMBER: _ClassVar[int]
    MAX_SEQUENCE_LENGTH_FIELD_NUMBER: _ClassVar[int]
    NORMALIZATION_FIELD_NUMBER: _ClassVar[int]
    SUPPORTS_BATCH_FIELD_NUMBER: _ClassVar[int]
    model_type: EmbeddingModelType
    custom_model: str
    version: str
    provider: str
    dimension: int
    max_sequence_length: float
    normalization: str
    supports_batch: bool
    def __init__(self, model_type: _Optional[_Union[EmbeddingModelType, str]] = ..., custom_model: _Optional[str] = ..., version: _Optional[str] = ..., provider: _Optional[str] = ..., dimension: _Optional[int] = ..., max_sequence_length: _Optional[float] = ..., normalization: _Optional[str] = ..., supports_batch: bool = ...) -> None: ...

class SourceContent(_message.Message):
    __slots__ = ("text", "binary", "external", "structured", "packed_attributes", "mime_type", "size_bytes", "compressed_size", "checksum", "processing")
    TEXT_FIELD_NUMBER: _ClassVar[int]
    BINARY_FIELD_NUMBER: _ClassVar[int]
    EXTERNAL_FIELD_NUMBER: _ClassVar[int]
    STRUCTURED_FIELD_NUMBER: _ClassVar[int]
    PACKED_ATTRIBUTES_FIELD_NUMBER: _ClassVar[int]
    MIME_TYPE_FIELD_NUMBER: _ClassVar[int]
    SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    COMPRESSED_SIZE_FIELD_NUMBER: _ClassVar[int]
    CHECKSUM_FIELD_NUMBER: _ClassVar[int]
    PROCESSING_FIELD_NUMBER: _ClassVar[int]
    text: TextContent
    binary: BinaryContent
    external: ExternalContent
    structured: StructuredContent
    packed_attributes: int
    mime_type: str
    size_bytes: int
    compressed_size: int
    checksum: int
    processing: ProcessingInfo
    def __init__(self, text: _Optional[_Union[TextContent, _Mapping]] = ..., binary: _Optional[_Union[BinaryContent, _Mapping]] = ..., external: _Optional[_Union[ExternalContent, _Mapping]] = ..., structured: _Optional[_Union[StructuredContent, _Mapping]] = ..., packed_attributes: _Optional[int] = ..., mime_type: _Optional[str] = ..., size_bytes: _Optional[int] = ..., compressed_size: _Optional[int] = ..., checksum: _Optional[int] = ..., processing: _Optional[_Union[ProcessingInfo, _Mapping]] = ...) -> None: ...

class TextContent(_message.Message):
    __slots__ = ("content", "language_code", "custom_language", "chunk")
    CONTENT_FIELD_NUMBER: _ClassVar[int]
    LANGUAGE_CODE_FIELD_NUMBER: _ClassVar[int]
    CUSTOM_LANGUAGE_FIELD_NUMBER: _ClassVar[int]
    CHUNK_FIELD_NUMBER: _ClassVar[int]
    content: str
    language_code: int
    custom_language: str
    chunk: ChunkContext
    def __init__(self, content: _Optional[str] = ..., language_code: _Optional[int] = ..., custom_language: _Optional[str] = ..., chunk: _Optional[_Union[ChunkContext, _Mapping]] = ...) -> None: ...

class BinaryContent(_message.Message):
    __slots__ = ("data", "media")
    DATA_FIELD_NUMBER: _ClassVar[int]
    MEDIA_FIELD_NUMBER: _ClassVar[int]
    data: bytes
    media: MediaMetadata
    def __init__(self, data: _Optional[bytes] = ..., media: _Optional[_Union[MediaMetadata, _Mapping]] = ...) -> None: ...

class ExternalContent(_message.Message):
    __slots__ = ("uri", "storage_backend", "access", "cache")
    class AccessEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    URI_FIELD_NUMBER: _ClassVar[int]
    STORAGE_BACKEND_FIELD_NUMBER: _ClassVar[int]
    ACCESS_FIELD_NUMBER: _ClassVar[int]
    CACHE_FIELD_NUMBER: _ClassVar[int]
    uri: str
    storage_backend: str
    access: _containers.ScalarMap[str, str]
    cache: CachePolicy
    def __init__(self, uri: _Optional[str] = ..., storage_backend: _Optional[str] = ..., access: _Optional[_Mapping[str, str]] = ..., cache: _Optional[_Union[CachePolicy, _Mapping]] = ...) -> None: ...

class StructuredContent(_message.Message):
    __slots__ = ("data", "schema_version")
    DATA_FIELD_NUMBER: _ClassVar[int]
    SCHEMA_VERSION_FIELD_NUMBER: _ClassVar[int]
    data: _struct_pb2.Struct
    schema_version: str
    def __init__(self, data: _Optional[_Union[_struct_pb2.Struct, _Mapping]] = ..., schema_version: _Optional[str] = ...) -> None: ...

class ChunkContext(_message.Message):
    __slots__ = ("document_id", "document_title", "chunk_index", "total_chunks", "char_start", "char_end", "token_start", "token_end", "preceding_text", "following_text", "section_path")
    DOCUMENT_ID_FIELD_NUMBER: _ClassVar[int]
    DOCUMENT_TITLE_FIELD_NUMBER: _ClassVar[int]
    CHUNK_INDEX_FIELD_NUMBER: _ClassVar[int]
    TOTAL_CHUNKS_FIELD_NUMBER: _ClassVar[int]
    CHAR_START_FIELD_NUMBER: _ClassVar[int]
    CHAR_END_FIELD_NUMBER: _ClassVar[int]
    TOKEN_START_FIELD_NUMBER: _ClassVar[int]
    TOKEN_END_FIELD_NUMBER: _ClassVar[int]
    PRECEDING_TEXT_FIELD_NUMBER: _ClassVar[int]
    FOLLOWING_TEXT_FIELD_NUMBER: _ClassVar[int]
    SECTION_PATH_FIELD_NUMBER: _ClassVar[int]
    document_id: str
    document_title: str
    chunk_index: int
    total_chunks: int
    char_start: int
    char_end: int
    token_start: int
    token_end: int
    preceding_text: str
    following_text: str
    section_path: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, document_id: _Optional[str] = ..., document_title: _Optional[str] = ..., chunk_index: _Optional[int] = ..., total_chunks: _Optional[int] = ..., char_start: _Optional[int] = ..., char_end: _Optional[int] = ..., token_start: _Optional[int] = ..., token_end: _Optional[int] = ..., preceding_text: _Optional[str] = ..., following_text: _Optional[str] = ..., section_path: _Optional[_Iterable[str]] = ...) -> None: ...

class MediaMetadata(_message.Message):
    __slots__ = ("width", "height", "duration_ms", "bitrate", "codec", "exif")
    class ExifEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    WIDTH_FIELD_NUMBER: _ClassVar[int]
    HEIGHT_FIELD_NUMBER: _ClassVar[int]
    DURATION_MS_FIELD_NUMBER: _ClassVar[int]
    BITRATE_FIELD_NUMBER: _ClassVar[int]
    CODEC_FIELD_NUMBER: _ClassVar[int]
    EXIF_FIELD_NUMBER: _ClassVar[int]
    width: int
    height: int
    duration_ms: int
    bitrate: int
    codec: str
    exif: _containers.ScalarMap[str, str]
    def __init__(self, width: _Optional[int] = ..., height: _Optional[int] = ..., duration_ms: _Optional[int] = ..., bitrate: _Optional[int] = ..., codec: _Optional[str] = ..., exif: _Optional[_Mapping[str, str]] = ...) -> None: ...

class ProcessingInfo(_message.Message):
    __slots__ = ("model_id", "packed_enums", "processing_time_ms", "processor_version")
    MODEL_ID_FIELD_NUMBER: _ClassVar[int]
    PACKED_ENUMS_FIELD_NUMBER: _ClassVar[int]
    PROCESSING_TIME_MS_FIELD_NUMBER: _ClassVar[int]
    PROCESSOR_VERSION_FIELD_NUMBER: _ClassVar[int]
    model_id: str
    packed_enums: int
    processing_time_ms: int
    processor_version: int
    def __init__(self, model_id: _Optional[str] = ..., packed_enums: _Optional[int] = ..., processing_time_ms: _Optional[int] = ..., processor_version: _Optional[int] = ...) -> None: ...

class CachePolicy(_message.Message):
    __slots__ = ("ttl_seconds", "prefetch", "tier")
    TTL_SECONDS_FIELD_NUMBER: _ClassVar[int]
    PREFETCH_FIELD_NUMBER: _ClassVar[int]
    TIER_FIELD_NUMBER: _ClassVar[int]
    ttl_seconds: int
    prefetch: bool
    tier: str
    def __init__(self, ttl_seconds: _Optional[int] = ..., prefetch: bool = ..., tier: _Optional[str] = ...) -> None: ...

class VectorRecord(_message.Message):
    __slots__ = ("id", "vector", "metadata", "timestamp", "updated_at", "expires_at", "version", "quantized_vector", "source")
    ID_FIELD_NUMBER: _ClassVar[int]
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_FIELD_NUMBER: _ClassVar[int]
    UPDATED_AT_FIELD_NUMBER: _ClassVar[int]
    EXPIRES_AT_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    QUANTIZED_VECTOR_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    id: str
    vector: _containers.RepeatedScalarFieldContainer[float]
    metadata: _containers.RepeatedCompositeFieldContainer[MetadataItem]
    timestamp: int
    updated_at: int
    expires_at: int
    version: int
    quantized_vector: bytes
    source: SourceContent
    def __init__(self, id: _Optional[str] = ..., vector: _Optional[_Iterable[float]] = ..., metadata: _Optional[_Iterable[_Union[MetadataItem, _Mapping]]] = ..., timestamp: _Optional[int] = ..., updated_at: _Optional[int] = ..., expires_at: _Optional[int] = ..., version: _Optional[int] = ..., quantized_vector: _Optional[bytes] = ..., source: _Optional[_Union[SourceContent, _Mapping]] = ...) -> None: ...

class SearchVectorRecord(_message.Message):
    __slots__ = ("id", "vector", "metadata", "score", "similarity", "version", "timestamp", "source", "expanded_context")
    ID_FIELD_NUMBER: _ClassVar[int]
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    SIMILARITY_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    EXPANDED_CONTEXT_FIELD_NUMBER: _ClassVar[int]
    id: str
    vector: _containers.RepeatedScalarFieldContainer[float]
    metadata: _containers.RepeatedCompositeFieldContainer[MetadataItem]
    score: float
    similarity: float
    version: int
    timestamp: int
    source: SourceContent
    expanded_context: _containers.RepeatedCompositeFieldContainer[SourceContent]
    def __init__(self, id: _Optional[str] = ..., vector: _Optional[_Iterable[float]] = ..., metadata: _Optional[_Iterable[_Union[MetadataItem, _Mapping]]] = ..., score: _Optional[float] = ..., similarity: _Optional[float] = ..., version: _Optional[int] = ..., timestamp: _Optional[int] = ..., source: _Optional[_Union[SourceContent, _Mapping]] = ..., expanded_context: _Optional[_Iterable[_Union[SourceContent, _Mapping]]] = ...) -> None: ...

class MetadataMap(_message.Message):
    __slots__ = ("fields",)
    class FieldsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: MetadataValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[MetadataValue, _Mapping]] = ...) -> None: ...
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    fields: _containers.MessageMap[str, MetadataValue]
    def __init__(self, fields: _Optional[_Mapping[str, MetadataValue]] = ...) -> None: ...

class MetadataValue(_message.Message):
    __slots__ = ("string_value", "int_value", "double_value", "bool_value", "string_array", "int_array", "double_array")
    STRING_VALUE_FIELD_NUMBER: _ClassVar[int]
    INT_VALUE_FIELD_NUMBER: _ClassVar[int]
    DOUBLE_VALUE_FIELD_NUMBER: _ClassVar[int]
    BOOL_VALUE_FIELD_NUMBER: _ClassVar[int]
    STRING_ARRAY_FIELD_NUMBER: _ClassVar[int]
    INT_ARRAY_FIELD_NUMBER: _ClassVar[int]
    DOUBLE_ARRAY_FIELD_NUMBER: _ClassVar[int]
    string_value: str
    int_value: int
    double_value: float
    bool_value: bool
    string_array: StringArray
    int_array: Int64Array
    double_array: DoubleArray
    def __init__(self, string_value: _Optional[str] = ..., int_value: _Optional[int] = ..., double_value: _Optional[float] = ..., bool_value: bool = ..., string_array: _Optional[_Union[StringArray, _Mapping]] = ..., int_array: _Optional[_Union[Int64Array, _Mapping]] = ..., double_array: _Optional[_Union[DoubleArray, _Mapping]] = ...) -> None: ...

class StringArray(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, values: _Optional[_Iterable[str]] = ...) -> None: ...

class Int64Array(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedScalarFieldContainer[int]
    def __init__(self, values: _Optional[_Iterable[int]] = ...) -> None: ...

class DoubleArray(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedScalarFieldContainer[float]
    def __init__(self, values: _Optional[_Iterable[float]] = ...) -> None: ...

class CollectionConfig(_message.Message):
    __slots__ = ("name", "dimension", "distance_metric", "storage_engine", "storage_config", "index_configs", "primary_index", "auto_index_selection", "filterable_columns", "quantization", "embedding_models", "description", "tags", "owner")
    NAME_FIELD_NUMBER: _ClassVar[int]
    DIMENSION_FIELD_NUMBER: _ClassVar[int]
    DISTANCE_METRIC_FIELD_NUMBER: _ClassVar[int]
    STORAGE_ENGINE_FIELD_NUMBER: _ClassVar[int]
    STORAGE_CONFIG_FIELD_NUMBER: _ClassVar[int]
    INDEX_CONFIGS_FIELD_NUMBER: _ClassVar[int]
    PRIMARY_INDEX_FIELD_NUMBER: _ClassVar[int]
    AUTO_INDEX_SELECTION_FIELD_NUMBER: _ClassVar[int]
    FILTERABLE_COLUMNS_FIELD_NUMBER: _ClassVar[int]
    QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    EMBEDDING_MODELS_FIELD_NUMBER: _ClassVar[int]
    DESCRIPTION_FIELD_NUMBER: _ClassVar[int]
    TAGS_FIELD_NUMBER: _ClassVar[int]
    OWNER_FIELD_NUMBER: _ClassVar[int]
    name: str
    dimension: int
    distance_metric: DistanceMetric
    storage_engine: StorageEngine
    storage_config: StorageConfig
    index_configs: _containers.RepeatedCompositeFieldContainer[IndexConfig]
    primary_index: str
    auto_index_selection: bool
    filterable_columns: _containers.RepeatedCompositeFieldContainer[FilterableColumnSpec]
    quantization: QuantizationConfig
    embedding_models: EmbeddingModelRegistry
    description: str
    tags: _containers.RepeatedScalarFieldContainer[str]
    owner: str
    def __init__(self, name: _Optional[str] = ..., dimension: _Optional[int] = ..., distance_metric: _Optional[_Union[DistanceMetric, str]] = ..., storage_engine: _Optional[_Union[StorageEngine, str]] = ..., storage_config: _Optional[_Union[StorageConfig, _Mapping]] = ..., index_configs: _Optional[_Iterable[_Union[IndexConfig, _Mapping]]] = ..., primary_index: _Optional[str] = ..., auto_index_selection: bool = ..., filterable_columns: _Optional[_Iterable[_Union[FilterableColumnSpec, _Mapping]]] = ..., quantization: _Optional[_Union[QuantizationConfig, _Mapping]] = ..., embedding_models: _Optional[_Union[EmbeddingModelRegistry, _Mapping]] = ..., description: _Optional[str] = ..., tags: _Optional[_Iterable[str]] = ..., owner: _Optional[str] = ...) -> None: ...

class IndexConfig(_message.Message):
    __slots__ = ("index_name", "algorithm", "update_mode", "async_update_timeout_ms", "async_update_batch_size", "enable_background_optimization", "hnsw_config", "ivf_config", "flat_config", "pq_config", "annoy_config", "lsh_config", "build_concurrency", "memory_limit_mb", "checkpoint_interval_ms", "is_primary", "use_cases", "selectivity_threshold", "use_quantization", "quantization_override", "queue_representation")
    INDEX_NAME_FIELD_NUMBER: _ClassVar[int]
    ALGORITHM_FIELD_NUMBER: _ClassVar[int]
    UPDATE_MODE_FIELD_NUMBER: _ClassVar[int]
    ASYNC_UPDATE_TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    ASYNC_UPDATE_BATCH_SIZE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_BACKGROUND_OPTIMIZATION_FIELD_NUMBER: _ClassVar[int]
    HNSW_CONFIG_FIELD_NUMBER: _ClassVar[int]
    IVF_CONFIG_FIELD_NUMBER: _ClassVar[int]
    FLAT_CONFIG_FIELD_NUMBER: _ClassVar[int]
    PQ_CONFIG_FIELD_NUMBER: _ClassVar[int]
    ANNOY_CONFIG_FIELD_NUMBER: _ClassVar[int]
    LSH_CONFIG_FIELD_NUMBER: _ClassVar[int]
    BUILD_CONCURRENCY_FIELD_NUMBER: _ClassVar[int]
    MEMORY_LIMIT_MB_FIELD_NUMBER: _ClassVar[int]
    CHECKPOINT_INTERVAL_MS_FIELD_NUMBER: _ClassVar[int]
    IS_PRIMARY_FIELD_NUMBER: _ClassVar[int]
    USE_CASES_FIELD_NUMBER: _ClassVar[int]
    SELECTIVITY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    USE_QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    QUANTIZATION_OVERRIDE_FIELD_NUMBER: _ClassVar[int]
    QUEUE_REPRESENTATION_FIELD_NUMBER: _ClassVar[int]
    index_name: str
    algorithm: IndexingAlgorithm
    update_mode: IndexUpdateMode
    async_update_timeout_ms: int
    async_update_batch_size: int
    enable_background_optimization: bool
    hnsw_config: HnswConfig
    ivf_config: IvfConfig
    flat_config: FlatConfig
    pq_config: PqConfig
    annoy_config: AnnoyConfig
    lsh_config: LshConfig
    build_concurrency: int
    memory_limit_mb: int
    checkpoint_interval_ms: int
    is_primary: bool
    use_cases: _containers.RepeatedScalarFieldContainer[str]
    selectivity_threshold: float
    use_quantization: bool
    quantization_override: QuantizationConfig
    queue_representation: VectorRepresentation
    def __init__(self, index_name: _Optional[str] = ..., algorithm: _Optional[_Union[IndexingAlgorithm, str]] = ..., update_mode: _Optional[_Union[IndexUpdateMode, str]] = ..., async_update_timeout_ms: _Optional[int] = ..., async_update_batch_size: _Optional[int] = ..., enable_background_optimization: bool = ..., hnsw_config: _Optional[_Union[HnswConfig, _Mapping]] = ..., ivf_config: _Optional[_Union[IvfConfig, _Mapping]] = ..., flat_config: _Optional[_Union[FlatConfig, _Mapping]] = ..., pq_config: _Optional[_Union[PqConfig, _Mapping]] = ..., annoy_config: _Optional[_Union[AnnoyConfig, _Mapping]] = ..., lsh_config: _Optional[_Union[LshConfig, _Mapping]] = ..., build_concurrency: _Optional[int] = ..., memory_limit_mb: _Optional[int] = ..., checkpoint_interval_ms: _Optional[int] = ..., is_primary: bool = ..., use_cases: _Optional[_Iterable[str]] = ..., selectivity_threshold: _Optional[float] = ..., use_quantization: bool = ..., quantization_override: _Optional[_Union[QuantizationConfig, _Mapping]] = ..., queue_representation: _Optional[_Union[VectorRepresentation, str]] = ...) -> None: ...

class HnswConfig(_message.Message):
    __slots__ = ("m", "ef_construction", "ef_search", "max_partition_size", "adaptive_parameters", "use_simd", "memory_limit_mb", "lazy_loading", "prune_connections", "level_multiplier")
    M_FIELD_NUMBER: _ClassVar[int]
    EF_CONSTRUCTION_FIELD_NUMBER: _ClassVar[int]
    EF_SEARCH_FIELD_NUMBER: _ClassVar[int]
    MAX_PARTITION_SIZE_FIELD_NUMBER: _ClassVar[int]
    ADAPTIVE_PARAMETERS_FIELD_NUMBER: _ClassVar[int]
    USE_SIMD_FIELD_NUMBER: _ClassVar[int]
    MEMORY_LIMIT_MB_FIELD_NUMBER: _ClassVar[int]
    LAZY_LOADING_FIELD_NUMBER: _ClassVar[int]
    PRUNE_CONNECTIONS_FIELD_NUMBER: _ClassVar[int]
    LEVEL_MULTIPLIER_FIELD_NUMBER: _ClassVar[int]
    m: int
    ef_construction: int
    ef_search: int
    max_partition_size: int
    adaptive_parameters: bool
    use_simd: bool
    memory_limit_mb: int
    lazy_loading: bool
    prune_connections: int
    level_multiplier: float
    def __init__(self, m: _Optional[int] = ..., ef_construction: _Optional[int] = ..., ef_search: _Optional[int] = ..., max_partition_size: _Optional[int] = ..., adaptive_parameters: bool = ..., use_simd: bool = ..., memory_limit_mb: _Optional[int] = ..., lazy_loading: bool = ..., prune_connections: _Optional[int] = ..., level_multiplier: _Optional[float] = ...) -> None: ...

class IvfConfig(_message.Message):
    __slots__ = ("n_lists", "n_probe", "quantization_bits", "use_pq", "pq_subspaces", "train_on_insert", "min_train_size")
    N_LISTS_FIELD_NUMBER: _ClassVar[int]
    N_PROBE_FIELD_NUMBER: _ClassVar[int]
    QUANTIZATION_BITS_FIELD_NUMBER: _ClassVar[int]
    USE_PQ_FIELD_NUMBER: _ClassVar[int]
    PQ_SUBSPACES_FIELD_NUMBER: _ClassVar[int]
    TRAIN_ON_INSERT_FIELD_NUMBER: _ClassVar[int]
    MIN_TRAIN_SIZE_FIELD_NUMBER: _ClassVar[int]
    n_lists: int
    n_probe: int
    quantization_bits: int
    use_pq: bool
    pq_subspaces: int
    train_on_insert: bool
    min_train_size: int
    def __init__(self, n_lists: _Optional[int] = ..., n_probe: _Optional[int] = ..., quantization_bits: _Optional[int] = ..., use_pq: bool = ..., pq_subspaces: _Optional[int] = ..., train_on_insert: bool = ..., min_train_size: _Optional[int] = ...) -> None: ...

class FlatConfig(_message.Message):
    __slots__ = ("enable_simd", "batch_size", "enable_parallel_search")
    ENABLE_SIMD_FIELD_NUMBER: _ClassVar[int]
    BATCH_SIZE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PARALLEL_SEARCH_FIELD_NUMBER: _ClassVar[int]
    enable_simd: bool
    batch_size: int
    enable_parallel_search: bool
    def __init__(self, enable_simd: bool = ..., batch_size: _Optional[int] = ..., enable_parallel_search: bool = ...) -> None: ...

class PqConfig(_message.Message):
    __slots__ = ("subvectors", "bits_per_subvector", "training_sample_count", "enable_reranking")
    SUBVECTORS_FIELD_NUMBER: _ClassVar[int]
    BITS_PER_SUBVECTOR_FIELD_NUMBER: _ClassVar[int]
    TRAINING_SAMPLE_COUNT_FIELD_NUMBER: _ClassVar[int]
    ENABLE_RERANKING_FIELD_NUMBER: _ClassVar[int]
    subvectors: int
    bits_per_subvector: int
    training_sample_count: int
    enable_reranking: bool
    def __init__(self, subvectors: _Optional[int] = ..., bits_per_subvector: _Optional[int] = ..., training_sample_count: _Optional[int] = ..., enable_reranking: bool = ...) -> None: ...

class AnnoyConfig(_message.Message):
    __slots__ = ("n_trees", "search_k", "max_leaf_size", "enable_mmap")
    N_TREES_FIELD_NUMBER: _ClassVar[int]
    SEARCH_K_FIELD_NUMBER: _ClassVar[int]
    MAX_LEAF_SIZE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_MMAP_FIELD_NUMBER: _ClassVar[int]
    n_trees: int
    search_k: int
    max_leaf_size: int
    enable_mmap: bool
    def __init__(self, n_trees: _Optional[int] = ..., search_k: _Optional[int] = ..., max_leaf_size: _Optional[int] = ..., enable_mmap: bool = ...) -> None: ...

class LshConfig(_message.Message):
    __slots__ = ("n_hash_tables", "n_hash_functions", "bucket_width", "binary_vectors", "max_candidates", "projection")
    N_HASH_TABLES_FIELD_NUMBER: _ClassVar[int]
    N_HASH_FUNCTIONS_FIELD_NUMBER: _ClassVar[int]
    BUCKET_WIDTH_FIELD_NUMBER: _ClassVar[int]
    BINARY_VECTORS_FIELD_NUMBER: _ClassVar[int]
    MAX_CANDIDATES_FIELD_NUMBER: _ClassVar[int]
    PROJECTION_FIELD_NUMBER: _ClassVar[int]
    n_hash_tables: int
    n_hash_functions: int
    bucket_width: float
    binary_vectors: bool
    max_candidates: int
    projection: RandomProjectionType
    def __init__(self, n_hash_tables: _Optional[int] = ..., n_hash_functions: _Optional[int] = ..., bucket_width: _Optional[float] = ..., binary_vectors: bool = ..., max_candidates: _Optional[int] = ..., projection: _Optional[_Union[RandomProjectionType, str]] = ...) -> None: ...

class CompressionConfig(_message.Message):
    __slots__ = ("algorithm", "level", "adaptive", "min_ratio", "enable_quantization", "quantization_type", "normalization_method", "block_size_kb", "dynamic_block_sizing")
    ALGORITHM_FIELD_NUMBER: _ClassVar[int]
    LEVEL_FIELD_NUMBER: _ClassVar[int]
    ADAPTIVE_FIELD_NUMBER: _ClassVar[int]
    MIN_RATIO_FIELD_NUMBER: _ClassVar[int]
    ENABLE_QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    QUANTIZATION_TYPE_FIELD_NUMBER: _ClassVar[int]
    NORMALIZATION_METHOD_FIELD_NUMBER: _ClassVar[int]
    BLOCK_SIZE_KB_FIELD_NUMBER: _ClassVar[int]
    DYNAMIC_BLOCK_SIZING_FIELD_NUMBER: _ClassVar[int]
    algorithm: CompressionAlgorithm
    level: int
    adaptive: bool
    min_ratio: float
    enable_quantization: bool
    quantization_type: str
    normalization_method: str
    block_size_kb: int
    dynamic_block_sizing: bool
    def __init__(self, algorithm: _Optional[_Union[CompressionAlgorithm, str]] = ..., level: _Optional[int] = ..., adaptive: bool = ..., min_ratio: _Optional[float] = ..., enable_quantization: bool = ..., quantization_type: _Optional[str] = ..., normalization_method: _Optional[str] = ..., block_size_kb: _Optional[int] = ..., dynamic_block_sizing: bool = ...) -> None: ...

class StorageConfig(_message.Message):
    __slots__ = ("storage_location", "persistent", "compression", "source_storage", "access_pattern", "data_density", "frequent_updates", "expected_size_mb", "read_write_ratio", "preset", "enable_all_optimizations", "parquet_writer", "footer_cache", "hybrid_writer", "sst_settings", "viper_settings", "nova_settings")
    STORAGE_LOCATION_FIELD_NUMBER: _ClassVar[int]
    PERSISTENT_FIELD_NUMBER: _ClassVar[int]
    COMPRESSION_FIELD_NUMBER: _ClassVar[int]
    SOURCE_STORAGE_FIELD_NUMBER: _ClassVar[int]
    ACCESS_PATTERN_FIELD_NUMBER: _ClassVar[int]
    DATA_DENSITY_FIELD_NUMBER: _ClassVar[int]
    FREQUENT_UPDATES_FIELD_NUMBER: _ClassVar[int]
    EXPECTED_SIZE_MB_FIELD_NUMBER: _ClassVar[int]
    READ_WRITE_RATIO_FIELD_NUMBER: _ClassVar[int]
    PRESET_FIELD_NUMBER: _ClassVar[int]
    ENABLE_ALL_OPTIMIZATIONS_FIELD_NUMBER: _ClassVar[int]
    PARQUET_WRITER_FIELD_NUMBER: _ClassVar[int]
    FOOTER_CACHE_FIELD_NUMBER: _ClassVar[int]
    HYBRID_WRITER_FIELD_NUMBER: _ClassVar[int]
    SST_SETTINGS_FIELD_NUMBER: _ClassVar[int]
    VIPER_SETTINGS_FIELD_NUMBER: _ClassVar[int]
    NOVA_SETTINGS_FIELD_NUMBER: _ClassVar[int]
    storage_location: str
    persistent: bool
    compression: CompressionConfig
    source_storage: SourceStorageConfig
    access_pattern: AccessPattern
    data_density: DataDensity
    frequent_updates: bool
    expected_size_mb: int
    read_write_ratio: float
    preset: str
    enable_all_optimizations: bool
    parquet_writer: ParquetWriterSettings
    footer_cache: FooterCacheSettings
    hybrid_writer: HybridWriterSettings
    sst_settings: SstEngineSettings
    viper_settings: ViperEngineSettings
    nova_settings: NovaEngineSettings
    def __init__(self, storage_location: _Optional[str] = ..., persistent: bool = ..., compression: _Optional[_Union[CompressionConfig, _Mapping]] = ..., source_storage: _Optional[_Union[SourceStorageConfig, _Mapping]] = ..., access_pattern: _Optional[_Union[AccessPattern, str]] = ..., data_density: _Optional[_Union[DataDensity, str]] = ..., frequent_updates: bool = ..., expected_size_mb: _Optional[int] = ..., read_write_ratio: _Optional[float] = ..., preset: _Optional[str] = ..., enable_all_optimizations: bool = ..., parquet_writer: _Optional[_Union[ParquetWriterSettings, _Mapping]] = ..., footer_cache: _Optional[_Union[FooterCacheSettings, _Mapping]] = ..., hybrid_writer: _Optional[_Union[HybridWriterSettings, _Mapping]] = ..., sst_settings: _Optional[_Union[SstEngineSettings, _Mapping]] = ..., viper_settings: _Optional[_Union[ViperEngineSettings, _Mapping]] = ..., nova_settings: _Optional[_Union[NovaEngineSettings, _Mapping]] = ...) -> None: ...

class SourceStorageConfig(_message.Message):
    __slots__ = ("enabled", "require_id", "tiering", "text_compression", "binary_compression", "external", "chunking", "cache", "prefetch_on_search")
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    REQUIRE_ID_FIELD_NUMBER: _ClassVar[int]
    TIERING_FIELD_NUMBER: _ClassVar[int]
    TEXT_COMPRESSION_FIELD_NUMBER: _ClassVar[int]
    BINARY_COMPRESSION_FIELD_NUMBER: _ClassVar[int]
    EXTERNAL_FIELD_NUMBER: _ClassVar[int]
    CHUNKING_FIELD_NUMBER: _ClassVar[int]
    CACHE_FIELD_NUMBER: _ClassVar[int]
    PREFETCH_ON_SEARCH_FIELD_NUMBER: _ClassVar[int]
    enabled: bool
    require_id: bool
    tiering: StorageTieringPolicy
    text_compression: CompressionPolicy
    binary_compression: CompressionPolicy
    external: ExternalStorageConfig
    chunking: ChunkingConfig
    cache: CacheConfig
    prefetch_on_search: bool
    def __init__(self, enabled: bool = ..., require_id: bool = ..., tiering: _Optional[_Union[StorageTieringPolicy, _Mapping]] = ..., text_compression: _Optional[_Union[CompressionPolicy, _Mapping]] = ..., binary_compression: _Optional[_Union[CompressionPolicy, _Mapping]] = ..., external: _Optional[_Union[ExternalStorageConfig, _Mapping]] = ..., chunking: _Optional[_Union[ChunkingConfig, _Mapping]] = ..., cache: _Optional[_Union[CacheConfig, _Mapping]] = ..., prefetch_on_search: bool = ...) -> None: ...

class StorageTieringPolicy(_message.Message):
    __slots__ = ("inline_threshold", "block_threshold", "file_threshold", "access_policy", "enable_promotion", "enable_demotion", "content_overrides")
    class ContentOverridesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: TierOverride
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[TierOverride, _Mapping]] = ...) -> None: ...
    INLINE_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    BLOCK_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    FILE_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    ACCESS_POLICY_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PROMOTION_FIELD_NUMBER: _ClassVar[int]
    ENABLE_DEMOTION_FIELD_NUMBER: _ClassVar[int]
    CONTENT_OVERRIDES_FIELD_NUMBER: _ClassVar[int]
    inline_threshold: int
    block_threshold: int
    file_threshold: int
    access_policy: AccessBasedTiering
    enable_promotion: bool
    enable_demotion: bool
    content_overrides: _containers.MessageMap[str, TierOverride]
    def __init__(self, inline_threshold: _Optional[int] = ..., block_threshold: _Optional[int] = ..., file_threshold: _Optional[int] = ..., access_policy: _Optional[_Union[AccessBasedTiering, _Mapping]] = ..., enable_promotion: bool = ..., enable_demotion: bool = ..., content_overrides: _Optional[_Mapping[str, TierOverride]] = ...) -> None: ...

class AccessBasedTiering(_message.Message):
    __slots__ = ("access_threshold_promote", "age_threshold_demote", "hot_data_ratio", "track_access_patterns")
    ACCESS_THRESHOLD_PROMOTE_FIELD_NUMBER: _ClassVar[int]
    AGE_THRESHOLD_DEMOTE_FIELD_NUMBER: _ClassVar[int]
    HOT_DATA_RATIO_FIELD_NUMBER: _ClassVar[int]
    TRACK_ACCESS_PATTERNS_FIELD_NUMBER: _ClassVar[int]
    access_threshold_promote: int
    age_threshold_demote: int
    hot_data_ratio: float
    track_access_patterns: bool
    def __init__(self, access_threshold_promote: _Optional[int] = ..., age_threshold_demote: _Optional[int] = ..., hot_data_ratio: _Optional[float] = ..., track_access_patterns: bool = ...) -> None: ...

class TierOverride(_message.Message):
    __slots__ = ("force_tier", "disable_promotion", "disable_demotion")
    FORCE_TIER_FIELD_NUMBER: _ClassVar[int]
    DISABLE_PROMOTION_FIELD_NUMBER: _ClassVar[int]
    DISABLE_DEMOTION_FIELD_NUMBER: _ClassVar[int]
    force_tier: StorageTier
    disable_promotion: bool
    disable_demotion: bool
    def __init__(self, force_tier: _Optional[_Union[StorageTier, str]] = ..., disable_promotion: bool = ..., disable_demotion: bool = ...) -> None: ...

class CompressionPolicy(_message.Message):
    __slots__ = ("algorithm", "level", "min_size_to_compress", "adaptive_level")
    ALGORITHM_FIELD_NUMBER: _ClassVar[int]
    LEVEL_FIELD_NUMBER: _ClassVar[int]
    MIN_SIZE_TO_COMPRESS_FIELD_NUMBER: _ClassVar[int]
    ADAPTIVE_LEVEL_FIELD_NUMBER: _ClassVar[int]
    algorithm: str
    level: int
    min_size_to_compress: int
    adaptive_level: bool
    def __init__(self, algorithm: _Optional[str] = ..., level: _Optional[int] = ..., min_size_to_compress: _Optional[int] = ..., adaptive_level: bool = ...) -> None: ...

class ExternalStorageConfig(_message.Message):
    __slots__ = ("backend", "bucket", "prefix", "credentials", "signed_url_expiry", "enable_cdn")
    class CredentialsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    BACKEND_FIELD_NUMBER: _ClassVar[int]
    BUCKET_FIELD_NUMBER: _ClassVar[int]
    PREFIX_FIELD_NUMBER: _ClassVar[int]
    CREDENTIALS_FIELD_NUMBER: _ClassVar[int]
    SIGNED_URL_EXPIRY_FIELD_NUMBER: _ClassVar[int]
    ENABLE_CDN_FIELD_NUMBER: _ClassVar[int]
    backend: str
    bucket: str
    prefix: str
    credentials: _containers.ScalarMap[str, str]
    signed_url_expiry: int
    enable_cdn: bool
    def __init__(self, backend: _Optional[str] = ..., bucket: _Optional[str] = ..., prefix: _Optional[str] = ..., credentials: _Optional[_Mapping[str, str]] = ..., signed_url_expiry: _Optional[int] = ..., enable_cdn: bool = ...) -> None: ...

class ChunkingConfig(_message.Message):
    __slots__ = ("enabled", "strategy", "chunk_size", "overlap", "preserve_sentences", "include_context", "context_window")
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    STRATEGY_FIELD_NUMBER: _ClassVar[int]
    CHUNK_SIZE_FIELD_NUMBER: _ClassVar[int]
    OVERLAP_FIELD_NUMBER: _ClassVar[int]
    PRESERVE_SENTENCES_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_CONTEXT_FIELD_NUMBER: _ClassVar[int]
    CONTEXT_WINDOW_FIELD_NUMBER: _ClassVar[int]
    enabled: bool
    strategy: ChunkingStrategy
    chunk_size: int
    overlap: int
    preserve_sentences: bool
    include_context: bool
    context_window: int
    def __init__(self, enabled: bool = ..., strategy: _Optional[_Union[ChunkingStrategy, str]] = ..., chunk_size: _Optional[int] = ..., overlap: _Optional[int] = ..., preserve_sentences: bool = ..., include_context: bool = ..., context_window: _Optional[int] = ...) -> None: ...

class CacheConfig(_message.Message):
    __slots__ = ("enabled", "cache_size_mb", "ttl_seconds", "cache_hot_only", "eviction")
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    CACHE_SIZE_MB_FIELD_NUMBER: _ClassVar[int]
    TTL_SECONDS_FIELD_NUMBER: _ClassVar[int]
    CACHE_HOT_ONLY_FIELD_NUMBER: _ClassVar[int]
    EVICTION_FIELD_NUMBER: _ClassVar[int]
    enabled: bool
    cache_size_mb: int
    ttl_seconds: int
    cache_hot_only: bool
    eviction: CacheEvictionPolicy
    def __init__(self, enabled: bool = ..., cache_size_mb: _Optional[int] = ..., ttl_seconds: _Optional[int] = ..., cache_hot_only: bool = ..., eviction: _Optional[_Union[CacheEvictionPolicy, str]] = ...) -> None: ...

class ParquetWriterSettings(_message.Message):
    __slots__ = ("row_group_size", "page_size", "enable_bloom_filters", "bloom_filter_fpp", "bloom_filter_columns", "enable_column_statistics", "enable_page_index", "enable_column_index", "enable_offset_index", "page_index_granularity", "enable_dictionary", "dictionary_threshold", "enable_delta_encoding", "enable_byte_stream_split", "enable_pq_sorting", "pq_sorting_segments", "pq_sorting_codebook_size", "enable_native_metadata", "metadata_inference_samples", "write_batch_size", "id_less_storage")
    ROW_GROUP_SIZE_FIELD_NUMBER: _ClassVar[int]
    PAGE_SIZE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_BLOOM_FILTERS_FIELD_NUMBER: _ClassVar[int]
    BLOOM_FILTER_FPP_FIELD_NUMBER: _ClassVar[int]
    BLOOM_FILTER_COLUMNS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_COLUMN_STATISTICS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PAGE_INDEX_FIELD_NUMBER: _ClassVar[int]
    ENABLE_COLUMN_INDEX_FIELD_NUMBER: _ClassVar[int]
    ENABLE_OFFSET_INDEX_FIELD_NUMBER: _ClassVar[int]
    PAGE_INDEX_GRANULARITY_FIELD_NUMBER: _ClassVar[int]
    ENABLE_DICTIONARY_FIELD_NUMBER: _ClassVar[int]
    DICTIONARY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    ENABLE_DELTA_ENCODING_FIELD_NUMBER: _ClassVar[int]
    ENABLE_BYTE_STREAM_SPLIT_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PQ_SORTING_FIELD_NUMBER: _ClassVar[int]
    PQ_SORTING_SEGMENTS_FIELD_NUMBER: _ClassVar[int]
    PQ_SORTING_CODEBOOK_SIZE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_NATIVE_METADATA_FIELD_NUMBER: _ClassVar[int]
    METADATA_INFERENCE_SAMPLES_FIELD_NUMBER: _ClassVar[int]
    WRITE_BATCH_SIZE_FIELD_NUMBER: _ClassVar[int]
    ID_LESS_STORAGE_FIELD_NUMBER: _ClassVar[int]
    row_group_size: int
    page_size: int
    enable_bloom_filters: bool
    bloom_filter_fpp: float
    bloom_filter_columns: _containers.RepeatedScalarFieldContainer[str]
    enable_column_statistics: bool
    enable_page_index: bool
    enable_column_index: bool
    enable_offset_index: bool
    page_index_granularity: int
    enable_dictionary: bool
    dictionary_threshold: float
    enable_delta_encoding: bool
    enable_byte_stream_split: bool
    enable_pq_sorting: bool
    pq_sorting_segments: int
    pq_sorting_codebook_size: int
    enable_native_metadata: bool
    metadata_inference_samples: int
    write_batch_size: int
    id_less_storage: bool
    def __init__(self, row_group_size: _Optional[int] = ..., page_size: _Optional[int] = ..., enable_bloom_filters: bool = ..., bloom_filter_fpp: _Optional[float] = ..., bloom_filter_columns: _Optional[_Iterable[str]] = ..., enable_column_statistics: bool = ..., enable_page_index: bool = ..., enable_column_index: bool = ..., enable_offset_index: bool = ..., page_index_granularity: _Optional[int] = ..., enable_dictionary: bool = ..., dictionary_threshold: _Optional[float] = ..., enable_delta_encoding: bool = ..., enable_byte_stream_split: bool = ..., enable_pq_sorting: bool = ..., pq_sorting_segments: _Optional[int] = ..., pq_sorting_codebook_size: _Optional[int] = ..., enable_native_metadata: bool = ..., metadata_inference_samples: _Optional[int] = ..., write_batch_size: _Optional[int] = ..., id_less_storage: bool = ...) -> None: ...

class FooterCacheSettings(_message.Message):
    __slots__ = ("enable", "max_entries", "ttl_seconds", "time_to_idle_seconds", "enable_persistence", "persistence_path", "enable_prefetch", "prefetch_threshold", "warming_interval_seconds", "enable_compression", "compression_level")
    ENABLE_FIELD_NUMBER: _ClassVar[int]
    MAX_ENTRIES_FIELD_NUMBER: _ClassVar[int]
    TTL_SECONDS_FIELD_NUMBER: _ClassVar[int]
    TIME_TO_IDLE_SECONDS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PERSISTENCE_FIELD_NUMBER: _ClassVar[int]
    PERSISTENCE_PATH_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PREFETCH_FIELD_NUMBER: _ClassVar[int]
    PREFETCH_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    WARMING_INTERVAL_SECONDS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_COMPRESSION_FIELD_NUMBER: _ClassVar[int]
    COMPRESSION_LEVEL_FIELD_NUMBER: _ClassVar[int]
    enable: bool
    max_entries: int
    ttl_seconds: int
    time_to_idle_seconds: int
    enable_persistence: bool
    persistence_path: str
    enable_prefetch: bool
    prefetch_threshold: int
    warming_interval_seconds: int
    enable_compression: bool
    compression_level: int
    def __init__(self, enable: bool = ..., max_entries: _Optional[int] = ..., ttl_seconds: _Optional[int] = ..., time_to_idle_seconds: _Optional[int] = ..., enable_persistence: bool = ..., persistence_path: _Optional[str] = ..., enable_prefetch: bool = ..., prefetch_threshold: _Optional[int] = ..., warming_interval_seconds: _Optional[int] = ..., enable_compression: bool = ..., compression_level: _Optional[int] = ...) -> None: ...

class HybridWriterSettings(_message.Message):
    __slots__ = ("enable", "initial_mode", "enable_auto_switch", "mode_switch_threshold", "pattern_window_size", "streaming_threshold", "batch_threshold", "max_buffer_size", "buffer_time_limit_seconds", "enable_concurrent_writes", "max_concurrent_writers", "optimize_row_group_size", "min_row_group_size", "max_row_group_size")
    ENABLE_FIELD_NUMBER: _ClassVar[int]
    INITIAL_MODE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_AUTO_SWITCH_FIELD_NUMBER: _ClassVar[int]
    MODE_SWITCH_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    PATTERN_WINDOW_SIZE_FIELD_NUMBER: _ClassVar[int]
    STREAMING_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    BATCH_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    MAX_BUFFER_SIZE_FIELD_NUMBER: _ClassVar[int]
    BUFFER_TIME_LIMIT_SECONDS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_CONCURRENT_WRITES_FIELD_NUMBER: _ClassVar[int]
    MAX_CONCURRENT_WRITERS_FIELD_NUMBER: _ClassVar[int]
    OPTIMIZE_ROW_GROUP_SIZE_FIELD_NUMBER: _ClassVar[int]
    MIN_ROW_GROUP_SIZE_FIELD_NUMBER: _ClassVar[int]
    MAX_ROW_GROUP_SIZE_FIELD_NUMBER: _ClassVar[int]
    enable: bool
    initial_mode: str
    enable_auto_switch: bool
    mode_switch_threshold: int
    pattern_window_size: int
    streaming_threshold: float
    batch_threshold: int
    max_buffer_size: int
    buffer_time_limit_seconds: int
    enable_concurrent_writes: bool
    max_concurrent_writers: int
    optimize_row_group_size: bool
    min_row_group_size: int
    max_row_group_size: int
    def __init__(self, enable: bool = ..., initial_mode: _Optional[str] = ..., enable_auto_switch: bool = ..., mode_switch_threshold: _Optional[int] = ..., pattern_window_size: _Optional[int] = ..., streaming_threshold: _Optional[float] = ..., batch_threshold: _Optional[int] = ..., max_buffer_size: _Optional[int] = ..., buffer_time_limit_seconds: _Optional[int] = ..., enable_concurrent_writes: bool = ..., max_concurrent_writers: _Optional[int] = ..., optimize_row_group_size: bool = ..., min_row_group_size: _Optional[int] = ..., max_row_group_size: _Optional[int] = ...) -> None: ...

class SstEngineSettings(_message.Message):
    __slots__ = ("enable_bloom_filters", "bloom_filter_fpp", "compression", "compression_level", "write_buffer_size", "max_write_buffers", "block_size_kb", "dynamic_block_sizing")
    ENABLE_BLOOM_FILTERS_FIELD_NUMBER: _ClassVar[int]
    BLOOM_FILTER_FPP_FIELD_NUMBER: _ClassVar[int]
    COMPRESSION_FIELD_NUMBER: _ClassVar[int]
    COMPRESSION_LEVEL_FIELD_NUMBER: _ClassVar[int]
    WRITE_BUFFER_SIZE_FIELD_NUMBER: _ClassVar[int]
    MAX_WRITE_BUFFERS_FIELD_NUMBER: _ClassVar[int]
    BLOCK_SIZE_KB_FIELD_NUMBER: _ClassVar[int]
    DYNAMIC_BLOCK_SIZING_FIELD_NUMBER: _ClassVar[int]
    enable_bloom_filters: bool
    bloom_filter_fpp: float
    compression: CompressionAlgorithm
    compression_level: int
    write_buffer_size: int
    max_write_buffers: int
    block_size_kb: int
    dynamic_block_sizing: bool
    def __init__(self, enable_bloom_filters: bool = ..., bloom_filter_fpp: _Optional[float] = ..., compression: _Optional[_Union[CompressionAlgorithm, str]] = ..., compression_level: _Optional[int] = ..., write_buffer_size: _Optional[int] = ..., max_write_buffers: _Optional[int] = ..., block_size_kb: _Optional[int] = ..., dynamic_block_sizing: bool = ...) -> None: ...

class ViperEngineSettings(_message.Message):
    __slots__ = ("inherit_global_settings", "enable_columnar_compression", "enable_vector_quantization", "vector_chunk_size", "enable_lazy_loading")
    INHERIT_GLOBAL_SETTINGS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_COLUMNAR_COMPRESSION_FIELD_NUMBER: _ClassVar[int]
    ENABLE_VECTOR_QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    VECTOR_CHUNK_SIZE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_LAZY_LOADING_FIELD_NUMBER: _ClassVar[int]
    inherit_global_settings: bool
    enable_columnar_compression: bool
    enable_vector_quantization: bool
    vector_chunk_size: int
    enable_lazy_loading: bool
    def __init__(self, inherit_global_settings: bool = ..., enable_columnar_compression: bool = ..., enable_vector_quantization: bool = ..., vector_chunk_size: _Optional[int] = ..., enable_lazy_loading: bool = ...) -> None: ...

class NovaEngineSettings(_message.Message):
    __slots__ = ("inherit_global_settings", "enable_real_time_mode", "streaming_buffer_size", "prefer_low_latency")
    INHERIT_GLOBAL_SETTINGS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_REAL_TIME_MODE_FIELD_NUMBER: _ClassVar[int]
    STREAMING_BUFFER_SIZE_FIELD_NUMBER: _ClassVar[int]
    PREFER_LOW_LATENCY_FIELD_NUMBER: _ClassVar[int]
    inherit_global_settings: bool
    enable_real_time_mode: bool
    streaming_buffer_size: int
    prefer_low_latency: bool
    def __init__(self, inherit_global_settings: bool = ..., enable_real_time_mode: bool = ..., streaming_buffer_size: _Optional[int] = ..., prefer_low_latency: bool = ...) -> None: ...

class QuantizationConfig(_message.Message):
    __slots__ = ("enabled", "strategy", "custom_levels", "enable_progressive_search", "binary_filter_selectivity", "int8_ranking_selectivity", "pq_ranking_selectivity", "training_sample_size", "quality_threshold", "enable_adaptive_training", "optimize_for_storage", "optimize_for_memory", "enable_simd_acceleration", "enable_binary", "enable_int8", "enable_pq", "pq_segments", "pq_bits", "pq_codebooks", "binary_threshold", "int8_threshold", "pq_threshold")
    class Strategy(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        SMART_DEFAULTS: _ClassVar[QuantizationConfig.Strategy]
        CUSTOM_LEVELS: _ClassVar[QuantizationConfig.Strategy]
        MINIMAL: _ClassVar[QuantizationConfig.Strategy]
        AGGRESSIVE: _ClassVar[QuantizationConfig.Strategy]
    SMART_DEFAULTS: QuantizationConfig.Strategy
    CUSTOM_LEVELS: QuantizationConfig.Strategy
    MINIMAL: QuantizationConfig.Strategy
    AGGRESSIVE: QuantizationConfig.Strategy
    ENABLED_FIELD_NUMBER: _ClassVar[int]
    STRATEGY_FIELD_NUMBER: _ClassVar[int]
    CUSTOM_LEVELS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PROGRESSIVE_SEARCH_FIELD_NUMBER: _ClassVar[int]
    BINARY_FILTER_SELECTIVITY_FIELD_NUMBER: _ClassVar[int]
    INT8_RANKING_SELECTIVITY_FIELD_NUMBER: _ClassVar[int]
    PQ_RANKING_SELECTIVITY_FIELD_NUMBER: _ClassVar[int]
    TRAINING_SAMPLE_SIZE_FIELD_NUMBER: _ClassVar[int]
    QUALITY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    ENABLE_ADAPTIVE_TRAINING_FIELD_NUMBER: _ClassVar[int]
    OPTIMIZE_FOR_STORAGE_FIELD_NUMBER: _ClassVar[int]
    OPTIMIZE_FOR_MEMORY_FIELD_NUMBER: _ClassVar[int]
    ENABLE_SIMD_ACCELERATION_FIELD_NUMBER: _ClassVar[int]
    ENABLE_BINARY_FIELD_NUMBER: _ClassVar[int]
    ENABLE_INT8_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PQ_FIELD_NUMBER: _ClassVar[int]
    PQ_SEGMENTS_FIELD_NUMBER: _ClassVar[int]
    PQ_BITS_FIELD_NUMBER: _ClassVar[int]
    PQ_CODEBOOKS_FIELD_NUMBER: _ClassVar[int]
    BINARY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    INT8_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    PQ_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    enabled: bool
    strategy: QuantizationConfig.Strategy
    custom_levels: _containers.RepeatedCompositeFieldContainer[QuantizationLevel]
    enable_progressive_search: bool
    binary_filter_selectivity: float
    int8_ranking_selectivity: float
    pq_ranking_selectivity: float
    training_sample_size: int
    quality_threshold: float
    enable_adaptive_training: bool
    optimize_for_storage: bool
    optimize_for_memory: bool
    enable_simd_acceleration: bool
    enable_binary: bool
    enable_int8: bool
    enable_pq: bool
    pq_segments: int
    pq_bits: int
    pq_codebooks: _containers.RepeatedScalarFieldContainer[bytes]
    binary_threshold: float
    int8_threshold: float
    pq_threshold: float
    def __init__(self, enabled: bool = ..., strategy: _Optional[_Union[QuantizationConfig.Strategy, str]] = ..., custom_levels: _Optional[_Iterable[_Union[QuantizationLevel, _Mapping]]] = ..., enable_progressive_search: bool = ..., binary_filter_selectivity: _Optional[float] = ..., int8_ranking_selectivity: _Optional[float] = ..., pq_ranking_selectivity: _Optional[float] = ..., training_sample_size: _Optional[int] = ..., quality_threshold: _Optional[float] = ..., enable_adaptive_training: bool = ..., optimize_for_storage: bool = ..., optimize_for_memory: bool = ..., enable_simd_acceleration: bool = ..., enable_binary: bool = ..., enable_int8: bool = ..., enable_pq: bool = ..., pq_segments: _Optional[int] = ..., pq_bits: _Optional[int] = ..., pq_codebooks: _Optional[_Iterable[bytes]] = ..., binary_threshold: _Optional[float] = ..., int8_threshold: _Optional[float] = ..., pq_threshold: _Optional[float] = ...) -> None: ...

class QuantizationLevel(_message.Message):
    __slots__ = ("level_id", "type", "bits", "num_subvectors", "adaptive_subvectors", "scale", "offset", "clamp_values", "threshold", "sign_based", "enable_in_storage", "enable_in_index", "search_priority", "min_recall", "enable_validation")
    class QuantizationType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
        __slots__ = ()
        BINARY: _ClassVar[QuantizationLevel.QuantizationType]
        SCALAR: _ClassVar[QuantizationLevel.QuantizationType]
        PRODUCT: _ClassVar[QuantizationLevel.QuantizationType]
        UNIFORM: _ClassVar[QuantizationLevel.QuantizationType]
        NONE: _ClassVar[QuantizationLevel.QuantizationType]
    BINARY: QuantizationLevel.QuantizationType
    SCALAR: QuantizationLevel.QuantizationType
    PRODUCT: QuantizationLevel.QuantizationType
    UNIFORM: QuantizationLevel.QuantizationType
    NONE: QuantizationLevel.QuantizationType
    LEVEL_ID_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    BITS_FIELD_NUMBER: _ClassVar[int]
    NUM_SUBVECTORS_FIELD_NUMBER: _ClassVar[int]
    ADAPTIVE_SUBVECTORS_FIELD_NUMBER: _ClassVar[int]
    SCALE_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    CLAMP_VALUES_FIELD_NUMBER: _ClassVar[int]
    THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    SIGN_BASED_FIELD_NUMBER: _ClassVar[int]
    ENABLE_IN_STORAGE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_IN_INDEX_FIELD_NUMBER: _ClassVar[int]
    SEARCH_PRIORITY_FIELD_NUMBER: _ClassVar[int]
    MIN_RECALL_FIELD_NUMBER: _ClassVar[int]
    ENABLE_VALIDATION_FIELD_NUMBER: _ClassVar[int]
    level_id: str
    type: QuantizationLevel.QuantizationType
    bits: int
    num_subvectors: int
    adaptive_subvectors: bool
    scale: float
    offset: float
    clamp_values: bool
    threshold: float
    sign_based: bool
    enable_in_storage: bool
    enable_in_index: bool
    search_priority: int
    min_recall: float
    enable_validation: bool
    def __init__(self, level_id: _Optional[str] = ..., type: _Optional[_Union[QuantizationLevel.QuantizationType, str]] = ..., bits: _Optional[int] = ..., num_subvectors: _Optional[int] = ..., adaptive_subvectors: bool = ..., scale: _Optional[float] = ..., offset: _Optional[float] = ..., clamp_values: bool = ..., threshold: _Optional[float] = ..., sign_based: bool = ..., enable_in_storage: bool = ..., enable_in_index: bool = ..., search_priority: _Optional[int] = ..., min_recall: _Optional[float] = ..., enable_validation: bool = ...) -> None: ...

class FilterableColumnSpec(_message.Message):
    __slots__ = ("name", "data_type", "indexed", "supports_range", "estimated_cardinality", "encoding_hint")
    NAME_FIELD_NUMBER: _ClassVar[int]
    DATA_TYPE_FIELD_NUMBER: _ClassVar[int]
    INDEXED_FIELD_NUMBER: _ClassVar[int]
    SUPPORTS_RANGE_FIELD_NUMBER: _ClassVar[int]
    ESTIMATED_CARDINALITY_FIELD_NUMBER: _ClassVar[int]
    ENCODING_HINT_FIELD_NUMBER: _ClassVar[int]
    name: str
    data_type: FilterableDataType
    indexed: bool
    supports_range: bool
    estimated_cardinality: int
    encoding_hint: ColumnEncoding
    def __init__(self, name: _Optional[str] = ..., data_type: _Optional[_Union[FilterableDataType, str]] = ..., indexed: bool = ..., supports_range: bool = ..., estimated_cardinality: _Optional[int] = ..., encoding_hint: _Optional[_Union[ColumnEncoding, str]] = ...) -> None: ...

class Collection(_message.Message):
    __slots__ = ("id", "config", "stats", "created_at", "updated_at", "storage_assignment")
    ID_FIELD_NUMBER: _ClassVar[int]
    CONFIG_FIELD_NUMBER: _ClassVar[int]
    STATS_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    UPDATED_AT_FIELD_NUMBER: _ClassVar[int]
    STORAGE_ASSIGNMENT_FIELD_NUMBER: _ClassVar[int]
    id: str
    config: CollectionConfig
    stats: CollectionStats
    created_at: int
    updated_at: int
    storage_assignment: StorageAssignment
    def __init__(self, id: _Optional[str] = ..., config: _Optional[_Union[CollectionConfig, _Mapping]] = ..., stats: _Optional[_Union[CollectionStats, _Mapping]] = ..., created_at: _Optional[int] = ..., updated_at: _Optional[int] = ..., storage_assignment: _Optional[_Union[StorageAssignment, _Mapping]] = ...) -> None: ...

class StorageAssignment(_message.Message):
    __slots__ = ("base_location", "assigned_at")
    BASE_LOCATION_FIELD_NUMBER: _ClassVar[int]
    ASSIGNED_AT_FIELD_NUMBER: _ClassVar[int]
    base_location: str
    assigned_at: int
    def __init__(self, base_location: _Optional[str] = ..., assigned_at: _Optional[int] = ...) -> None: ...

class CollectionStats(_message.Message):
    __slots__ = ("vector_count", "index_size_bytes", "data_size_bytes")
    VECTOR_COUNT_FIELD_NUMBER: _ClassVar[int]
    INDEX_SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    DATA_SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    vector_count: int
    index_size_bytes: int
    data_size_bytes: int
    def __init__(self, vector_count: _Optional[int] = ..., index_size_bytes: _Optional[int] = ..., data_size_bytes: _Optional[int] = ...) -> None: ...

class CollectionRequest(_message.Message):
    __slots__ = ("operation", "collection_id", "collection_config", "query_params", "options", "migration_config")
    class QueryParamsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    class OptionsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: bool
        def __init__(self, key: _Optional[str] = ..., value: bool = ...) -> None: ...
    class MigrationConfigEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_CONFIG_FIELD_NUMBER: _ClassVar[int]
    QUERY_PARAMS_FIELD_NUMBER: _ClassVar[int]
    OPTIONS_FIELD_NUMBER: _ClassVar[int]
    MIGRATION_CONFIG_FIELD_NUMBER: _ClassVar[int]
    operation: CollectionOperation
    collection_id: str
    collection_config: CollectionConfig
    query_params: _containers.ScalarMap[str, str]
    options: _containers.ScalarMap[str, bool]
    migration_config: _containers.ScalarMap[str, str]
    def __init__(self, operation: _Optional[_Union[CollectionOperation, str]] = ..., collection_id: _Optional[str] = ..., collection_config: _Optional[_Union[CollectionConfig, _Mapping]] = ..., query_params: _Optional[_Mapping[str, str]] = ..., options: _Optional[_Mapping[str, bool]] = ..., migration_config: _Optional[_Mapping[str, str]] = ...) -> None: ...

class CollectionResponse(_message.Message):
    __slots__ = ("success", "operation", "collection", "collections", "affected_count", "total_count", "metadata", "error_message", "error_code", "processing_time_us")
    class MetadataEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    COLLECTIONS_FIELD_NUMBER: _ClassVar[int]
    AFFECTED_COUNT_FIELD_NUMBER: _ClassVar[int]
    TOTAL_COUNT_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    ERROR_MESSAGE_FIELD_NUMBER: _ClassVar[int]
    ERROR_CODE_FIELD_NUMBER: _ClassVar[int]
    PROCESSING_TIME_US_FIELD_NUMBER: _ClassVar[int]
    success: bool
    operation: CollectionOperation
    collection: Collection
    collections: _containers.RepeatedCompositeFieldContainer[Collection]
    affected_count: int
    total_count: int
    metadata: _containers.ScalarMap[str, str]
    error_message: str
    error_code: str
    processing_time_us: int
    def __init__(self, success: bool = ..., operation: _Optional[_Union[CollectionOperation, str]] = ..., collection: _Optional[_Union[Collection, _Mapping]] = ..., collections: _Optional[_Iterable[_Union[Collection, _Mapping]]] = ..., affected_count: _Optional[int] = ..., total_count: _Optional[int] = ..., metadata: _Optional[_Mapping[str, str]] = ..., error_message: _Optional[str] = ..., error_code: _Optional[str] = ..., processing_time_us: _Optional[int] = ...) -> None: ...

class VectorBatchRequest(_message.Message):
    __slots__ = ("collection_id", "vectors", "batch_timeout_ms", "request_id")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    VECTORS_FIELD_NUMBER: _ClassVar[int]
    BATCH_TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    REQUEST_ID_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    vectors: _containers.RepeatedCompositeFieldContainer[VectorRecord]
    batch_timeout_ms: int
    request_id: str
    def __init__(self, collection_id: _Optional[str] = ..., vectors: _Optional[_Iterable[_Union[VectorRecord, _Mapping]]] = ..., batch_timeout_ms: _Optional[int] = ..., request_id: _Optional[str] = ...) -> None: ...

class VectorSearchRequest(_message.Message):
    __slots__ = ("collection_id", "queries", "top_k", "distance_metric_override", "search_params", "include_fields", "search_optimization")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    QUERIES_FIELD_NUMBER: _ClassVar[int]
    TOP_K_FIELD_NUMBER: _ClassVar[int]
    DISTANCE_METRIC_OVERRIDE_FIELD_NUMBER: _ClassVar[int]
    SEARCH_PARAMS_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_FIELDS_FIELD_NUMBER: _ClassVar[int]
    SEARCH_OPTIMIZATION_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    queries: _containers.RepeatedCompositeFieldContainer[SearchQuery]
    top_k: int
    distance_metric_override: DistanceMetric
    search_params: SearchParameters
    include_fields: IncludeFields
    search_optimization: SearchParams
    def __init__(self, collection_id: _Optional[str] = ..., queries: _Optional[_Iterable[_Union[SearchQuery, _Mapping]]] = ..., top_k: _Optional[int] = ..., distance_metric_override: _Optional[_Union[DistanceMetric, str]] = ..., search_params: _Optional[_Union[SearchParameters, _Mapping]] = ..., include_fields: _Optional[_Union[IncludeFields, _Mapping]] = ..., search_optimization: _Optional[_Union[SearchParams, _Mapping]] = ...) -> None: ...

class VectorGetRequest(_message.Message):
    __slots__ = ("collection_id", "vector_id", "include_fields")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    VECTOR_ID_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_FIELDS_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    vector_id: str
    include_fields: IncludeFields
    def __init__(self, collection_id: _Optional[str] = ..., vector_id: _Optional[str] = ..., include_fields: _Optional[_Union[IncludeFields, _Mapping]] = ...) -> None: ...

class SearchParameters(_message.Message):
    __slots__ = ("ef_search", "max_connections", "n_probe", "enable_reranking", "batch_size", "timeout_ms", "accuracy_threshold", "enable_parallel_search", "thread_count")
    EF_SEARCH_FIELD_NUMBER: _ClassVar[int]
    MAX_CONNECTIONS_FIELD_NUMBER: _ClassVar[int]
    N_PROBE_FIELD_NUMBER: _ClassVar[int]
    ENABLE_RERANKING_FIELD_NUMBER: _ClassVar[int]
    BATCH_SIZE_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    ACCURACY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PARALLEL_SEARCH_FIELD_NUMBER: _ClassVar[int]
    THREAD_COUNT_FIELD_NUMBER: _ClassVar[int]
    ef_search: int
    max_connections: int
    n_probe: int
    enable_reranking: bool
    batch_size: int
    timeout_ms: int
    accuracy_threshold: float
    enable_parallel_search: bool
    thread_count: int
    def __init__(self, ef_search: _Optional[int] = ..., max_connections: _Optional[int] = ..., n_probe: _Optional[int] = ..., enable_reranking: bool = ..., batch_size: _Optional[int] = ..., timeout_ms: _Optional[int] = ..., accuracy_threshold: _Optional[float] = ..., enable_parallel_search: bool = ..., thread_count: _Optional[int] = ...) -> None: ...

class SearchQuery(_message.Message):
    __slots__ = ("vector", "id", "metadata_filter")
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    ID_FIELD_NUMBER: _ClassVar[int]
    METADATA_FILTER_FIELD_NUMBER: _ClassVar[int]
    vector: _containers.RepeatedScalarFieldContainer[float]
    id: str
    metadata_filter: MetadataFilter
    def __init__(self, vector: _Optional[_Iterable[float]] = ..., id: _Optional[str] = ..., metadata_filter: _Optional[_Union[MetadataFilter, _Mapping]] = ...) -> None: ...

class MetadataFilter(_message.Message):
    __slots__ = ("conditions", "operator")
    CONDITIONS_FIELD_NUMBER: _ClassVar[int]
    OPERATOR_FIELD_NUMBER: _ClassVar[int]
    conditions: _containers.RepeatedCompositeFieldContainer[FilterCondition]
    operator: FilterOperator
    def __init__(self, conditions: _Optional[_Iterable[_Union[FilterCondition, _Mapping]]] = ..., operator: _Optional[_Union[FilterOperator, str]] = ...) -> None: ...

class FilterCondition(_message.Message):
    __slots__ = ("field_name", "operation", "value")
    FIELD_NAME_FIELD_NUMBER: _ClassVar[int]
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    field_name: str
    operation: FilterOperation
    value: MetadataValue
    def __init__(self, field_name: _Optional[str] = ..., operation: _Optional[_Union[FilterOperation, str]] = ..., value: _Optional[_Union[MetadataValue, _Mapping]] = ...) -> None: ...

class SourceRetrievalOptions(_message.Message):
    __slots__ = ("expand_chunks", "max_chunk_expansion", "source_fields", "resolve_external", "max_source_size", "tier_preference", "include_chunk_context", "include_processing_info")
    EXPAND_CHUNKS_FIELD_NUMBER: _ClassVar[int]
    MAX_CHUNK_EXPANSION_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELDS_FIELD_NUMBER: _ClassVar[int]
    RESOLVE_EXTERNAL_FIELD_NUMBER: _ClassVar[int]
    MAX_SOURCE_SIZE_FIELD_NUMBER: _ClassVar[int]
    TIER_PREFERENCE_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_CHUNK_CONTEXT_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_PROCESSING_INFO_FIELD_NUMBER: _ClassVar[int]
    expand_chunks: bool
    max_chunk_expansion: int
    source_fields: _containers.RepeatedScalarFieldContainer[str]
    resolve_external: bool
    max_source_size: int
    tier_preference: str
    include_chunk_context: bool
    include_processing_info: bool
    def __init__(self, expand_chunks: bool = ..., max_chunk_expansion: _Optional[int] = ..., source_fields: _Optional[_Iterable[str]] = ..., resolve_external: bool = ..., max_source_size: _Optional[int] = ..., tier_preference: _Optional[str] = ..., include_chunk_context: bool = ..., include_processing_info: bool = ...) -> None: ...

class IncludeFields(_message.Message):
    __slots__ = ("vector", "metadata", "score", "rank", "source", "source_options")
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    RANK_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    SOURCE_OPTIONS_FIELD_NUMBER: _ClassVar[int]
    vector: bool
    metadata: bool
    score: bool
    rank: bool
    source: bool
    source_options: SourceRetrievalOptions
    def __init__(self, vector: bool = ..., metadata: bool = ..., score: bool = ..., rank: bool = ..., source: bool = ..., source_options: _Optional[_Union[SourceRetrievalOptions, _Mapping]] = ...) -> None: ...

class BinaryQuantizationParams(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class ScalarQuantizationParams(_message.Message):
    __slots__ = ("bits",)
    BITS_FIELD_NUMBER: _ClassVar[int]
    bits: int
    def __init__(self, bits: _Optional[int] = ...) -> None: ...

class ProductQuantizationParams(_message.Message):
    __slots__ = ("num_subvectors", "bits_per_code")
    NUM_SUBVECTORS_FIELD_NUMBER: _ClassVar[int]
    BITS_PER_CODE_FIELD_NUMBER: _ClassVar[int]
    num_subvectors: int
    bits_per_code: int
    def __init__(self, num_subvectors: _Optional[int] = ..., bits_per_code: _Optional[int] = ...) -> None: ...

class UniformQuantizationParams(_message.Message):
    __slots__ = ("scale", "offset")
    SCALE_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    scale: float
    offset: float
    def __init__(self, scale: _Optional[float] = ..., offset: _Optional[float] = ...) -> None: ...

class SearchParams(_message.Message):
    __slots__ = ("top_k", "filters", "accuracy_threshold", "include_expired", "timeout_ms", "enable_two_stage", "no_quantization", "binary", "scalar", "product", "uniform", "enable_clustering_hint", "enable_metadata_filtering_hint", "custom_hints")
    class FiltersEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _struct_pb2.Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ...) -> None: ...
    class CustomHintsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _struct_pb2.Value
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_struct_pb2.Value, _Mapping]] = ...) -> None: ...
    TOP_K_FIELD_NUMBER: _ClassVar[int]
    FILTERS_FIELD_NUMBER: _ClassVar[int]
    ACCURACY_THRESHOLD_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_EXPIRED_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    ENABLE_TWO_STAGE_FIELD_NUMBER: _ClassVar[int]
    NO_QUANTIZATION_FIELD_NUMBER: _ClassVar[int]
    BINARY_FIELD_NUMBER: _ClassVar[int]
    SCALAR_FIELD_NUMBER: _ClassVar[int]
    PRODUCT_FIELD_NUMBER: _ClassVar[int]
    UNIFORM_FIELD_NUMBER: _ClassVar[int]
    ENABLE_CLUSTERING_HINT_FIELD_NUMBER: _ClassVar[int]
    ENABLE_METADATA_FILTERING_HINT_FIELD_NUMBER: _ClassVar[int]
    CUSTOM_HINTS_FIELD_NUMBER: _ClassVar[int]
    top_k: int
    filters: _containers.MessageMap[str, _struct_pb2.Value]
    accuracy_threshold: float
    include_expired: bool
    timeout_ms: int
    enable_two_stage: bool
    no_quantization: bool
    binary: BinaryQuantizationParams
    scalar: ScalarQuantizationParams
    product: ProductQuantizationParams
    uniform: UniformQuantizationParams
    enable_clustering_hint: bool
    enable_metadata_filtering_hint: bool
    custom_hints: _containers.MessageMap[str, _struct_pb2.Value]
    def __init__(self, top_k: _Optional[int] = ..., filters: _Optional[_Mapping[str, _struct_pb2.Value]] = ..., accuracy_threshold: _Optional[float] = ..., include_expired: bool = ..., timeout_ms: _Optional[int] = ..., enable_two_stage: bool = ..., no_quantization: bool = ..., binary: _Optional[_Union[BinaryQuantizationParams, _Mapping]] = ..., scalar: _Optional[_Union[ScalarQuantizationParams, _Mapping]] = ..., product: _Optional[_Union[ProductQuantizationParams, _Mapping]] = ..., uniform: _Optional[_Union[UniformQuantizationParams, _Mapping]] = ..., enable_clustering_hint: bool = ..., enable_metadata_filtering_hint: bool = ..., custom_hints: _Optional[_Mapping[str, _struct_pb2.Value]] = ...) -> None: ...

class VectorOperationResponse(_message.Message):
    __slots__ = ("success", "operation", "metrics", "results", "vector_ids", "error_message", "error_code", "result_info")
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    OPERATION_FIELD_NUMBER: _ClassVar[int]
    METRICS_FIELD_NUMBER: _ClassVar[int]
    RESULTS_FIELD_NUMBER: _ClassVar[int]
    VECTOR_IDS_FIELD_NUMBER: _ClassVar[int]
    ERROR_MESSAGE_FIELD_NUMBER: _ClassVar[int]
    ERROR_CODE_FIELD_NUMBER: _ClassVar[int]
    RESULT_INFO_FIELD_NUMBER: _ClassVar[int]
    success: bool
    operation: VectorOperation
    metrics: OperationMetrics
    results: SearchResult
    vector_ids: _containers.RepeatedScalarFieldContainer[str]
    error_message: str
    error_code: str
    result_info: ResultMetadata
    def __init__(self, success: bool = ..., operation: _Optional[_Union[VectorOperation, str]] = ..., metrics: _Optional[_Union[OperationMetrics, _Mapping]] = ..., results: _Optional[_Union[SearchResult, _Mapping]] = ..., vector_ids: _Optional[_Iterable[str]] = ..., error_message: _Optional[str] = ..., error_code: _Optional[str] = ..., result_info: _Optional[_Union[ResultMetadata, _Mapping]] = ...) -> None: ...

class SearchResult(_message.Message):
    __slots__ = ("results", "total_found", "collection_id")
    RESULTS_FIELD_NUMBER: _ClassVar[int]
    TOTAL_FOUND_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    results: _containers.RepeatedCompositeFieldContainer[SearchVectorRecord]
    total_found: int
    collection_id: str
    def __init__(self, results: _Optional[_Iterable[_Union[SearchVectorRecord, _Mapping]]] = ..., total_found: _Optional[int] = ..., collection_id: _Optional[str] = ...) -> None: ...

class ResultMetadata(_message.Message):
    __slots__ = ("result_count", "estimated_size_bytes", "processing_time_us", "algorithm_used")
    RESULT_COUNT_FIELD_NUMBER: _ClassVar[int]
    ESTIMATED_SIZE_BYTES_FIELD_NUMBER: _ClassVar[int]
    PROCESSING_TIME_US_FIELD_NUMBER: _ClassVar[int]
    ALGORITHM_USED_FIELD_NUMBER: _ClassVar[int]
    result_count: int
    estimated_size_bytes: int
    processing_time_us: int
    algorithm_used: str
    def __init__(self, result_count: _Optional[int] = ..., estimated_size_bytes: _Optional[int] = ..., processing_time_us: _Optional[int] = ..., algorithm_used: _Optional[str] = ...) -> None: ...

class OperationMetrics(_message.Message):
    __slots__ = ("total_processed", "successful_count", "failed_count", "updated_count", "processing_time_us", "wal_write_time_us", "index_update_time_us")
    TOTAL_PROCESSED_FIELD_NUMBER: _ClassVar[int]
    SUCCESSFUL_COUNT_FIELD_NUMBER: _ClassVar[int]
    FAILED_COUNT_FIELD_NUMBER: _ClassVar[int]
    UPDATED_COUNT_FIELD_NUMBER: _ClassVar[int]
    PROCESSING_TIME_US_FIELD_NUMBER: _ClassVar[int]
    WAL_WRITE_TIME_US_FIELD_NUMBER: _ClassVar[int]
    INDEX_UPDATE_TIME_US_FIELD_NUMBER: _ClassVar[int]
    total_processed: int
    successful_count: int
    failed_count: int
    updated_count: int
    processing_time_us: int
    wal_write_time_us: int
    index_update_time_us: int
    def __init__(self, total_processed: _Optional[int] = ..., successful_count: _Optional[int] = ..., failed_count: _Optional[int] = ..., updated_count: _Optional[int] = ..., processing_time_us: _Optional[int] = ..., wal_write_time_us: _Optional[int] = ..., index_update_time_us: _Optional[int] = ...) -> None: ...

class HealthRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class HealthResponse(_message.Message):
    __slots__ = ("status", "version", "uptime_seconds", "active_connections", "memory_usage_bytes", "storage_usage_bytes")
    STATUS_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    UPTIME_SECONDS_FIELD_NUMBER: _ClassVar[int]
    ACTIVE_CONNECTIONS_FIELD_NUMBER: _ClassVar[int]
    MEMORY_USAGE_BYTES_FIELD_NUMBER: _ClassVar[int]
    STORAGE_USAGE_BYTES_FIELD_NUMBER: _ClassVar[int]
    status: str
    version: str
    uptime_seconds: int
    active_connections: int
    memory_usage_bytes: int
    storage_usage_bytes: int
    def __init__(self, status: _Optional[str] = ..., version: _Optional[str] = ..., uptime_seconds: _Optional[int] = ..., active_connections: _Optional[int] = ..., memory_usage_bytes: _Optional[int] = ..., storage_usage_bytes: _Optional[int] = ...) -> None: ...

class MetricsRequest(_message.Message):
    __slots__ = ("collection_id", "metric_names")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    METRIC_NAMES_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    metric_names: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, collection_id: _Optional[str] = ..., metric_names: _Optional[_Iterable[str]] = ...) -> None: ...

class MetricsResponse(_message.Message):
    __slots__ = ("metrics", "timestamp")
    class MetricsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: float
        def __init__(self, key: _Optional[str] = ..., value: _Optional[float] = ...) -> None: ...
    METRICS_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_FIELD_NUMBER: _ClassVar[int]
    metrics: _containers.ScalarMap[str, float]
    timestamp: int
    def __init__(self, metrics: _Optional[_Mapping[str, float]] = ..., timestamp: _Optional[int] = ...) -> None: ...

class CollectionSnapshot(_message.Message):
    __slots__ = ("collections", "version", "timestamp")
    COLLECTIONS_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_FIELD_NUMBER: _ClassVar[int]
    collections: _containers.RepeatedCompositeFieldContainer[Collection]
    version: int
    timestamp: int
    def __init__(self, collections: _Optional[_Iterable[_Union[Collection, _Mapping]]] = ..., version: _Optional[int] = ..., timestamp: _Optional[int] = ...) -> None: ...
