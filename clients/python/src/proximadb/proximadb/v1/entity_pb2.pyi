from proximadb.v1 import types_pb2 as _types_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Modality(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    TEXT: _ClassVar[Modality]
    IMAGE: _ClassVar[Modality]
    AUDIO: _ClassVar[Modality]
    VIDEO: _ClassVar[Modality]
    MULTIMODAL: _ClassVar[Modality]

class ComparisonOp(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    EQ: _ClassVar[ComparisonOp]
    NE: _ClassVar[ComparisonOp]
    GT: _ClassVar[ComparisonOp]
    GTE: _ClassVar[ComparisonOp]
    LT: _ClassVar[ComparisonOp]
    LTE: _ClassVar[ComparisonOp]
    IN: _ClassVar[ComparisonOp]
    NOT_IN: _ClassVar[ComparisonOp]
    CONTAINS: _ClassVar[ComparisonOp]

class LogicalOp(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    AND: _ClassVar[LogicalOp]
    OR: _ClassVar[LogicalOp]
    NOT: _ClassVar[LogicalOp]
TEXT: Modality
IMAGE: Modality
AUDIO: Modality
VIDEO: Modality
MULTIMODAL: Modality
EQ: ComparisonOp
NE: ComparisonOp
GT: ComparisonOp
GTE: ComparisonOp
LT: ComparisonOp
LTE: ComparisonOp
IN: ComparisonOp
NOT_IN: ComparisonOp
CONTAINS: ComparisonOp
AND: LogicalOp
OR: LogicalOp
NOT: LogicalOp

class Entity(_message.Message):
    __slots__ = ("id", "embeddings", "typed_metadata", "flexible_metadata", "provenance", "relations", "temporal", "collection_id")
    class FlexibleMetadataEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: _types_pb2.SqlValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[_types_pb2.SqlValue, _Mapping]] = ...) -> None: ...
    ID_FIELD_NUMBER: _ClassVar[int]
    EMBEDDINGS_FIELD_NUMBER: _ClassVar[int]
    TYPED_METADATA_FIELD_NUMBER: _ClassVar[int]
    FLEXIBLE_METADATA_FIELD_NUMBER: _ClassVar[int]
    PROVENANCE_FIELD_NUMBER: _ClassVar[int]
    RELATIONS_FIELD_NUMBER: _ClassVar[int]
    TEMPORAL_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    id: str
    embeddings: _containers.RepeatedCompositeFieldContainer[EmbeddingVersion]
    typed_metadata: TypedMetadata
    flexible_metadata: _containers.MessageMap[str, _types_pb2.SqlValue]
    provenance: Provenance
    relations: _containers.RepeatedCompositeFieldContainer[Relation]
    temporal: TemporalInfo
    collection_id: str
    def __init__(self, id: _Optional[str] = ..., embeddings: _Optional[_Iterable[_Union[EmbeddingVersion, _Mapping]]] = ..., typed_metadata: _Optional[_Union[TypedMetadata, _Mapping]] = ..., flexible_metadata: _Optional[_Mapping[str, _types_pb2.SqlValue]] = ..., provenance: _Optional[_Union[Provenance, _Mapping]] = ..., relations: _Optional[_Iterable[_Union[Relation, _Mapping]]] = ..., temporal: _Optional[_Union[TemporalInfo, _Mapping]] = ..., collection_id: _Optional[str] = ...) -> None: ...

class EmbeddingVersion(_message.Message):
    __slots__ = ("model_id", "model_version", "vector", "dimension", "created_at_ms", "model_params", "modality")
    class ModelParamsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    MODEL_ID_FIELD_NUMBER: _ClassVar[int]
    MODEL_VERSION_FIELD_NUMBER: _ClassVar[int]
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    DIMENSION_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_MS_FIELD_NUMBER: _ClassVar[int]
    MODEL_PARAMS_FIELD_NUMBER: _ClassVar[int]
    MODALITY_FIELD_NUMBER: _ClassVar[int]
    model_id: str
    model_version: str
    vector: _containers.RepeatedScalarFieldContainer[float]
    dimension: int
    created_at_ms: int
    model_params: _containers.ScalarMap[str, str]
    modality: Modality
    def __init__(self, model_id: _Optional[str] = ..., model_version: _Optional[str] = ..., vector: _Optional[_Iterable[float]] = ..., dimension: _Optional[int] = ..., created_at_ms: _Optional[int] = ..., model_params: _Optional[_Mapping[str, str]] = ..., modality: _Optional[_Union[Modality, str]] = ...) -> None: ...

class TypedMetadata(_message.Message):
    __slots__ = ("fields",)
    class FieldsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: TypedField
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[TypedField, _Mapping]] = ...) -> None: ...
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    fields: _containers.MessageMap[str, TypedField]
    def __init__(self, fields: _Optional[_Mapping[str, TypedField]] = ...) -> None: ...

