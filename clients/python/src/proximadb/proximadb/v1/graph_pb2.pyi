from proximadb.v1 import entity_pb2 as _entity_pb2
from proximadb.v1 import relations_pb2 as _relations_pb2
from proximadb.v1 import vector_pb2 as _vector_pb2
from proximadb.v1 import vector_types_pb2 as _vector_types_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class PropertyFilterOperator(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    PROPERTY_FILTER_OPERATOR_UNSPECIFIED: _ClassVar[PropertyFilterOperator]
    PROPERTY_FILTER_OPERATOR_EQUALS: _ClassVar[PropertyFilterOperator]
    PROPERTY_FILTER_OPERATOR_NOT_EQUALS: _ClassVar[PropertyFilterOperator]
    PROPERTY_FILTER_OPERATOR_GREATER_THAN: _ClassVar[PropertyFilterOperator]
    PROPERTY_FILTER_OPERATOR_LESS_THAN: _ClassVar[PropertyFilterOperator]
    PROPERTY_FILTER_OPERATOR_GREATER_EQUAL: _ClassVar[PropertyFilterOperator]
    PROPERTY_FILTER_OPERATOR_LESS_EQUAL: _ClassVar[PropertyFilterOperator]
    PROPERTY_FILTER_OPERATOR_CONTAINS: _ClassVar[PropertyFilterOperator]
    PROPERTY_FILTER_OPERATOR_STARTS_WITH: _ClassVar[PropertyFilterOperator]
    PROPERTY_FILTER_OPERATOR_ENDS_WITH: _ClassVar[PropertyFilterOperator]

class TraversalAlgorithm(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    TRAVERSAL_ALGORITHM_UNSPECIFIED: _ClassVar[TraversalAlgorithm]
    TRAVERSAL_ALGORITHM_BFS: _ClassVar[TraversalAlgorithm]
    TRAVERSAL_ALGORITHM_DFS: _ClassVar[TraversalAlgorithm]
    TRAVERSAL_ALGORITHM_PARALLEL_BFS: _ClassVar[TraversalAlgorithm]

class CombinationStrategy(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    COMBINATION_STRATEGY_UNSPECIFIED: _ClassVar[CombinationStrategy]
    COMBINATION_STRATEGY_VECTOR_THEN_GRAPH: _ClassVar[CombinationStrategy]
    COMBINATION_STRATEGY_GRAPH_THEN_VECTOR: _ClassVar[CombinationStrategy]
    COMBINATION_STRATEGY_BALANCED: _ClassVar[CombinationStrategy]

class ShortestPathAlgorithm(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    SHORTEST_PATH_ALGORITHM_UNSPECIFIED: _ClassVar[ShortestPathAlgorithm]
    SHORTEST_PATH_ALGORITHM_DIJKSTRA: _ClassVar[ShortestPathAlgorithm]
    SHORTEST_PATH_ALGORITHM_ASTAR: _ClassVar[ShortestPathAlgorithm]
PROPERTY_FILTER_OPERATOR_UNSPECIFIED: PropertyFilterOperator
PROPERTY_FILTER_OPERATOR_EQUALS: PropertyFilterOperator
PROPERTY_FILTER_OPERATOR_NOT_EQUALS: PropertyFilterOperator
PROPERTY_FILTER_OPERATOR_GREATER_THAN: PropertyFilterOperator
PROPERTY_FILTER_OPERATOR_LESS_THAN: PropertyFilterOperator
PROPERTY_FILTER_OPERATOR_GREATER_EQUAL: PropertyFilterOperator
PROPERTY_FILTER_OPERATOR_LESS_EQUAL: PropertyFilterOperator
PROPERTY_FILTER_OPERATOR_CONTAINS: PropertyFilterOperator
PROPERTY_FILTER_OPERATOR_STARTS_WITH: PropertyFilterOperator
PROPERTY_FILTER_OPERATOR_ENDS_WITH: PropertyFilterOperator
TRAVERSAL_ALGORITHM_UNSPECIFIED: TraversalAlgorithm
TRAVERSAL_ALGORITHM_BFS: TraversalAlgorithm
TRAVERSAL_ALGORITHM_DFS: TraversalAlgorithm
TRAVERSAL_ALGORITHM_PARALLEL_BFS: TraversalAlgorithm
COMBINATION_STRATEGY_UNSPECIFIED: CombinationStrategy
COMBINATION_STRATEGY_VECTOR_THEN_GRAPH: CombinationStrategy
COMBINATION_STRATEGY_GRAPH_THEN_VECTOR: CombinationStrategy
COMBINATION_STRATEGY_BALANCED: CombinationStrategy
SHORTEST_PATH_ALGORITHM_UNSPECIFIED: ShortestPathAlgorithm
SHORTEST_PATH_ALGORITHM_DIJKSTRA: ShortestPathAlgorithm
SHORTEST_PATH_ALGORITHM_ASTAR: ShortestPathAlgorithm

class Node(_message.Message):
    __slots__ = ("id", "labels", "properties", "embedding", "created_at_ms", "updated_at_ms")
    class PropertiesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: PropertyValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[PropertyValue, _Mapping]] = ...) -> None: ...
    ID_FIELD_NUMBER: _ClassVar[int]
    LABELS_FIELD_NUMBER: _ClassVar[int]
    PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    EMBEDDING_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_MS_FIELD_NUMBER: _ClassVar[int]
    UPDATED_AT_MS_FIELD_NUMBER: _ClassVar[int]
    id: str
    labels: _containers.RepeatedScalarFieldContainer[str]
    properties: _containers.MessageMap[str, PropertyValue]
    embedding: _entity_pb2.EmbeddingVersion
    created_at_ms: int
    updated_at_ms: int
    def __init__(self, id: _Optional[str] = ..., labels: _Optional[_Iterable[str]] = ..., properties: _Optional[_Mapping[str, PropertyValue]] = ..., embedding: _Optional[_Union[_entity_pb2.EmbeddingVersion, _Mapping]] = ..., created_at_ms: _Optional[int] = ..., updated_at_ms: _Optional[int] = ...) -> None: ...

class Edge(_message.Message):
    __slots__ = ("id", "from_node_id", "to_node_id", "edge_type", "properties", "weight", "created_at_ms", "updated_at_ms")
    class PropertiesEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: PropertyValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[PropertyValue, _Mapping]] = ...) -> None: ...
    ID_FIELD_NUMBER: _ClassVar[int]
    FROM_NODE_ID_FIELD_NUMBER: _ClassVar[int]
    TO_NODE_ID_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPE_FIELD_NUMBER: _ClassVar[int]
    PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    WEIGHT_FIELD_NUMBER: _ClassVar[int]
    CREATED_AT_MS_FIELD_NUMBER: _ClassVar[int]
    UPDATED_AT_MS_FIELD_NUMBER: _ClassVar[int]
    id: str
    from_node_id: str
    to_node_id: str
    edge_type: str
    properties: _containers.MessageMap[str, PropertyValue]
    weight: float
    created_at_ms: int
    updated_at_ms: int
    def __init__(self, id: _Optional[str] = ..., from_node_id: _Optional[str] = ..., to_node_id: _Optional[str] = ..., edge_type: _Optional[str] = ..., properties: _Optional[_Mapping[str, PropertyValue]] = ..., weight: _Optional[float] = ..., created_at_ms: _Optional[int] = ..., updated_at_ms: _Optional[int] = ...) -> None: ...

