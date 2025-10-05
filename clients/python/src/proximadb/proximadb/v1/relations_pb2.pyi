from google.protobuf import empty_pb2 as _empty_pb2
from proximadb.v1 import entity_pb2 as _entity_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class CreateRelationRequest(_message.Message):
    __slots__ = ("collection_id", "relation")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    RELATION_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    relation: _entity_pb2.Relation
    def __init__(self, collection_id: _Optional[str] = ..., relation: _Optional[_Union[_entity_pb2.Relation, _Mapping]] = ...) -> None: ...

class DeleteRelationRequest(_message.Message):
    __slots__ = ("collection_id", "source_entity_id", "target_entity_id", "relation_type")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    SOURCE_ENTITY_ID_FIELD_NUMBER: _ClassVar[int]
    TARGET_ENTITY_ID_FIELD_NUMBER: _ClassVar[int]
    RELATION_TYPE_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    source_entity_id: str
    target_entity_id: str
    relation_type: str
    def __init__(self, collection_id: _Optional[str] = ..., source_entity_id: _Optional[str] = ..., target_entity_id: _Optional[str] = ..., relation_type: _Optional[str] = ...) -> None: ...

class ListRelationsRequest(_message.Message):
    __slots__ = ("collection_id", "entity_id")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    ENTITY_ID_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    entity_id: str
    def __init__(self, collection_id: _Optional[str] = ..., entity_id: _Optional[str] = ...) -> None: ...

class ListRelationsResponse(_message.Message):
    __slots__ = ("relations",)
    RELATIONS_FIELD_NUMBER: _ClassVar[int]
    relations: _containers.RepeatedCompositeFieldContainer[_entity_pb2.Relation]
    def __init__(self, relations: _Optional[_Iterable[_Union[_entity_pb2.Relation, _Mapping]]] = ...) -> None: ...

class GraphPath(_message.Message):
    __slots__ = ("entities", "relations")
    ENTITIES_FIELD_NUMBER: _ClassVar[int]
    RELATIONS_FIELD_NUMBER: _ClassVar[int]
    entities: _containers.RepeatedCompositeFieldContainer[_entity_pb2.Entity]
    relations: _containers.RepeatedCompositeFieldContainer[_entity_pb2.Relation]
    def __init__(self, entities: _Optional[_Iterable[_Union[_entity_pb2.Entity, _Mapping]]] = ..., relations: _Optional[_Iterable[_Union[_entity_pb2.Relation, _Mapping]]] = ...) -> None: ...

class TraverseRequest(_message.Message):
    __slots__ = ("collection_id", "start_entity_id", "max_depth")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    START_ENTITY_ID_FIELD_NUMBER: _ClassVar[int]
    MAX_DEPTH_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    start_entity_id: str
    max_depth: int
    def __init__(self, collection_id: _Optional[str] = ..., start_entity_id: _Optional[str] = ..., max_depth: _Optional[int] = ...) -> None: ...

class TraverseResponse(_message.Message):
    __slots__ = ("paths",)
    PATHS_FIELD_NUMBER: _ClassVar[int]
    paths: _containers.RepeatedCompositeFieldContainer[GraphPath]
    def __init__(self, paths: _Optional[_Iterable[_Union[GraphPath, _Mapping]]] = ...) -> None: ...