class TypedField(_message.Message):
    __slots__ = ("string_value", "int_value", "double_value", "bool_value", "string_array", "timestamp_value_ms", "indexed", "filterable")
    STRING_VALUE_FIELD_NUMBER: _ClassVar[int]
    INT_VALUE_FIELD_NUMBER: _ClassVar[int]
    DOUBLE_VALUE_FIELD_NUMBER: _ClassVar[int]
    BOOL_VALUE_FIELD_NUMBER: _ClassVar[int]
    STRING_ARRAY_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_VALUE_MS_FIELD_NUMBER: _ClassVar[int]
    INDEXED_FIELD_NUMBER: _ClassVar[int]
    FILTERABLE_FIELD_NUMBER: _ClassVar[int]
    string_value: str
    int_value: int
    double_value: float
    bool_value: bool
    string_array: StringArray
    timestamp_value_ms: int
    indexed: bool
    filterable: bool
    def __init__(self, string_value: _Optional[str] = ..., int_value: _Optional[int] = ..., double_value: _Optional[float] = ..., bool_value: bool = ..., string_array: _Optional[_Union[StringArray, _Mapping]] = ..., timestamp_value_ms: _Optional[int] = ..., indexed: bool = ..., filterable: bool = ...) -> None: ...

class StringArray(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, values: _Optional[_Iterable[str]] = ...) -> None: ...

class Provenance(_message.Message):
    __slots__ = ("source_id", "chunk_id", "chunk_position", "extraction_method", "extracted_at_ms", "metadata")
    class MetadataEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    SOURCE_ID_FIELD_NUMBER: _ClassVar[int]
    CHUNK_ID_FIELD_NUMBER: _ClassVar[int]
    CHUNK_POSITION_FIELD_NUMBER: _ClassVar[int]
    EXTRACTION_METHOD_FIELD_NUMBER: _ClassVar[int]
    EXTRACTED_AT_MS_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    source_id: str
    chunk_id: str
    chunk_position: int
    extraction_method: str
    extracted_at_ms: int
    metadata: _containers.ScalarMap[str, str]
    def __init__(self, source_id: _Optional[str] = ..., chunk_id: _Optional[str] = ..., chunk_position: _Optional[int] = ..., extraction_method: _Optional[str] = ..., extracted_at_ms: _Optional[int] = ..., metadata: _Optional[_Mapping[str, str]] = ...) -> None: ...

class TemporalInfo(_message.Message):
    __slots__ = ("created_at_ms", "valid_from_ms", "valid_to_ms", "is_current", "versions")
    CREATED_AT_MS_FIELD_NUMBER: _ClassVar[int]
    VALID_FROM_MS_FIELD_NUMBER: _ClassVar[int]
    VALID_TO_MS_FIELD_NUMBER: _ClassVar[int]
    IS_CURRENT_FIELD_NUMBER: _ClassVar[int]
    VERSIONS_FIELD_NUMBER: _ClassVar[int]
    created_at_ms: int
    valid_from_ms: int
    valid_to_ms: int
    is_current: bool
    versions: _containers.RepeatedCompositeFieldContainer[TemporalVersion]
    def __init__(self, created_at_ms: _Optional[int] = ..., valid_from_ms: _Optional[int] = ..., valid_to_ms: _Optional[int] = ..., is_current: bool = ..., versions: _Optional[_Iterable[_Union[TemporalVersion, _Mapping]]] = ...) -> None: ...

class TemporalVersion(_message.Message):
    __slots__ = ("timestamp_ms", "version_id", "changes")
    class ChangesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    TIMESTAMP_MS_FIELD_NUMBER: _ClassVar[int]
    VERSION_ID_FIELD_NUMBER: _ClassVar[int]
    CHANGES_FIELD_NUMBER: _ClassVar[int]
    timestamp_ms: int
    version_id: str
    changes: _containers.ScalarMap[str, str]
    def __init__(self, timestamp_ms: _Optional[int] = ..., version_id: _Optional[str] = ..., changes: _Optional[_Mapping[str, str]] = ...) -> None: ...