class PropertyValue(_message.Message):
    __slots__ = ("string_value", "int_value", "double_value", "bool_value", "bytes_value", "array_value", "object_value", "vector_value")
    STRING_VALUE_FIELD_NUMBER: _ClassVar[int]
    INT_VALUE_FIELD_NUMBER: _ClassVar[int]
    DOUBLE_VALUE_FIELD_NUMBER: _ClassVar[int]
    BOOL_VALUE_FIELD_NUMBER: _ClassVar[int]
    BYTES_VALUE_FIELD_NUMBER: _ClassVar[int]
    ARRAY_VALUE_FIELD_NUMBER: _ClassVar[int]
    OBJECT_VALUE_FIELD_NUMBER: _ClassVar[int]
    VECTOR_VALUE_FIELD_NUMBER: _ClassVar[int]
    string_value: str
    int_value: int
    double_value: float
    bool_value: bool
    bytes_value: bytes
    array_value: PropertyArray
    object_value: PropertyObject
    vector_value: _entity_pb2.VectorData
    def __init__(self, string_value: _Optional[str] = ..., int_value: _Optional[int] = ..., double_value: _Optional[float] = ..., bool_value: bool = ..., bytes_value: _Optional[bytes] = ..., array_value: _Optional[_Union[PropertyArray, _Mapping]] = ..., object_value: _Optional[_Union[PropertyObject, _Mapping]] = ..., vector_value: _Optional[_Union[_entity_pb2.VectorData, _Mapping]] = ...) -> None: ...

