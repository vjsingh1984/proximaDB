from google.protobuf import timestamp_pb2 as _timestamp_pb2
from proximadb.v1 import collection_types_pb2 as _collection_types_pb2
from proximadb.v1 import graph_pb2 as _graph_pb2
from proximadb.v1 import vector_types_pb2 as _vector_types_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class PropertyType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    PROPERTY_TYPE_UNSPECIFIED: _ClassVar[PropertyType]
    STRING: _ClassVar[PropertyType]
    INTEGER: _ClassVar[PropertyType]
    FLOAT: _ClassVar[PropertyType]
    BOOLEAN: _ClassVar[PropertyType]
    DATETIME: _ClassVar[PropertyType]
    JSON: _ClassVar[PropertyType]
    ARRAY: _ClassVar[PropertyType]
    EMBEDDING: _ClassVar[PropertyType]

class Cardinality(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    CARDINALITY_UNSPECIFIED: _ClassVar[Cardinality]
    ONE_TO_ONE: _ClassVar[Cardinality]
    ONE_TO_MANY: _ClassVar[Cardinality]
    MANY_TO_ONE: _ClassVar[Cardinality]
    MANY_TO_MANY: _ClassVar[Cardinality]

class PermissionType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    PERMISSION_TYPE_UNSPECIFIED: _ClassVar[PermissionType]
    READ: _ClassVar[PermissionType]
    WRITE: _ClassVar[PermissionType]
    ADMIN: _ClassVar[PermissionType]
    DELETE: _ClassVar[PermissionType]
PROPERTY_TYPE_UNSPECIFIED: PropertyType
STRING: PropertyType
INTEGER: PropertyType
FLOAT: PropertyType
BOOLEAN: PropertyType
DATETIME: PropertyType
JSON: PropertyType
ARRAY: PropertyType
EMBEDDING: PropertyType
CARDINALITY_UNSPECIFIED: Cardinality
ONE_TO_ONE: Cardinality
ONE_TO_MANY: Cardinality
MANY_TO_ONE: Cardinality
MANY_TO_MANY: Cardinality
PERMISSION_TYPE_UNSPECIFIED: PermissionType
READ: PermissionType
WRITE: PermissionType
ADMIN: PermissionType
DELETE: PermissionType

class GraphCollection(_message.Message):
    __slots__ = ("graph_id", "name", "description", "schema", "storage_config", "engine_config", "access_control", "stats", "created_at", "updated_at")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    DESCRIPTION_FIELD_NUMBER: _ClassVar[int]
    SCHEMA_FIELD_NUMBER: _ClassVar[int]
    STORAGE_CONFIG_FIELD_NUMBER: _ClassVar[int]
    ENGINE_CONFIG_FIELD_NUMBER: _ClassVar[int]
    ACCESS_CONTROL_FIELD_NUMBER: _ClassVar[int]
    STATS_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_FIELD_NUMBER: _ClassVar[int]
    UPDATED_AT_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    name: str
    description: str
    schema: GraphSchema
    storage_config: GraphStorageConfig
    engine_config: GraphEngineConfig
    access_control: AccessControl
    stats: _graph_pb2.GraphStats
    created_at: int
    updated_at: int
    def __init__(self, graph_id: _Optional[str] = ..., name: _Optional[str] = ..., description: _Optional[str] = ..., schema: _Optional[_Union[GraphSchema, _Mapping]] = ..., storage_config: _Optional[_Union[GraphStorageConfig, _Mapping]] = ..., engine_config: _Optional[_Union[GraphEngineConfig, _Mapping]] = ..., access_control: _Optional[_Union[AccessControl, _Mapping]] = ..., stats: _Optional[_Union[_graph_pb2.GraphStats, _Mapping]] = ..., created_at: _Optional[int] = ..., updated_at: _Optional[int] = ...) -> None: ...

class GraphSchema(_message.Message):
    __slots__ = ("node_labels", "edge_types", "properties", "unique_constraints", "strict_mode")
    class PropertiesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: PropertySchema
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[PropertySchema, _Mapping]] = ...) -> None: ...
    NODE_LABELS_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPES_FIELD_NUMBER: _ClassVar[int]
    PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    UNIQUE_CONSTRAINTS_FIELD_NUMBER: _ClassVar[int]
    STRICT_MODE_FIELD_NUMBER: _ClassVar[int]
    node_labels: _containers.RepeatedCompositeFieldContainer[NodeLabelSchema]
    edge_types: _containers.RepeatedCompositeFieldContainer[EdgeTypeSchema]
    properties: _containers.MessageMap[str, PropertySchema]
    unique_constraints: _containers.RepeatedCompositeFieldContainer[UniqueConstraint]
    strict_mode: bool
    def __init__(self, node_labels: _Optional[_Iterable[_Union[NodeLabelSchema, _Mapping]]] = ..., edge_types: _Optional[_Iterable[_Union[EdgeTypeSchema, _Mapping]]] = ..., properties: _Optional[_Mapping[str, PropertySchema]] = ..., unique_constraints: _Optional[_Iterable[_Union[UniqueConstraint, _Mapping]]] = ..., strict_mode: bool = ...) -> None: ...