class Relation(_message.Message):
    __slots__ = ("source_entity_id", "target_entity_id", "relation_type", "weight", "created_at_ms", "properties")
    class PropertiesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    SOURCE_ENTITY_ID_FIELD_NUMBER: _ClassVar[int]
    TARGET_ENTITY_ID_FIELD_NUMBER: _ClassVar[int]
    RELATION_TYPE_FIELD_NUMBER: _ClassVar[int]
    WEIGHT_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_MS_FIELD_NUMBER: _ClassVar[int]
    PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    source_entity_id: str
    target_entity_id: str
    relation_type: str
    weight: float
    created_at_ms: int
    properties: _containers.ScalarMap[str, str]
    def __init__(self, source_entity_id: _Optional[str] = ..., target_entity_id: _Optional[str] = ..., relation_type: _Optional[str] = ..., weight: _Optional[float] = ..., created_at_ms: _Optional[int] = ..., properties: _Optional[_Mapping[str, str]] = ...) -> None: ...

class UpsertEntityRequest(_message.Message):
    __slots__ = ("collection_id", "entity", "create_collection_if_missing")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    ENTITY_FIELD_NUMBER: _ClassVar[int]
    CREATE_COLLECTION_IF_MISSING_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    entity: Entity
    create_collection_if_missing: bool
    def __init__(self, collection_id: _Optional[str] = ..., entity: _Optional[_Union[Entity, _Mapping]] = ..., create_collection_if_missing: bool = ...) -> None: ...

class UpsertEntityResponse(_message.Message):
    __slots__ = ("success", "entity_id", "message")
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    ENTITY_ID_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    success: bool
    entity_id: str
    message: str
    def __init__(self, success: bool = ..., entity_id: _Optional[str] = ..., message: _Optional[str] = ...) -> None: ...

class GetEntityRequest(_message.Message):
    __slots__ = ("collection_id", "entity_id", "include_embeddings", "include_relations")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    ENTITY_ID_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_EMBEDDINGS_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_RELATIONS_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    entity_id: str
    include_embeddings: bool
    include_relations: bool
    def __init__(self, collection_id: _Optional[str] = ..., entity_id: _Optional[str] = ..., include_embeddings: bool = ..., include_relations: bool = ...) -> None: ...

class GetEntityResponse(_message.Message):
    __slots__ = ("entity",)
    ENTITY_FIELD_NUMBER: _ClassVar[int]
    entity: Entity
    def __init__(self, entity: _Optional[_Union[Entity, _Mapping]] = ...) -> None: ...

class DeleteEntityRequest(_message.Message):
    __slots__ = ("collection_id", "entity_id", "hard_delete")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    ENTITY_ID_FIELD_NUMBER: _ClassVar[int]
    HARD_DELETE_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    entity_id: str
    hard_delete: bool
    def __init__(self, collection_id: _Optional[str] = ..., entity_id: _Optional[str] = ..., hard_delete: bool = ...) -> None: ...

class DeleteEntityResponse(_message.Message):
    __slots__ = ("success", "message")
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    success: bool
    message: str
    def __init__(self, success: bool = ..., message: _Optional[str] = ...) -> None: ...

class SearchEntitiesRequest(_message.Message):
    __slots__ = ("collection_id", "similar", "filters", "temporal", "top_k", "progressive")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    SIMILAR_FIELD_NUMBER: _ClassVar[int]
    FILTERS_FIELD_NUMBER: _ClassVar[int]
    TEMPORAL_FIELD_NUMBER: _ClassVar[int]
    TOP_K_FIELD_NUMBER: _ClassVar[int]
    PROGRESSIVE_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    similar: SimilarQuery
    filters: MetadataFilter
    temporal: TemporalClause
    top_k: int
    progressive: bool
    def __init__(self, collection_id: _Optional[str] = ..., similar: _Optional[_Union[SimilarQuery, _Mapping]] = ..., filters: _Optional[_Union[MetadataFilter, _Mapping]] = ..., temporal: _Optional[_Union[TemporalClause, _Mapping]] = ..., top_k: _Optional[int] = ..., progressive: bool = ...) -> None: ...

class SearchEntitiesResponse(_message.Message):
    __slots__ = ("results", "total", "page_info", "progress")
    RESULTS_FIELD_NUMBER: _ClassVar[int]
    TOTAL_FIELD_NUMBER: _ClassVar[int]
    PAGE_INFO_FIELD_NUMBER: _ClassVar[int]
    PROGRESS_FIELD_NUMBER: _ClassVar[int]
    results: _containers.RepeatedCompositeFieldContainer[EntityResult]
    total: int
    page_info: PageInfo
    progress: ProgressInfo
    def __init__(self, results: _Optional[_Iterable[_Union[EntityResult, _Mapping]]] = ..., total: _Optional[int] = ..., page_info: _Optional[_Union[PageInfo, _Mapping]] = ..., progress: _Optional[_Union[ProgressInfo, _Mapping]] = ...) -> None: ...