class PropertyArray(_message.Message):
    __slots__ = ("values",)
    VALUES_FIELD_NUMBER: _ClassVar[int]
    values: _containers.RepeatedCompositeFieldContainer[PropertyValue]
    def __init__(self, values: _Optional[_Iterable[_Union[PropertyValue, _Mapping]]] = ...) -> None: ...

class PropertyObject(_message.Message):
    __slots__ = ("fields",)
    class FieldsEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: PropertyValue
        def __init__(self, key: _Optional[str] = ..., value: _Optional[_Union[PropertyValue, _Mapping]] = ...) -> None: ...
    FIELDS_FIELD_NUMBER: _ClassVar[int]
    fields: _containers.MessageMap[str, PropertyValue]
    def __init__(self, fields: _Optional[_Mapping[str, PropertyValue]] = ...) -> None: ...

class TraversalRequest(_message.Message):
    __slots__ = ("graph_id", "start_node_id", "max_depth", "edge_types", "node_labels", "filters", "algorithm", "limit", "timeout_ms", "max_frontier")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    START_NODE_ID_FIELD_NUMBER: _ClassVar[int]
    MAX_DEPTH_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPES_FIELD_NUMBER: _ClassVar[int]
    NODE_LABELS_FIELD_NUMBER: _ClassVar[int]
    FILTERS_FIELD_NUMBER: _ClassVar[int]
    ALGORITHM_FIELD_NUMBER: _ClassVar[int]
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_MS_FIELD_NUMBER: _ClassVar[int]
    MAX_FRONTIER_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    start_node_id: str
    max_depth: int
    edge_types: _containers.RepeatedScalarFieldContainer[str]
    node_labels: _containers.RepeatedScalarFieldContainer[str]
    filters: _containers.RepeatedCompositeFieldContainer[PropertyFilter]
    algorithm: TraversalAlgorithm
    limit: int
    timeout_ms: int
    max_frontier: int
    def __init__(self, graph_id: _Optional[str] = ..., start_node_id: _Optional[str] = ..., max_depth: _Optional[int] = ..., edge_types: _Optional[_Iterable[str]] = ..., node_labels: _Optional[_Iterable[str]] = ..., filters: _Optional[_Iterable[_Union[PropertyFilter, _Mapping]]] = ..., algorithm: _Optional[_Union[TraversalAlgorithm, str]] = ..., limit: _Optional[int] = ..., timeout_ms: _Optional[int] = ..., max_frontier: _Optional[int] = ...) -> None: ...