class NodeLabelSchema(_message.Message):
    __slots__ = ("label", "required_properties", "optional_properties", "allow_additional_properties", "property_constraints")
    class PropertyConstraintsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: PropertyConstraint
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[PropertyConstraint, _Mapping]] = ...) -> None: ...
    LABEL_FIELD_NUMBER: _ClassVar[int]
    REQUIRED_PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    OPTIONAL_PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    ALLOW_ADDITIONAL_PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    PROPERTY_CONSTRAINTS_FIELD_NUMBER: _ClassVar[int]
    label: str
    required_properties: _containers.RepeatedScalarFieldContainer[str]
    optional_properties: _containers.RepeatedScalarFieldContainer[str]
    allow_additional_properties: bool
    property_constraints: _containers.MessageMap[str, PropertyConstraint]
    def __init__(self, label: _Optional[str] = ..., required_properties: _Optional[_Iterable[str]] = ..., optional_properties: _Optional[_Iterable[str]] = ..., allow_additional_properties: bool = ..., property_constraints: _Optional[_Mapping[str, PropertyConstraint]] = ...) -> None: ...

class EdgeTypeSchema(_message.Message):
    __slots__ = ("edge_type", "source_labels", "target_labels", "required_properties", "optional_properties", "allow_additional_properties", "cardinality", "property_constraints")
    class PropertyConstraintsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: PropertyConstraint
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[PropertyConstraint, _Mapping]] = ...) -> None: ...
    EDGE_TYPE_FIELD_NUMBER: _ClassVar[int]
    SOURCE_LABELS_FIELD_NUMBER: _ClassVar[int]
    TARGET_LABELS_FIELD_NUMBER: _ClassVar[int]
    REQUIRED_PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    OPTIONAL_PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    ALLOW_ADDITIONAL_PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    CARDINALITY_FIELD_NUMBER: _ClassVar[int]
    PROPERTY_CONSTRAINTS_FIELD_NUMBER: _ClassVar[int]
    edge_type: str
    source_labels: _containers.RepeatedScalarFieldContainer[str]
    target_labels: _containers.RepeatedScalarFieldContainer[str]
    required_properties: _containers.RepeatedScalarFieldContainer[str]
    optional_properties: _containers.RepeatedScalarFieldContainer[str]
    allow_additional_properties: bool
    cardinality: Cardinality
    property_constraints: _containers.MessageMap[str, PropertyConstraint]
    def __init__(self, edge_type: _Optional[str] = ..., source_labels: _Optional[_Iterable[str]] = ..., target_labels: _Optional[_Iterable[str]] = ..., required_properties: _Optional[_Iterable[str]] = ..., optional_properties: _Optional[_Iterable[str]] = ..., allow_additional_properties: bool = ..., cardinality: _Optional[_Union[Cardinality, str]] = ..., property_constraints: _Optional[_Mapping[str, PropertyConstraint]] = ...) -> None: ...

