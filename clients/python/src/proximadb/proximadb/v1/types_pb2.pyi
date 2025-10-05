from google.protobuf import struct_pb2 as _struct_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class SqlArray(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedCompositeFieldContainer[SqlValue]
    def __init__(self, values: _Optional[_Iterable[_Union[SqlValue, _Mapping]]] = ...) -> None: ...

class SqlObject(_message.Message):
    __slots__ = ("fields",)
    class FieldsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: SqlValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[SqlValue, _Mapping]] = ...) -> None: ...
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    fields: _containers.MessageMap[str, SqlValue]
    def __init__(self, fields: _Optional[_Mapping[str, SqlValue]] = ...) -> None: ...

class SqlValue(_message.Message):
    __slots__ = ("string_value", "number_value", "bool_value", "int64_value", "bytes_value", "null_value", "array_value", "object_value")
    STRING_VALUE_FIELD_NUMBER: _ClassVar[int]
    NUMBER_VALUE_FIELD_NUMBER: _ClassVar[int]
    BOOL_VALUE_FIELD_NUMBER: _ClassVar[int]
    INT64_VALUE_FIELD_NUMBER: _ClassVar[int]
    BYTES_VALUE_FIELD_NUMBER: _ClassVar[int]
    NULL_VALUE_FIELD_NUMBER: _ClassVar[int]
    ARRAY_VALUE_FIELD_NUMBER: _ClassVar[int]
    OBJECT_VALUE_FIELD_NUMBER: _ClassVar[int]
    string_value: str
    number_value: float
    bool_value: bool
    int64_value: int
    bytes_value: bytes
    null_value: _struct_pb2.NullValue
    array_value: SqlArray
    object_value: SqlObject
    def __init__(self, string_value: _Optional[str] = ..., number_value: _Optional[float] = ..., bool_value: bool = ..., int64_value: _Optional[int] = ..., bytes_value: _Optional[bytes] = ..., null_value: _Optional[_Union[_struct_pb2.NullValue, str]] = ..., array_value: _Optional[_Union[SqlArray, _Mapping]] = ..., object_value: _Optional[_Union[SqlObject, _Mapping]] = ...) -> None: ...

class SqlRowField(_message.Message):
    __slots__ = ("key", "value")
    KEY_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    key: str
    value: SqlValue
    def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[SqlValue, _Mapping]] = ...) -> None: ...

class SqlRow(_message.Message):
    __slots__ = ("fields", "similarity")
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    SIMILARITY_FIELD_NUMBER: _ClassVar[int]
    fields: _containers.RepeatedCompositeFieldContainer[SqlRowField]
    similarity: float
    def __init__(self, fields: _Optional[_Iterable[_Union[SqlRowField, _Mapping]]] = ..., similarity: _Optional[float] = ...) -> None: ...

class ExecuteSqlRequest(_message.Message):
    __slots__ = ("query", "parameters", "collection", "limit", "offset")
    QUERY_FIELD_NUMBER: _ClassVar[int]
    PARAMETERS_FIELD_NUMBER: _ClassVar[int]
    COLLECTION_FIELD_NUMBER: _ClassVar[int]
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    query: str
    parameters: _containers.RepeatedCompositeFieldContainer[SqlValue]
    collection: str
    limit: int
    offset: int
    def __init__(self, query: _Optional[str] = ..., parameters: _Optional[_Iterable[_Union[SqlValue, _Mapping]]] = ..., collection: _Optional[str] = ..., limit: _Optional[int] = ..., offset: _Optional[int] = ...) -> None: ...

class ExecuteSqlResponse(_message.Message):
    __slots__ = ("rows", "rows_scanned", "rows_returned", "execution_time_ms", "columns", "column_types")
    ROWS_FIELD_NUMBER: _ClassVar[int]
    ROWS_SCANNED_FIELD_NUMBER: _ClassVar[int]
    ROWS_RETURNED_FIELD_NUMBER: _ClassVar[int]
    EXECUTION_TIME_MS_FIELD_NUMBER: _ClassVar[int]
    COLUMNS_FIELD_NUMBER: _ClassVar[int]
    COLUMN_TYPES_FIELD_NUMBER: _ClassVar[int]
    rows: _containers.RepeatedCompositeFieldContainer[SqlRow]
    rows_scanned: int
    rows_returned: int
    execution_time_ms: int
    columns: _containers.RepeatedScalarFieldContainer[str]
    column_types: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, rows: _Optional[_Iterable[_Union[SqlRow, _Mapping]]] = ..., rows_scanned: _Optional[int] = ..., rows_returned: _Optional[int] = ..., execution_time_ms: _Optional[int] = ..., columns: _Optional[_Iterable[str]] = ..., column_types: _Optional[_Iterable[str]] = ...) -> None: ...