class PropertyFilter(_message.Message):
    __slots__ = ("key", "operator", "value")
    KEY_FIELD_NUMBER: _ClassVar[int]
    OPERATOR_FIELD_NUMBER: _ClassVar[int]
    VALUE_FIELD_NUMBER: _ClassVar[int]
    key: str
    operator: PropertyFilterOperator
    value: PropertyValue
    def __init__(self, key: _Optional[str] = ..., operator: _Optional[_Union[PropertyFilterOperator, str]] = ..., value: _Optional[_Union[PropertyValue, _Mapping]] = ...) -> None: ...

class TraversalResponse(_message.Message):
    __slots__ = ("nodes", "edges", "paths", "stats")
    NODES_FIELD_NUMBER: _ClassVar[int]
    EDGES_FIELD_NUMBER: _ClassVar[int]
    PATHS_FIELD_NUMBER: _ClassVar[int]
    STATS_FIELD_NUMBER: _ClassVar[int]
    nodes: _containers.RepeatedCompositeFieldContainer[Node]
    edges: _containers.RepeatedCompositeFieldContainer[Edge]
    paths: _containers.RepeatedCompositeFieldContainer[_relations_pb2.GraphPath]
    stats: TraversalStats
    def __init__(self, nodes: _Optional[_Iterable[_Union[Node, _Mapping]]] = ..., edges: _Optional[_Iterable[_Union[Edge, _Mapping]]] = ..., paths: _Optional[_Iterable[_Union[_relations_pb2.GraphPath, _Mapping]]] = ..., stats: _Optional[_Union[TraversalStats, _Mapping]] = ...) -> None: ...

class TraversalStats(_message.Message):
    __slots__ = ("nodes_visited", "edges_traversed", "max_depth_reached", "execution_time_microseconds")
    NODES_VISITED_FIELD_NUMBER: _ClassVar[int]
    EDGES_TRAVERSED_FIELD_NUMBER: _ClassVar[int]
    MAX_DEPTH_REACHED_FIELD_NUMBER: _ClassVar[int]
    EXECUTION_TIME_MICROSECONDS_FIELD_NUMBER: _ClassVar[int]
    nodes_visited: int
    edges_traversed: int
    max_depth_reached: int
    execution_time_microseconds: int
    def __init__(self, nodes_visited: _Optional[int] = ..., edges_traversed: _Optional[int] = ..., max_depth_reached: _Optional[int] = ..., execution_time_microseconds: _Optional[int] = ...) -> None: ...

class NodeQuery(_message.Message):
    __slots__ = ("graph_id", "labels", "filters", "limit", "offset", "continuation_token")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    LABELS_FIELD_NUMBER: _ClassVar[int]
    FILTERS_FIELD_NUMBER: _ClassVar[int]
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    CONTINUATION_TOKEN_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    labels: _containers.RepeatedScalarFieldContainer[str]
    filters: _containers.RepeatedCompositeFieldContainer[PropertyFilter]
    limit: int
    offset: int
    continuation_token: str
    def __init__(self, graph_id: _Optional[str] = ..., labels: _Optional[_Iterable[str]] = ..., filters: _Optional[_Iterable[_Union[PropertyFilter, _Mapping]]] = ..., limit: _Optional[int] = ..., offset: _Optional[int] = ..., continuation_token: _Optional[str] = ...) -> None: ...

class EdgeQuery(_message.Message):
    __slots__ = ("graph_id", "from_node_id", "to_node_id", "edge_types", "filters", "limit", "offset", "continuation_token")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    FROM_NODE_ID_FIELD_NUMBER: _ClassVar[int]
    TO_NODE_ID_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPES_FIELD_NUMBER: _ClassVar[int]
    FILTERS_FIELD_NUMBER: _ClassVar[int]
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    CONTINUATION_TOKEN_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    from_node_id: str
    to_node_id: str
    edge_types: _containers.RepeatedScalarFieldContainer[str]
    filters: _containers.RepeatedCompositeFieldContainer[PropertyFilter]
    limit: int
    offset: int
    continuation_token: str
    def __init__(self, graph_id: _Optional[str] = ..., from_node_id: _Optional[str] = ..., to_node_id: _Optional[str] = ..., edge_types: _Optional[_Iterable[str]] = ..., filters: _Optional[_Iterable[_Union[PropertyFilter, _Mapping]]] = ..., limit: _Optional[int] = ..., offset: _Optional[int] = ..., continuation_token: _Optional[str] = ...) -> None: ...