class PropertySchema(_message.Message):
    __slots__ = ("name", "type", "required", "default_value", "constraints", "description")
    NAME_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    REQUIRED_FIELD_NUMBER: _ClassVar[int]
    DEFAULT_VALUE_FIELD_NUMBER: _ClassVar[int]
    CONSTRAINTS_FIELD_NUMBER: _ClassVar[int]
    DESCRIPTION_FIELD_NUMBER: _ClassVar[int]
    name: str
    type: PropertyType
    required: bool
    default_value: _graph_pb2.PropertyValue
    constraints: _containers.RepeatedCompositeFieldContainer[PropertyConstraint]
    description: str
    def __init__(self, name: _Optional[str] = ..., type: _Optional[_Union[PropertyType, str]] = ..., required: bool = ..., default_value: _Optional[_Union[_graph_pb2.PropertyValue, _Mapping]] = ..., constraints: _Optional[_Iterable[_Union[PropertyConstraint, _Mapping]]] = ..., description: _Optional[str] = ...) -> None: ...

class PropertyConstraint(_message.Message):
    __slots__ = ("string_constraint", "numeric_constraint", "array_constraint", "regex_constraint")
    STRING_CONSTRAINT_FIELD_NUMBER: _ClassVar[int]
    NUMERIC_CONSTRAINT_FIELD_NUMBER: _ClassVar[int]
    ARRAY_CONSTRAINT_FIELD_NUMBER: _ClassVar[int]
    REGEX_CONSTRAINT_FIELD_NUMBER: _ClassVar[int]
    string_constraint: StringConstraint
    numeric_constraint: NumericConstraint
    array_constraint: ArrayConstraint
    regex_constraint: RegexConstraint
    def __init__(self, string_constraint: _Optional[_Union[StringConstraint, _Mapping]] = ..., numeric_constraint: _Optional[_Union[NumericConstraint, _Mapping]] = ..., array_constraint: _Optional[_Union[ArrayConstraint, _Mapping]] = ..., regex_constraint: _Optional[_Union[RegexConstraint, _Mapping]] = ...) -> None: ...

class StringConstraint(_message.Message):
    __slots__ = ("min_length", "max_length", "allowed_values")
    MIN_LENGTH_FIELD_NUMBER: _ClassVar[int]
    MAX_LENGTH_FIELD_NUMBER: _ClassVar[int]
    ALLOWED_VALUES_FIELD_NUMBER: _ClassVar[int]
    min_length: int
    max_length: int
    allowed_values: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, min_length: _Optional[int] = ..., max_length: _Optional[int] = ..., allowed_values: _Optional[_Iterable[str]] = ...) -> None: ...

class NumericConstraint(_message.Message):
    __slots__ = ("min_value", "max_value", "multiple_of")
    MIN_VALUE_FIELD_NUMBER: _ClassVar[int]
    MAX_VALUE_FIELD_NUMBER: _ClassVar[int]
    MULTIPLE_OF_FIELD_NUMBER: _ClassVar[int]
    min_value: float
    max_value: float
    multiple_of: float
    def __init__(self, min_value: _Optional[float] = ..., max_value: _Optional[float] = ..., multiple_of: _Optional[float] = ...) -> None: ...

class ArrayConstraint(_message.Message):
    __slots__ = ("min_items", "max_items", "item_type")
    MIN_ITEMS_FIELD_NUMBER: _ClassVar[int]
    MAX_ITEMS_FIELD_NUMBER: _ClassVar[int]
    ITEM_TYPE_FIELD_NUMBER: _ClassVar[int]
    min_items: int
    max_items: int
    item_type: PropertyType
    def __init__(self, min_items: _Optional[int] = ..., max_items: _Optional[int] = ..., item_type: _Optional[_Union[PropertyType, str]] = ...) -> None: ...

class RegexConstraint(_message.Message):
    __slots__ = ("pattern", "flags")
    PATTERN_FIELD_NUMBER: _ClassVar[int]
    FLAGS_FIELD_NUMBER: _ClassVar[int]
    pattern: str
    flags: str
    def __init__(self, pattern: _Optional[str] = ..., flags: _Optional[str] = ...) -> None: ...

