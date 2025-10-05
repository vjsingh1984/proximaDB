from proximadb.v1 import entity_pb2 as _entity_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class AssembleDocumentRequest(_message.Message):
    __slots__ = ("collection_id", "source_id")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    SOURCE_ID_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    source_id: str
    def __init__(self, collection_id: _Optional[str] = ..., source_id: _Optional[str] = ...) -> None: ...

class DocumentSegment(_message.Message):
    __slots__ = ("text", "provenance", "order")
    TEXT_FIELD_NUMBER: _ClassVar[int]
    PROVENANCE_FIELD_NUMBER: _ClassVar[int]
    ORDER_FIELD_NUMBER: _ClassVar[int]
    text: str
    provenance: _entity_pb2.Provenance
    order: int
    def __init__(self, text: _Optional[str] = ..., provenance: _Optional[_Union[_entity_pb2.Provenance, _Mapping]] = ..., order: _Optional[int] = ...) -> None: ...

class AssembleDocumentResponse(_message.Message):
    __slots__ = ("source_id", "segments")
    SOURCE_ID_FIELD_NUMBER: _ClassVar[int]
    SEGMENTS_FIELD_NUMBER: _ClassVar[int]
    source_id: str
    segments: _containers.RepeatedCompositeFieldContainer[DocumentSegment]
    def __init__(self, source_id: _Optional[str] = ..., segments: _Optional[_Iterable[_Union[DocumentSegment, _Mapping]]] = ...) -> None: ...

class AssembleContextRequest(_message.Message):
    __slots__ = ("collection_id", "entity_ids", "radius")
    COLLECTION_ID_FIELD_NUMBER: _ClassVar[int]
    ENTITY_IDS_FIELD_NUMBER: _ClassVar[int]
    RADIUS_FIELD_NUMBER: _ClassVar[int]
    collection_id: str
    entity_ids: _containers.RepeatedScalarFieldContainer[str]
    radius: int
    def __init__(self, collection_id: _Optional[str] = ..., entity_ids: _Optional[_Iterable[str]] = ..., radius: _Optional[int] = ...) -> None: ...

class ContextSegment(_message.Message):
    __slots__ = ("text", "provenance", "score")
    TEXT_FIELD_NUMBER: _ClassVar[int]
    PROVENANCE_FIELD_NUMBER: _ClassVar[int]
    SCORE_FIELD_NUMBER: _ClassVar[int]
    text: str
    provenance: _entity_pb2.Provenance
    score: float
    def __init__(self, text: _Optional[str] = ..., provenance: _Optional[_Union[_entity_pb2.Provenance, _Mapping]] = ..., score: _Optional[float] = ...) -> None: ...

class AssembleContextResponse(_message.Message):
    __slots__ = ("segments",)
    SEGMENTS_FIELD_NUMBER: _ClassVar[int]
    segments: _containers.RepeatedCompositeFieldContainer[ContextSegment]
    def __init__(self, segments: _Optional[_Iterable[_Union[ContextSegment, _Mapping]]] = ...) -> None: ...