class BatchNodeRequest(_message.Message):
    __slots__ = ("graph_id", "nodes")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    NODES_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    nodes: _containers.RepeatedCompositeFieldContainer[Node]
    def __init__(self, graph_id: _Optional[str] = ..., nodes: _Optional[_Iterable[_Union[Node, _Mapping]]] = ...) -> None: ...

class BatchEdgeRequest(_message.Message):
    __slots__ = ("graph_id", "edges")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    EDGES_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    edges: _containers.RepeatedCompositeFieldContainer[Edge]
    def __init__(self, graph_id: _Optional[str] = ..., edges: _Optional[_Iterable[_Union[Edge, _Mapping]]] = ...) -> None: ...

class BatchResponse(_message.Message):
    __slots__ = ("success", "nodes", "edges", "error_message", "next_token", "created_count", "updated_count", "failed_count", "failed_ids", "error_messages")
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    NODES_FIELD_NUMBER: _ClassVar[int]
    EDGES_FIELD_NUMBER: _ClassVar[int]
    ERROR_MESSAGE_FIELD_NUMBER: _ClassVar[int]
    NEXT_TOKEN_FIELD_NUMBER: _ClassVar[int]
    CREATED_COUNT_FIELD_NUMBER: _ClassVar[int]
    UPDATED_COUNT_FIELD_NUMBER: _ClassVar[int]
    FAILED_COUNT_FIELD_NUMBER: _ClassVar[int]
    FAILED_IDS_FIELD_NUMBER: _ClassVar[int]
    ERROR_MESSAGES_FIELD_NUMBER: _ClassVar[int]
    success: bool
    nodes: _containers.RepeatedCompositeFieldContainer[Node]
    edges: _containers.RepeatedCompositeFieldContainer[Edge]
    error_message: str
    next_token: str
    created_count: int
    updated_count: int
    failed_count: int
    failed_ids: _containers.RepeatedScalarFieldContainer[str]
    error_messages: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, success: bool = ..., nodes: _Optional[_Iterable[_Union[Node, _Mapping]]] = ..., edges: _Optional[_Iterable[_Union[Edge, _Mapping]]] = ..., error_message: _Optional[str] = ..., next_token: _Optional[str] = ..., created_count: _Optional[int] = ..., updated_count: _Optional[int] = ..., failed_count: _Optional[int] = ..., failed_ids: _Optional[_Iterable[str]] = ..., error_messages: _Optional[_Iterable[str]] = ...) -> None: ...

class HybridSearchRequest(_message.Message):
    __slots__ = ("vector_search_request", "graph_traversal_request", "combination_strategy", "limit", "offset")
    VECTOR_SEARCH_REQUEST_FIELD_NUMBER: _ClassVar[int]
    GRAPH_TRAVERSAL_REQUEST_FIELD_NUMBER: _ClassVar[int]
    COMBINATION_STRATEGY_FIELD_NUMBER: _ClassVar[int]
    LIMIT_FIELD_NUMBER: _ClassVar[int]
    OFFSET_FIELD_NUMBER: _ClassVar[int]
    vector_search_request: _vector_types_pb2.VectorSearchRequest
    graph_traversal_request: TraversalRequest
    combination_strategy: CombinationStrategy
    limit: int
    offset: int
    def __init__(self, vector_search_request: _Optional[_Union[_vector_types_pb2.VectorSearchRequest, _Mapping]] = ..., graph_traversal_request: _Optional[_Union[TraversalRequest, _Mapping]] = ..., combination_strategy: _Optional[_Union[CombinationStrategy, str]] = ..., limit: _Optional[int] = ..., offset: _Optional[int] = ...) -> None: ...