class UniqueConstraint(_message.Message):
    __slots__ = ("name", "node_labels", "properties", "description")
    NAME_FIELD_NUMBER: _ClassVar[int]
    NODE_LABELS_FIELD_NUMBER: _ClassVar[int]
    PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    DESCRIPTION_FIELD_NUMBER: _ClassVar[int]
    name: str
    node_labels: _containers.RepeatedScalarFieldContainer[str]
    properties: _containers.RepeatedScalarFieldContainer[str]
    description: str
    def __init__(self, name: _Optional[str] = ..., node_labels: _Optional[_Iterable[str]] = ..., properties: _Optional[_Iterable[str]] = ..., description: _Optional[str] = ...) -> None: ...

class GraphStorageConfig(_message.Message):
    __slots__ = ("engine_type", "base_url", "compression", "enable_wal", "snapshot_interval_hours", "engine_specific_config")
    class EngineSpecificConfigEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    ENGINE_TYPE_FIELD_NUMBER: _ClassVar[int]
    BASE_URL_FIELD_NUMBER: _ClassVar[int]
    COMPRESSION_FIELD_NUMBER: _ClassVar[int]
    ENABLE_WAL_FIELD_NUMBER: _ClassVar[int]
    SNAPSHOT_INTERVAL_HOURS_FIELD_NUMBER: _ClassVar[int]
    ENGINE_SPECIFIC_CONFIG_FIELD_NUMBER: _ClassVar[int]
    engine_type: str
    base_url: str
    compression: _vector_types_pb2.CompressionAlgorithm
    enable_wal: bool
    snapshot_interval_hours: int
    engine_specific_config: _containers.ScalarMap[str, str]
    def __init__(self, engine_type: _Optional[str] = ..., base_url: _Optional[str] = ..., compression: _Optional[_Union[_vector_types_pb2.CompressionAlgorithm, str]] = ..., enable_wal: bool = ..., snapshot_interval_hours: _Optional[int] = ..., engine_specific_config: _Optional[_Mapping[str, str]] = ...) -> None: ...

class GraphEngineConfig(_message.Message):
    __slots__ = ("engine_type", "memory_pool_size_mb", "csr_cache_size_mb", "enable_parallel_operations", "max_traversal_depth", "advanced_config")
    class AdvancedConfigEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    ENGINE_TYPE_FIELD_NUMBER: _ClassVar[int]
    MEMORY_POOL_SIZE_MB_FIELD_NUMBER: _ClassVar[int]
    CSR_CACHE_SIZE_MB_FIELD_NUMBER: _ClassVar[int]
    ENABLE_PARALLEL_OPERATIONS_FIELD_NUMBER: _ClassVar[int]
    MAX_TRAVERSAL_DEPTH_FIELD_NUMBER: _ClassVar[int]
    ADVANCED_CONFIG_FIELD_NUMBER: _ClassVar[int]
    engine_type: str
    memory_pool_size_mb: int
    csr_cache_size_mb: int
    enable_parallel_operations: bool
    max_traversal_depth: int
    advanced_config: _containers.ScalarMap[str, str]
    def __init__(self, engine_type: _Optional[str] = ..., memory_pool_size_mb: _Optional[int] = ..., csr_cache_size_mb: _Optional[int] = ..., enable_parallel_operations: bool = ..., max_traversal_depth: _Optional[int] = ..., advanced_config: _Optional[_Mapping[str, str]] = ...) -> None: ...

class AccessControl(_message.Message):
    __slots__ = ("permissions", "owner", "admins", "readers", "writers")
    PERMISSIONS_FIELD_NUMBER: _ClassVar[int]
    OWNER_FIELD_NUMBER: _ClassVar[int]
    ADMINS_FIELD_NUMBER: _ClassVar[int]
    READERS_FIELD_NUMBER: _ClassVar[int]
    WRITERS_FIELD_NUMBER: _ClassVar[int]
    permissions: _containers.RepeatedCompositeFieldContainer[Permission]
    owner: str
    admins: _containers.RepeatedScalarFieldContainer[str]
    readers: _containers.RepeatedScalarFieldContainer[str]
    writers: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, permissions: _Optional[_Iterable[_Union[Permission, _Mapping]]] = ..., owner: _Optional[str] = ..., admins: _Optional[_Iterable[str]] = ..., readers: _Optional[_Iterable[str]] = ..., writers: _Optional[_Iterable[str]] = ...) -> None: ...