class EntityResult(_message.Message):
    __slots__ = ("entity", "score", "debug_info")
    class DebugInfoEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    ENTITY_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    DEBUG_INFO_FIELD_NUMBER: _ClassVar[int]
    entity: Entity
    score: float
    debug_info: _containers.ScalarMap[str, str]
    def __init__(self, entity: _Optional[_Union[Entity, _Mapping]] = ..., score: _Optional[float] = ..., debug_info: _Optional[_Mapping[str, str]] = ...) -> None: ...

class VectorData(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedScalarFieldContainer[float]
    def __init__(self, values: _Optional[_Iterable[float]] = ...) -> None: ...

class SimilarQuery(_message.Message):
    __slots__ = ("text", "vector", "raw_data", "model_id", "modality")
    TEXT_FIELD_NUMBER: _ClassVar[int]
    VECTOR_FIELD_NUMBER: _ClassVar[int]
    RAW_DATA_FIELD_NUMBER: _ClassVar[int]
    MODEL_ID_FIELD_NUMBER: _ClassVar[int]
    MODALITY_FIELD_NUMBER: _ClassVar[int]
    text: str
    vector: VectorData
    raw_data: bytes
    model_id: str
    modality: Modality
    def __init__(self, text: _Optional[str] = ..., vector: _Optional[_Union[VectorData, _Mapping]] = ..., raw_data: _Optional[bytes] = ..., model_id: _Optional[str] = ..., modality: _Optional[_Union[Modality, str]] = ...) -> None: ...

class MetadataFilter(_message.Message):
    __slots__ = ("clauses", "op")
    CLAUSES_FIELD_NUMBER: _ClassVar[int]
    OP_FIELD_NUMBER: _ClassVar[int]
    clauses: _containers.RepeatedCompositeFieldContainer[FilterClause]
    op: LogicalOp
    def __init__(self, clauses: _Optional[_Iterable[_Union[FilterClause, _Mapping]]] = ..., op: _Optional[_Union[LogicalOp, str]] = ...) -> None: ...

class FilterClause(_message.Message):
    __slots__ = ("field", "op", "string_value", "int_value", "double_value", "bool_value")
    FIELD_FIELD_NUMBER: _ClassVar[int]
    OP_FIELD_NUMBER: _ClassVar[int]
    STRING_VALUE_FIELD_NUMBER: _ClassVar[int]
    INT_VALUE_FIELD_NUMBER: _ClassVar[int]
    DOUBLE_VALUE_FIELD_NUMBER: _ClassVar[int]
    BOOL_VALUE_FIELD_NUMBER: _ClassVar[int]
    field: str
    op: ComparisonOp
    string_value: str
    int_value: int
    double_value: float
    bool_value: bool
    def __init__(self, field: _Optional[str] = ..., op: _Optional[_Union[ComparisonOp, str]] = ..., string_value: _Optional[str] = ..., int_value: _Optional[int] = ..., double_value: _Optional[float] = ..., bool_value: bool = ...) -> None: ...

class TemporalClause(_message.Message):
    __slots__ = ("at_time_ms", "valid_between")
    AT_TIME_MS_FIELD_NUMBER: _ClassVar[int]
    VALID_BETWEEN_FIELD_NUMBER: _ClassVar[int]
    at_time_ms: int
    valid_between: TimeRange
    def __init__(self, at_time_ms: _Optional[int] = ..., valid_between: _Optional[_Union[TimeRange, _Mapping]] = ...) -> None: ...

class TimeRange(_message.Message):
    __slots__ = ("start_ms", "end_ms")
    START_MS_FIELD_NUMBER: _ClassVar[int]
    END_MS_FIELD_NUMBER: _ClassVar[int]
    start_ms: int
    end_ms: int
    def __init__(self, start_ms: _Optional[int] = ..., end_ms: _Optional[int] = ...) -> None: ...

class PageInfo(_message.Message):
    __slots__ = ("cursor", "has_more")
    CURSOR_FIELD_NUMBER: _ClassVar[int]
    HAS_MORE_FIELD_NUMBER: _ClassVar[int]
    cursor: str
    has_more: bool
    def __init__(self, cursor: _Optional[str] = ..., has_more: bool = ...) -> None: ...

class ProgressInfo(_message.Message):
    __slots__ = ("stage", "total_stages", "complete")
    STAGE_FIELD_NUMBER: _ClassVar[int]
    TOTAL_STAGES_FIELD_NUMBER: _ClassVar[int]
    COMPLETE_FIELD_NUMBER: _ClassVar[int]
    stage: int
    total_stages: int
    complete: bool
    def __init__(self, stage: _Optional[int] = ..., total_stages: _Optional[int] = ..., complete: bool = ...) -> None: ...