class HybridSearchResponse(_message.Message):
    __slots__ = ("nodes", "edges", "paths", "stats", "vector_results")
    NODES_FIELD_NUMBER: _ClassVar[int]
    EDGES_FIELD_NUMBER: _ClassVar[int]
    PATHS_FIELD_NUMBER: _ClassVar[int]
    STATS_FIELD_NUMBER: _ClassVar[int]
    VECTOR_RESULTS_FIELD_NUMBER: _ClassVar[int]
    nodes: _containers.RepeatedCompositeFieldContainer[Node]
    edges: _containers.RepeatedCompositeFieldContainer[Edge]
    paths: _containers.RepeatedCompositeFieldContainer[_relations_pb2.GraphPath]
    stats: HybridSearchStats
    vector_results: _containers.RepeatedCompositeFieldContainer[_vector_types_pb2.SearchVectorRecord]
    def __init__(self, nodes: _Optional[_Iterable[_Union[Node, _Mapping]]] = ..., edges: _Optional[_Iterable[_Union[Edge, _Mapping]]] = ..., paths: _Optional[_Iterable[_Union[_relations_pb2.GraphPath, _Mapping]]] = ..., stats: _Optional[_Union[HybridSearchStats, _Mapping]] = ..., vector_results: _Optional[_Iterable[_Union[_vector_types_pb2.SearchVectorRecord, _Mapping]]] = ...) -> None: ...

class HybridSearchStats(_message.Message):
    __slots__ = ("vector_results_count", "graph_traversal_count", "execution_time_microseconds")
    VECTOR_RESULTS_COUNT_FIELD_NUMBER: _ClassVar[int]
    GRAPH_TRAVERSAL_COUNT_FIELD_NUMBER: _ClassVar[int]
    EXECUTION_TIME_MICROSECONDS_FIELD_NUMBER: _ClassVar[int]
    vector_results_count: int
    graph_traversal_count: int
    execution_time_microseconds: int
    def __init__(self, vector_results_count: _Optional[int] = ..., graph_traversal_count: _Optional[int] = ..., execution_time_microseconds: _Optional[int] = ...) -> None: ...

class GraphStats(_message.Message):
    __slots__ = ("total_nodes", "total_edges", "label_stats", "edge_type_stats", "total_properties", "memory_usage_bytes", "average_degree", "max_degree", "connected_components")
    TOTAL_NODES_FIELD_NUMBER: _ClassVar[int]
    TOTAL_EDGES_FIELD_NUMBER: _ClassVar[int]
    LABEL_STATS_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPE_STATS_FIELD_NUMBER: _ClassVar[int]
    TOTAL_PROPERTIES_FIELD_NUMBER: _ClassVar[int]
    MEMORY_USAGE_BYTES_FIELD_NUMBER: _ClassVar[int]
    AVERAGE_DEGREE_FIELD_NUMBER: _ClassVar[int]
    MAX_DEGREE_FIELD_NUMBER: _ClassVar[int]
    CONNECTED_COMPONENTS_FIELD_NUMBER: _ClassVar[int]
    total_nodes: int
    total_edges: int
    label_stats: _containers.RepeatedCompositeFieldContainer[LabelStats]
    edge_type_stats: _containers.RepeatedCompositeFieldContainer[EdgeTypeStats]
    total_properties: int
    memory_usage_bytes: int
    average_degree: float
    max_degree: int
    connected_components: int
    def __init__(self, total_nodes: _Optional[int] = ..., total_edges: _Optional[int] = ..., label_stats: _Optional[_Iterable[_Union[LabelStats, _Mapping]]] = ..., edge_type_stats: _Optional[_Iterable[_Union[EdgeTypeStats, _Mapping]]] = ..., total_properties: _Optional[int] = ..., memory_usage_bytes: _Optional[int] = ..., average_degree: _Optional[float] = ..., max_degree: _Optional[int] = ..., connected_components: _Optional[int] = ...) -> None: ...