class Permission(_message.Message):
    __slots__ = ("user_or_role", "type", "scopes")
    USER_OR_ROLE_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    SCOPES_FIELD_NUMBER: _ClassVar[int]
    user_or_role: str
    type: PermissionType
    scopes: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, user_or_role: _Optional[str] = ..., type: _Optional[_Union[PermissionType, str]] = ..., scopes: _Optional[_Iterable[str]] = ...) -> None: ...

class CreateGraphRequest(_message.Message):
    __slots__ = ("graph_id", "name", "description", "schema", "storage_config", "engine_config", "access_control")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    NAME_FIELD_NUMBER: _ClassVar[int]
    DESCRIPTION_FIELD_NUMBER: _ClassVar[int]
    SCHEMA_FIELD_NUMBER: _ClassVar[int]
    STORAGE_CONFIG_FIELD_NUMBER: _ClassVar[int]
    ENGINE_CONFIG_FIELD_NUMBER: _ClassVar[int]
    ACCESS_CONTROL_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    name: str
    description: str
    schema: GraphSchema
    storage_config: GraphStorageConfig
    engine_config: GraphEngineConfig
    access_control: AccessControl
    def __init__(self, graph_id: _Optional[str] = ..., name: _Optional[str] = ..., description: _Optional[str] = ..., schema: _Optional[_Union[GraphSchema, _Mapping]] = ..., storage_config: _Optional[_Union[GraphStorageConfig, _Mapping]] = ..., engine_config: _Optional[_Union[GraphEngineConfig, _Mapping]] = ..., access_control: _Optional[_Union[AccessControl, _Mapping]] = ...) -> None: ...

class UpdateSchemaRequest(_message.Message):
    __slots__ = ("graph_id", "schema", "validate_existing_data", "force_migration")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    SCHEMA_FIELD_NUMBER: _ClassVar[int]
    VALIDATE_EXISTING_DATA_FIELD_NUMBER: _ClassVar[int]
    FORCE_MIGRATION_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    schema: GraphSchema
    validate_existing_data: bool
    force_migration: bool
    def __init__(self, graph_id: _Optional[str] = ..., schema: _Optional[_Union[GraphSchema, _Mapping]] = ..., validate_existing_data: bool = ..., force_migration: bool = ...) -> None: ...

class SchemaValidationResult(_message.Message):
    __slots__ = ("valid", "errors", "warnings", "affected_nodes", "affected_edges")
    VALID_FIELD_NUMBER: _ClassVar[int]
    ERRORS_FIELD_NUMBER: _ClassVar[int]
    WARNINGS_FIELD_NUMBER: _ClassVar[int]
    AFFECTED_NODES_FIELD_NUMBER: _ClassVar[int]
    AFFECTED_EDGES_FIELD_NUMBER: _ClassVar[int]
    valid: bool
    errors: _containers.RepeatedCompositeFieldContainer[ValidationError]
    warnings: _containers.RepeatedCompositeFieldContainer[ValidationWarning]
    affected_nodes: int
    affected_edges: int
    def __init__(self, valid: bool = ..., errors: _Optional[_Iterable[_Union[ValidationError, _Mapping]]] = ..., warnings: _Optional[_Iterable[_Union[ValidationWarning, _Mapping]]] = ..., affected_nodes: _Optional[int] = ..., affected_edges: _Optional[int] = ...) -> None: ...

class ValidationError(_message.Message):
    __slots__ = ("message", "path", "severity")
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    PATH_FIELD_NUMBER: _ClassVar[int]
    SEVERITY_FIELD_NUMBER: _ClassVar[int]
    message: str
    path: str
    severity: str
    def __init__(self, message: _Optional[str] = ..., path: _Optional[str] = ..., severity: _Optional[str] = ...) -> None: ...

class ValidationWarning(_message.Message):
    __slots__ = ("message", "path", "suggestion")
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    PATH_FIELD_NUMBER: _ClassVar[int]
    SUGGESTION_FIELD_NUMBER: _ClassVar[int]
    message: str
    path: str
    suggestion: str
    def __init__(self, message: _Optional[str] = ..., path: _Optional[str] = ..., suggestion: _Optional[str] = ...) -> None: ...