class LabelStats(_message.Message):
    __slots__ = ("label", "count")
    LABEL_FIELD_NUMBER: _ClassVar[int]
    COUNT_FIELD_NUMBER: _ClassVar[int]
    label: str
    count: int
    def __init__(self, label: _Optional[str] = ..., count: _Optional[int] = ...) -> None: ...

class EdgeTypeStats(_message.Message):
    __slots__ = ("edge_type", "count")
    EDGE_TYPE_FIELD_NUMBER: _ClassVar[int]
    COUNT_FIELD_NUMBER: _ClassVar[int]
    edge_type: str
    count: int
    def __init__(self, edge_type: _Optional[str] = ..., count: _Optional[int] = ...) -> None: ...

class CreateNodeRequest(_message.Message):
    __slots__ = ("graph_id", "node")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    NODE_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    node: Node
    def __init__(self, graph_id: _Optional[str] = ..., node: _Optional[_Union[Node, _Mapping]] = ...) -> None: ...

class GetNodeRequest(_message.Message):
    __slots__ = ("graph_id", "node_id")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    NODE_ID_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    node_id: str
    def __init__(self, graph_id: _Optional[str] = ..., node_id: _Optional[str] = ...) -> None: ...

class UpdateNodeRequest(_message.Message):
    __slots__ = ("graph_id", "node")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    NODE_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    node: Node
    def __init__(self, graph_id: _Optional[str] = ..., node: _Optional[_Union[Node, _Mapping]] = ...) -> None: ...

class DeleteNodeRequest(_message.Message):
    __slots__ = ("graph_id", "node_id")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    NODE_ID_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    node_id: str
    def __init__(self, graph_id: _Optional[str] = ..., node_id: _Optional[str] = ...) -> None: ...

class CreateEdgeRequest(_message.Message):
    __slots__ = ("graph_id", "edge")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    EDGE_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    edge: Edge
    def __init__(self, graph_id: _Optional[str] = ..., edge: _Optional[_Union[Edge, _Mapping]] = ...) -> None: ...

class GetEdgeRequest(_message.Message):
    __slots__ = ("graph_id", "edge_id")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    EDGE_ID_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    edge_id: str
    def __init__(self, graph_id: _Optional[str] = ..., edge_id: _Optional[str] = ...) -> None: ...

class UpdateEdgeRequest(_message.Message):
    __slots__ = ("graph_id", "edge")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    EDGE_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    edge: Edge
    def __init__(self, graph_id: _Optional[str] = ..., edge: _Optional[_Union[Edge, _Mapping]] = ...) -> None: ...

class DeleteEdgeRequest(_message.Message):
    __slots__ = ("graph_id", "edge_id")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    EDGE_ID_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    edge_id: str
    def __init__(self, graph_id: _Optional[str] = ..., edge_id: _Optional[str] = ...) -> None: ...

class GetNeighborsRequest(_message.Message):
    __slots__ = ("graph_id", "node_id", "edge_type")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    NODE_ID_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPE_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    node_id: str
    edge_type: str
    def __init__(self, graph_id: _Optional[str] = ..., node_id: _Optional[str] = ..., edge_type: _Optional[str] = ...) -> None: ...

class GetStatsRequest(_message.Message):
    __slots__ = ("graph_id",)
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    def __init__(self, graph_id: _Optional[str] = ...) -> None: ...

class ShortestPathRequest(_message.Message):
    __slots__ = ("graph_id", "start_node_id", "target_node_id", "max_depth", "edge_types", "algorithm", "k")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    START_NODE_ID_FIELD_NUMBER: _ClassVar[int]
    TARGET_NODE_ID_FIELD_NUMBER: _ClassVar[int]
    MAX_DEPTH_FIELD_NUMBER: _ClassVar[int]
    EDGE_TYPES_FIELD_NUMBER: _ClassVar[int]
    ALGORITHM_FIELD_NUMBER: _ClassVar[int]
    K_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    start_node_id: str
    target_node_id: str
    max_depth: int
    edge_types: _containers.RepeatedScalarFieldContainer[str]
    algorithm: ShortestPathAlgorithm
    k: int
    def __init__(self, graph_id: _Optional[str] = ..., start_node_id: _Optional[str] = ..., target_node_id: _Optional[str] = ..., max_depth: _Optional[int] = ..., edge_types: _Optional[_Iterable[str]] = ..., algorithm: _Optional[_Union[ShortestPathAlgorithm, str]] = ..., k: _Optional[int] = ...) -> None: ...

class ShortestPathResponse(_message.Message):
    __slots__ = ("node_ids", "total_weight")
    NODE_IDS_FIELD_NUMBER: _ClassVar[int]
    TOTAL_WEIGHT_FIELD_NUMBER: _ClassVar[int]
    node_ids: _containers.RepeatedScalarFieldContainer[str]
    total_weight: float
    def __init__(self, node_ids: _Optional[_Iterable[str]] = ..., total_weight: _Optional[float] = ...) -> None: ...

class TraversalChunk(_message.Message):
    __slots__ = ("nodes", "edges", "paths", "stats", "done")
    NODES_FIELD_NUMBER: _ClassVar[int]
    EDGES_FIELD_NUMBER: _ClassVar[int]
    PATHS_FIELD_NUMBER: _ClassVar[int]
    STATS_FIELD_NUMBER: _ClassVar[int]
    DONE_FIELD_NUMBER: _ClassVar[int]
    nodes: _containers.RepeatedCompositeFieldContainer[Node]
    edges: _containers.RepeatedCompositeFieldContainer[Edge]
    paths: _containers.RepeatedCompositeFieldContainer[_relations_pb2.GraphPath]
    stats: TraversalStats
    done: bool
    def __init__(self, nodes: _Optional[_Iterable[_Union[Node, _Mapping]]] = ..., edges: _Optional[_Iterable[_Union[Edge, _Mapping]]] = ..., paths: _Optional[_Iterable[_Union[_relations_pb2.GraphPath, _Mapping]]] = ..., stats: _Optional[_Union[TraversalStats, _Mapping]] = ..., done: bool = ...) -> None: ...

class UniqueConstraintRequest(_message.Message):
    __slots__ = ("graph_id", "label", "property")
    GRAPH_ID_FIELD_NUMBER: _ClassVar[int]
    LABEL_FIELD_NUMBER: _ClassVar[int]
    PROPERTY_FIELD_NUMBER: _ClassVar[int]
    graph_id: str
    label: str
    property: str
    def __init__(self, graph_id: _Optional[str] = ..., label: _Optional[str] = ..., property: _Optional[str] = ...) -> None: ...

class UniqueConstraintResponse(_message.Message):
    __slots__ = ("success", "error_message")
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    ERROR_MESSAGE_FIELD_NUMBER: _ClassVar[int]
    success: bool
    error_message: str
    def __init__(self, success: bool = ..., error_message: _Optional[str] = ...) -> None: ...

class ConnectedComponentsResponse(_message.Message):
    __slots__ = ("components",)
    COMPONENTS_FIELD_NUMBER: _ClassVar[int]
    components: _containers.RepeatedCompositeFieldContainer[Component]
    def __init__(self, components: _Optional[_Iterable[_Union[Component, _Mapping]]] = ...) -> None: ...

class Component(_message.Message):
    __slots__ = ("node_ids",)
    NODE_IDS_FIELD_NUMBER: _ClassVar[int]
    node_ids: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, node_ids: _Optional[_Iterable[str]] = ...) -> None: ...

class CycleCheckResponse(_message.Message):
    __slots__ = ("has_cycle",)
    HAS_CYCLE_FIELD_NUMBER: _ClassVar[int]
    has_cycle: bool
    def __init__(self, has_cycle: bool = ...) -> None: ...
