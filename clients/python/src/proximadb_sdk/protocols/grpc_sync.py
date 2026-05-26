"""
ProximaDB Synchronous gRPC Client Wrapper

Provides a synchronous interface with connection pooling for optimal performance.
Features:
- Load-balanced gRPC connection pool (15-25% throughput improvement)
- Automatic channel health monitoring
- Thread-safe concurrent operations
"""

import logging
import json
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Union

from ..exceptions import ProximaDBError
from ..models import (
    BatchResult,
    OperationMetrics,
    SearchResult,
    VectorOperationResponse,
)
from ..models_v2 import ProximaRecord
from .connection_pools import GrpcChannelContext, GrpcConnectionPool


@dataclass
class HealthCheckResponse:
    """Health check response with server status"""

    healthy: bool
    latency_ms: float
    status: str
    server_address: str
    details: Optional[str] = None
    version: Optional[str] = None


@dataclass
class DeleteCollectionResponse:
    """Delete collection response"""

    success: bool
    collection_id: str
    status: str = "deleted"


class CollectionWrapper:
    """
    Wrapper for protobuf Collection objects to provide convenient attribute access.

    Protobuf Collection objects have the structure:
    - collection.id (collection ID)
    - collection.config.name (collection name)
    - collection.config.dimension (vector dimension)

    This wrapper provides backward-compatible access via:
    - .name (maps to .config.name)
    - .dimension (maps to .config.dimension)
    - .id (maps to .id)
    - All other attributes are passed through to the underlying protobuf object
    """

    def __init__(self, proto_collection):
        """Initialize wrapper with a protobuf Collection object"""
        self._proto = proto_collection

    @property
    def name(self):
        """Get collection name from config.name"""
        if hasattr(self._proto, "config") and hasattr(self._proto.config, "name"):
            return self._proto.config.name
        return None

    @property
    def dimension(self):
        """Get collection dimension from config.dimension"""
        if hasattr(self._proto, "config") and hasattr(self._proto.config, "dimension"):
            return self._proto.config.dimension
        return None

    @property
    def id(self):
        """Get collection ID"""
        return getattr(self._proto, "id", None)

    @property
    def config(self):
        """Get collection config"""
        return getattr(self._proto, "config", None)

    @property
    def stats(self):
        """Get collection stats if available"""
        return getattr(self._proto, "stats", None)

    def __getattr__(self, name):
        """Pass through any other attribute access to the underlying protobuf object"""
        return getattr(self._proto, name)

    def __repr__(self):
        return f"CollectionWrapper(name={self.name}, id={self.id}, dimension={self.dimension})"


class SearchResultsWrapper:
    """
    Wrapper for search results to provide backward-compatible .results attribute.

    Wraps a list of SearchResult objects to provide:
    - .results attribute (the list itself)
    - Direct list access via indexing, iteration, len()
    """

    def __init__(self, results_list: List[Any]):
        """Initialize wrapper with a list of SearchResult objects"""
        self.results = results_list

    def __len__(self):
        """Support len() operation"""
        return len(self.results)

    def __iter__(self):
        """Support iteration"""
        return iter(self.results)

    def __getitem__(self, index):
        """Support indexing"""
        return self.results[index]

    def __repr__(self):
        return f"SearchResultsWrapper(count={len(self.results)})"


class VectorWrapper:
    """
    Wrapper for vector dict to provide attribute access.

    Converts a dict like {'id': ..., 'vector': ..., 'metadata': ...}
    to an object with attribute access: obj.id, obj.vector, obj.metadata
    """

    def __init__(self, vector_dict: Dict[str, Any]):
        """Initialize wrapper with a vector dictionary"""
        self._dict = vector_dict
        # Set attributes from dict
        for key, value in vector_dict.items():
            setattr(self, key, value)

    def __getitem__(self, key):
        """Support dict-like access"""
        return self._dict[key]

    def get(self, key, default=None):
        """Support dict.get() method"""
        return self._dict.get(key, default)

    def __repr__(self):
        return f"VectorWrapper(id={getattr(self, 'id', None)})"


class DictWrapper:
    """
    Generic wrapper to convert any dict to an object with attribute access.

    Used for operation results like delete, update, etc.
    """

    def __init__(self, data_dict: Dict[str, Any]):
        """Initialize wrapper with a dictionary"""
        self._dict = data_dict
        # Set attributes from dict
        for key, value in data_dict.items():
            setattr(self, key, value)

    def __getitem__(self, key):
        """Support dict-like access"""
        return self._dict[key]

    def get(self, key, default=None):
        """Support dict.get() method"""
        return self._dict.get(key, default)

    def __repr__(self):
        return f"DictWrapper({self._dict})"


try:
    import grpc

    from proximadb_sdk.v1 import (
        collection_pb2_grpc as v1_collection_pb2_grpc,  # type: ignore
    )
    from proximadb_sdk.v1 import (
        collection_types_pb2 as v1_collection_types_pb2,  # type: ignore
    )
    from proximadb_sdk.v1 import sql_pb2_grpc as v1_sql_pb2_grpc  # type: ignore
    from proximadb_sdk.v1 import types_pb2 as v1_types_pb2  # type: ignore
    from proximadb_sdk.v1 import vector_pb2_grpc as v1_vector_pb2_grpc  # type: ignore
    from proximadb_sdk.v1 import vector_types_pb2 as v1_vector_types_pb2  # type: ignore

    # Optional graph service (generated via Makefile: gen-proto)
    try:
        from proximadb_sdk.v1 import graph_pb2 as v1_graph_pb2  # type: ignore
        from proximadb_sdk.v1 import graph_pb2_grpc as v1_graph_pb2_grpc  # type: ignore
    except Exception:  # pragma: no cover - optional
        v1_graph_pb2_grpc = None
        v1_graph_pb2 = None
    try:
        from proximadb.v2 import record_pb2 as v2_record_pb2  # type: ignore
        from proximadb.v2 import record_pb2_grpc as v2_record_pb2_grpc  # type: ignore
    except Exception:  # pragma: no cover - broken generated-stub install
        v2_record_pb2 = None
        v2_record_pb2_grpc = None
    GRPC_AVAILABLE = True
except ImportError:
    GRPC_AVAILABLE = False
    v2_record_pb2 = None
    v2_record_pb2_grpc = None

logger = logging.getLogger(__name__)


class ProximaDBSyncGrpcClient:
    """
    High-performance synchronous gRPC client with connection pooling

    Features:
    - Connection pool with 5 channels for load balancing
    - Automatic health monitoring and failover
    - 15-25% throughput improvement over single-channel approach
    """

    def __init__(
        self,
        server_address: str,
        timeout: float = 60.0,
        enable_compression: bool = False,  # Disabled by default - server doesn't support gzip yet
        compression_algorithm: str = "gzip",
        pool_size: int = 5,
        max_message_size: int = 64 * 1024 * 1024,
    ):
        """Initialize sync gRPC client with connection pool

        Args:
            server_address: gRPC server address. Use "localhost:5678" for unified port mode
                           (recommended) or "localhost:5679" for legacy multi-port mode.
            timeout: Request timeout in seconds
            enable_compression: Enable gRPC compression (default: False - server requires config)
            compression_algorithm: Compression algorithm ('gzip', default: 'gzip')
            pool_size: Number of gRPC channels in pool (default: 5)
            max_message_size: Maximum message size in bytes (default: 64MB)
        """
        self.server_address = server_address
        self.timeout = timeout
        self.enable_compression = enable_compression
        self.compression_algorithm = compression_algorithm.lower()
        self.pool_size = pool_size
        self.max_message_size = max_message_size

        # Initialize connection pool instead of single client
        self._connection_pool = None
        self._init_connection_pool()

        # Alias for backward compatibility with tests
        self._pool = self._connection_pool

    def _python_to_sql_value(self, value: Any):
        """Encode Python values into v1 SqlValue without losing nested shape."""
        from google.protobuf.struct_pb2 import NullValue

        sv = v1_types_pb2.SqlValue()
        if value is None:
            sv.null_value = NullValue.NULL_VALUE
        elif isinstance(value, bool):
            sv.bool_value = value
        elif isinstance(value, int) and not isinstance(value, bool):
            sv.int64_value = value
        elif isinstance(value, float):
            sv.number_value = value
        elif isinstance(value, (bytes, bytearray, memoryview)):
            sv.bytes_value = bytes(value)
        elif isinstance(value, (list, tuple)):
            sv.array_value.values.extend(
                self._python_to_sql_value(item) for item in value
            )
        elif isinstance(value, dict):
            for key, item in value.items():
                sv.object_value.fields[str(key)].CopyFrom(
                    self._python_to_sql_value(item)
                )
        else:
            sv.string_value = str(value)
        return sv

    def _sql_value_to_python(self, value) -> Any:
        """Decode v1 SqlValue rows into native Python values recursively."""
        kind = value.WhichOneof("value")
        if kind == "string_value":
            return value.string_value
        if kind == "number_value":
            return value.number_value
        if kind == "bool_value":
            return value.bool_value
        if kind == "int64_value":
            return value.int64_value
        if kind == "bytes_value":
            return bytes(value.bytes_value)
        if kind == "array_value":
            return [
                self._sql_value_to_python(item) for item in value.array_value.values
            ]
        if kind == "object_value":
            return {
                key: self._sql_value_to_python(item)
                for key, item in value.object_value.fields.items()
            }
        return None

    def _init_connection_pool(self):
        """Initialize gRPC connection pool for optimal performance"""
        try:
            import grpc

            # Map compression algorithm
            compression = None
            if self.enable_compression:
                if self.compression_algorithm == "gzip":
                    compression = grpc.Compression.Gzip
                elif self.compression_algorithm == "deflate":
                    compression = grpc.Compression.Deflate
                else:
                    logger.warning(
                        f"Unknown compression algorithm: {self.compression_algorithm}, using gzip"
                    )
                    compression = grpc.Compression.Gzip

            self._connection_pool = GrpcConnectionPool(
                endpoint=self.server_address,
                pool_size=self.pool_size,
                max_message_size=self.max_message_size,
                use_tls=False,  # TLS configuration can be added via environment variables or config
                compression=compression,
            )

            # Update alias for backward compatibility
            self._pool = self._connection_pool

            logger.info(
                f"Initialized gRPC connection pool: {self.pool_size} channels to {self.server_address}"
            )

        except Exception as e:
            logger.error(f"Failed to initialize gRPC connection pool: {e}")
            raise ProximaDBError(f"gRPC connection pool initialization failed: {e}")

    def _execute_with_pool(self, operation_name: str, operation_func):
        """Execute operation using connection pool with automatic error handling"""
        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )

        try:
            with GrpcChannelContext(self._connection_pool) as channel:
                # Create stub for this operation
                # Use versioned VectorService exclusively (v1)
                stub = v1_vector_pb2_grpc.VectorServiceStub(channel)

                # Execute the operation with timeout
                return operation_func(stub)

        except grpc.RpcError as e:
            logger.error(f"gRPC {operation_name} RPC error: {e.code()} - {e.details()}")
            details = e.details() or str(e)
            if e.code() == grpc.StatusCode.UNAVAILABLE or "connect" in details.lower():
                raise ProximaDBError(f"{operation_name} connection failed: {details}")
            raise ProximaDBError(f"{operation_name} RPC failed: {details}")
        except Exception as e:
            logger.error(f"gRPC {operation_name} failed: {e}")
            raise ProximaDBError(f"{operation_name} failed: {e}")

    def _execute_collection_with_pool(self, operation_name: str, operation_func):
        """Execute collection operation using connection pool with CollectionService"""
        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )

        try:
            with GrpcChannelContext(self._connection_pool) as channel:
                # Create CollectionService stub for collection operations
                stub = v1_collection_pb2_grpc.CollectionServiceStub(channel)

                # Execute the operation with timeout
                return operation_func(stub)

        except grpc.RpcError as e:
            logger.error(f"gRPC {operation_name} RPC error: {e.code()} - {e.details()}")
            details = e.details() or str(e)
            if e.code() == grpc.StatusCode.UNAVAILABLE or "connect" in details.lower():
                raise ProximaDBError(f"{operation_name} connection failed: {details}")
            raise ProximaDBError(f"{operation_name} RPC failed: {details}")
        except Exception as e:
            logger.error(f"gRPC {operation_name} failed: {e}")
            raise ProximaDBError(f"{operation_name} failed: {e}")

    def _execute_record_with_pool(self, operation_name: str, operation_func):
        """Execute record operation using the v2 ProximaRecordService."""
        if not GRPC_AVAILABLE or v2_record_pb2_grpc is None:
            raise ProximaDBError(
                "v2 record gRPC stubs not available. Regenerate Python protobuf stubs."
            )

        try:
            with GrpcChannelContext(self._connection_pool) as channel:
                stub = v2_record_pb2_grpc.ProximaRecordServiceStub(channel)
                return operation_func(stub)

        except grpc.RpcError as e:
            logger.error(f"gRPC {operation_name} RPC error: {e.code()} - {e.details()}")
            details = e.details() or str(e)
            if e.code() == grpc.StatusCode.UNAVAILABLE or "connect" in details.lower():
                raise ProximaDBError(f"{operation_name} connection failed: {details}")
            raise ProximaDBError(f"{operation_name} RPC failed: {details}")
        except Exception as e:
            logger.error(f"gRPC {operation_name} failed: {e}")
            raise ProximaDBError(f"{operation_name} failed: {e}")

    def get_pool_metrics(self):
        """Get connection pool performance metrics"""
        if self._connection_pool:
            return self._connection_pool.get_metrics()
        return None

    def close(self):
        """Close the connection pool and cleanup"""
        if self._connection_pool:
            try:
                self._connection_pool.close()
                logger.info("gRPC connection pool closed")
            except Exception as e:
                logger.warning(f"Error closing gRPC connection pool: {e}")

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()

    def health_check(self) -> HealthCheckResponse:
        """
        Check server health via gRPC health check

        Returns:
            HealthCheckResponse with:
                - healthy: bool
                - latency_ms: float
                - status: str
                - server_address: str
                - details: Optional[str]
                - version: Optional[str]
        """
        import time

        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )

        try:
            start_time = time.time()

            # Use list_collections as a lightweight health check
            # This verifies the server is responding and the connection pool works
            with GrpcChannelContext(self._connection_pool) as channel:
                stub = v1_collection_pb2_grpc.CollectionServiceStub(channel)

                # Make a lightweight request
                req = v1_collection_types_pb2.ListCollectionsRequest(limit=1)
                response = stub.ListCollections(req, timeout=self.timeout)

                latency_ms = (time.time() - start_time) * 1000

                return HealthCheckResponse(
                    healthy=True,
                    latency_ms=latency_ms,
                    status="connected",
                    server_address=self.server_address,
                )

        except grpc.RpcError as e:
            return HealthCheckResponse(
                healthy=False,
                latency_ms=-1,
                status=f"error: {e.code()}",
                server_address=self.server_address,
                details=e.details(),
            )
        except Exception as e:
            return HealthCheckResponse(
                healthy=False,
                latency_ms=-1,
                status=f"error: {type(e).__name__}",
                server_address=self.server_address,
                details=str(e),
            )

    # Health check via REST endpoint (gRPC doesn't have dedicated Health service in v1)

    # Graph (v1) — optional
    def shortest_path(
        self,
        start_node_id: str,
        target_node_id: str,
        max_depth: Optional[int] = None,
        edge_types: Optional[List[str]] = None,
        algorithm: str = "DIJKSTRA",
        k: Optional[int] = None,
        enable_prefetch: Optional[bool] = None,
        prefetch_budget: Optional[int] = None,
    ) -> Dict[str, Any]:
        """Compute shortest path via GraphService.ShortestPath with per-call prefetch overrides.

        Per-call overrides are passed as gRPC metadata headers:
        - x-graph-prefetch-enabled: true|false|1|0
        - x-graph-prefetch-budget: <int>
        """
        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )
        if v1_graph_pb2_grpc is None or v1_graph_pb2 is None:
            raise ProximaDBError(
                "GraphService stubs not found. Run: make -C clients/python gen-proto"
            )

        def _op(channel):
            stub = v1_graph_pb2_grpc.GraphServiceStub(channel)
            algo_enum = {
                "DIJKSTRA": v1_graph_pb2.ShortestPathAlgorithm.SHORTEST_PATH_ALGORITHM_DIJKSTRA,
                "ASTAR": v1_graph_pb2.ShortestPathAlgorithm.SHORTEST_PATH_ALGORITHM_ASTAR,
            }.get(
                algorithm.upper(),
                v1_graph_pb2.ShortestPathAlgorithm.SHORTEST_PATH_ALGORITHM_DIJKSTRA,
            )

            req = v1_graph_pb2.ShortestPathRequest(
                start_node_id=start_node_id,
                target_node_id=target_node_id,
                max_depth=max_depth or 0,
                edge_types=edge_types or [],
                algorithm=algo_enum,
                k=k or 0,
            )

            metadata = []
            if enable_prefetch is not None:
                metadata.append(
                    ("x-graph-prefetch-enabled", "true" if enable_prefetch else "false")
                )
            if prefetch_budget is not None:
                metadata.append(("x-graph-prefetch-budget", str(prefetch_budget)))

            # Use unary_unary with metadata support
            return stub.ShortestPath(req, timeout=self.timeout, metadata=metadata)

        return self._execute_with_pool("shortest_path", _op)

    # SQL (v1)
    def execute_sql(
        self,
        query: str,
        parameters: Optional[list] = None,
        collection: Optional[str] = None,
    ):
        """Execute SQL via proximadb.v1.SqlService.ExecuteSql

        Args:
            query: SQL text
            parameters: Optional list of rich values (scalars, bytes, lists, dicts)
            collection: Optional default collection context
        Returns:
            ExecuteSqlResponse as dict-like (via proto object fields)
        """
        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )

        try:
            with GrpcChannelContext(self._connection_pool) as channel:
                stub = v1_sql_pb2_grpc.SqlServiceStub(channel)
                # Build ExecuteSqlRequest using v1 messages
                from proximadb_sdk.v1 import types_pb2 as v1_types_pb2  # type: ignore

                req = v1_types_pb2.ExecuteSqlRequest(query=query)
                if parameters:
                    for p in parameters:
                        req.parameters.append(self._python_to_sql_value(p))
                if collection:
                    req.collection = collection
                resp = stub.ExecuteSql(req, timeout=self.timeout)
                # Return as a simple dict for convenience
                rows = [
                    {f.key: self._sql_value_to_python(f.value) for f in row.fields}
                    for row in resp.rows
                ]
                return {
                    "rows": rows,
                    "row_count": len(rows),  # Add row_count for compatibility
                    "rows_scanned": resp.rows_scanned,
                    "rows_returned": resp.rows_returned,
                    "execution_time_ms": resp.execution_time_ms,
                    "columns": list(resp.columns),
                    "column_types": list(resp.column_types),
                }
        except grpc.RpcError as e:
            logger.error(f"gRPC execute_sql RPC error: {e.code()} - {e.details()}")
            raise ProximaDBError(f"execute_sql RPC failed: {e.details()}")
        except Exception as e:
            logger.error(f"gRPC execute_sql failed: {e}")
            raise ProximaDBError(f"execute_sql failed: {e}")

    # Collections (v1)
    def create_collection_v1(
        self,
        name: str,
        dimension: int,
        distance_metric: int,
        storage_engine: int,
        tags: Optional[list] = None,
        description: Optional[str] = None,
    ):
        def _op(channel):
            stub = v1_collection_pb2_grpc.CollectionServiceStub(channel)
            cfg = v1_collection_types_pb2.CollectionConfig(
                name=name,
                dimension=dimension,
                distance_metric=distance_metric,
                storage_engine=storage_engine,
                tags=tags or [],
                description=description or "",
            )
            return stub.CreateCollection(cfg, timeout=self.timeout)

        return self._execute_with_pool("create_collection_v1", _op)

    def get_collection_v1(self, collection_id: str):
        def _op(channel):
            stub = v1_collection_pb2_grpc.CollectionServiceStub(channel)
            req = v1_collection_types_pb2.GetCollectionRequest(
                collection_id=collection_id
            )
            return stub.GetCollection(req, timeout=self.timeout)

        return self._execute_with_pool("get_collection_v1", _op)

    def list_collections_v1(
        self,
        limit: Optional[int] = None,
        offset: Optional[int] = None,
        include_stats: Optional[bool] = None,
    ):
        def _op(channel):
            stub = v1_collection_pb2_grpc.CollectionServiceStub(channel)
            req = v1_collection_types_pb2.ListCollectionsRequest(
                limit=limit or 0,
                offset=offset or 0,
                include_stats=include_stats or False,
            )
            return stub.ListCollections(req, timeout=self.timeout)

        return self._execute_with_pool("list_collections_v1", _op)

    def delete_collection_v1(self, collection_id: str):
        def _op(channel):
            stub = v1_collection_pb2_grpc.CollectionServiceStub(channel)
            req = v1_collection_types_pb2.DeleteCollectionRequest(
                collection_id=collection_id
            )
            return stub.DeleteCollection(req, timeout=self.timeout)

        return self._execute_with_pool("delete_collection_v1", _op)

    # Collection Operations - Unified Interface
    def create_collection(
        self,
        name: str,
        dimension: int,
        distance_metric: int = None,
        indexing_algorithm: int = None,
        storage_engine: int = None,
        engine: int = None,  # Alias for storage_engine (backward compatibility)
        filterable_columns: List[Any] = None,
        index_configs: List[Any] = None,
        quantization_config: Any = None,
        canonical_embedding_precision: Optional[int] = None,
    ) -> Any:
        """Create collection with unified interface

        Args:
            name: Collection identifier
            dimension: Vector dimension
            distance_metric: Distance metric enum value
            indexing_algorithm: Indexing algorithm enum value
            storage_engine: Storage engine enum value
            engine: Alias for storage_engine (for backward compatibility)
            filterable_columns: Fields that can be filtered
            index_configs: Index configuration parameters
            quantization_config: Quantization configuration

        Returns:
            Collection creation result
        """
        # Handle backward compatibility: engine is an alias for storage_engine
        if engine is not None and storage_engine is None:
            storage_engine = engine

        # Convert string storage engine names to enum integers if needed
        if storage_engine is not None and isinstance(storage_engine, str):
            from proximadb_sdk.models import StorageEngineType

            storage_engine_map = {
                "viper": StorageEngineType.VIPER,
                "sst": StorageEngineType.SST,
                "nova": StorageEngineType.NOVA,
                "helix": StorageEngineType.HELIX,
                "swift": StorageEngineType.SWIFT,
                "raptor": StorageEngineType.RAPTOR,
            }
            storage_engine_str = storage_engine.lower()
            if storage_engine_str in storage_engine_map:
                storage_engine = int(storage_engine_map[storage_engine_str])
            else:
                raise ValueError(
                    f"Unknown storage engine: {storage_engine}. Valid options: {list(storage_engine_map.keys())}"
                )

        def _create_collection_operation(stub):
            # Build collection config using v1 types
            config = v1_collection_types_pb2.CollectionConfig(
                name=name, dimension=dimension
            )

            if distance_metric is not None:
                config.distance_metric = distance_metric
            # Indexing algorithm is configured via IndexConfig; prefer index_configs param
            # If a simple algorithm enum is provided without configs, create a basic index config
            if indexing_algorithm is not None and not index_configs:
                ic = v1_collection_types_pb2.IndexConfig(
                    index_name=f"{name}_primary",
                    algorithm=indexing_algorithm,
                    is_primary=True,
                )
                config.index_configs.extend([ic])
            if storage_engine is not None:
                config.storage_engine = storage_engine
            if filterable_columns:
                config.filterable_columns.extend(filterable_columns)
            if index_configs:
                config.index_configs.extend(index_configs)
            if quantization_config:
                # Field name in proto is `quantization`
                config.quantization.CopyFrom(quantization_config)
            if canonical_embedding_precision is not None:
                config.canonical_embedding_precision = canonical_embedding_precision

            # Use CollectionService.CreateCollection method from v1 API
            # CreateCollection expects CollectionConfig directly, not wrapped in a request
            response = stub.CreateCollection(config, timeout=self.timeout)

            # Wrap the protobuf Collection to provide .name and .dimension attributes
            return CollectionWrapper(response)

        return self._execute_collection_with_pool(
            "create_collection", _create_collection_operation
        )

    def get_collection(self, name: str) -> Any:
        """Get collection metadata"""

        def _get_collection_operation(stub):
            request = v1_collection_types_pb2.GetCollectionRequest(collection_id=name)
            response = stub.GetCollection(request, timeout=self.timeout)

            # Wrap the protobuf Collection to provide .name and .dimension attributes
            return CollectionWrapper(response)

        return self._execute_collection_with_pool(
            "get_collection", _get_collection_operation
        )

    def list_collections(self) -> List[Any]:
        """List all collections"""

        def _list_collections_operation(stub):
            request = v1_collection_types_pb2.ListCollectionsRequest()
            response = stub.ListCollections(request, timeout=self.timeout)

            # Wrap protobuf Collection objects to provide .name and .dimension attributes
            collections = []
            for coll in response.collections:
                wrapped = CollectionWrapper(coll)
                collections.append(wrapped)

            return collections

        return self._execute_collection_with_pool(
            "list_collections", _list_collections_operation
        )

    def delete_collection(self, collection_id: str) -> DeleteCollectionResponse:
        """Delete collection"""

        def _delete_collection_operation(stub):
            request = v1_collection_types_pb2.DeleteCollectionRequest(
                collection_id=collection_id
            )
            response = stub.DeleteCollection(request, timeout=self.timeout)
            return DeleteCollectionResponse(
                success=response.success, collection_id=collection_id, status="deleted"
            )

        return self._execute_collection_with_pool(
            "delete_collection", _delete_collection_operation
        )

    # Record Operations - Unified Interface
    def _python_to_v2_typed_value(self, value: Any):
        """Encode Python values into v2 ProximaValue/TypedValue protobufs."""
        if v2_record_pb2 is None:
            raise ProximaDBError("v2 record protobuf stubs not available")

        type_hint = None
        if isinstance(value, dict) and set(value.keys()) == {"type", "value"}:
            type_hint = str(value["type"]).lower()
            value = value["value"]

        tv = v2_record_pb2.TypedValue()
        if value is None:
            tv.declared_type = v2_record_pb2.COLUMN_TYPE_UNSPECIFIED
            tv.is_null = True
        elif isinstance(value, bool):
            tv.declared_type = v2_record_pb2.BOOLEAN
            tv.boolean_value = value
        elif isinstance(value, int) and not isinstance(value, bool):
            tv.declared_type = v2_record_pb2.INTEGER
            tv.integer_value = value
        elif isinstance(value, float):
            tv.declared_type = (
                v2_record_pb2.FLOAT32 if type_hint == "float32" else v2_record_pb2.FLOAT
            )
            if type_hint == "float32":
                tv.float32_value = value
            else:
                tv.float_value = value
        elif isinstance(value, (bytes, bytearray, memoryview)):
            tv.declared_type = v2_record_pb2.BINARY
            tv.binary_value = bytes(value)
        elif isinstance(value, str):
            tv.declared_type = (
                v2_record_pb2.SYMBOL if type_hint == "symbol" else v2_record_pb2.TEXT
            )
            if type_hint == "symbol":
                tv.symbol_value = value
            else:
                tv.text_value = value
        elif isinstance(value, (list, tuple)):
            tv.declared_type = v2_record_pb2.ARRAY_ANY
            tv.array_value.values.extend(
                self._python_to_v2_typed_value(item) for item in value
            )
        elif isinstance(value, dict):
            tv.declared_type = v2_record_pb2.JSONB
            tv.jsonb_value = json.dumps(value, separators=(",", ":")).encode("utf-8")
        else:
            tv.declared_type = v2_record_pb2.TEXT
            tv.text_value = str(value)
        return tv

    def _v2_typed_value_to_python(self, value: Any) -> Any:
        """Decode v2 TypedValue protobufs into Python values."""
        which = value.WhichOneof("value")
        if which in (None, "is_null"):
            return None
        if which == "text_value":
            return value.text_value
        if which == "integer_value":
            return value.integer_value
        if which == "float_value":
            return value.float_value
        if which == "boolean_value":
            return value.boolean_value
        if which == "timestamp_value":
            return value.timestamp_value
        if which == "date_value":
            return value.date_value
        if which == "time_value":
            return value.time_value
        if which == "duration_value":
            return value.duration_value
        if which == "uuid_value":
            return bytes(value.uuid_value).hex()
        if which == "binary_value":
            return bytes(value.binary_value)
        if which == "json_value":
            try:
                return json.loads(value.json_value)
            except json.JSONDecodeError:
                return value.json_value
        if which == "jsonb_value":
            try:
                return json.loads(bytes(value.jsonb_value).decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError):
                return bytes(value.jsonb_value)
        if which == "array_value":
            return [
                self._v2_typed_value_to_python(item)
                for item in value.array_value.values
            ]
        if which == "map_value":
            return {
                key: self._v2_typed_value_to_python(item)
                for key, item in value.map_value.entries.items()
            }
        if which == "struct_value":
            return {
                key: self._v2_typed_value_to_python(item)
                for key, item in value.struct_value.entries.items()
            }
        if which == "float32_value":
            return value.float32_value
        if which.endswith("_array"):
            return list(getattr(value, which).values)
        return getattr(value, which)

    def _normalize_vector_alias_records(
        self, vectors: List[Union[Dict[str, Any], Any]]
    ) -> List[Dict[str, Any]]:
        """Normalize legacy vector alias inputs to v2 ProximaRecord payloads."""
        records: List[Dict[str, Any]] = []
        for index, vector_data in enumerate(vectors):
            if hasattr(vector_data, "model_dump"):
                vector_dict = vector_data.model_dump(exclude_none=True)
            elif hasattr(vector_data, "dict"):
                vector_dict = vector_data.dict(exclude_none=True)
            elif hasattr(vector_data, "__dict__") and not isinstance(vector_data, dict):
                vector_dict = {
                    key: value
                    for key, value in vector_data.__dict__.items()
                    if not key.startswith("_")
                }
            else:
                vector_dict = vector_data

            if not isinstance(vector_dict, dict):
                vector_dict = {"id": f"record_{index}", "vector": vector_dict}

            props = (
                vector_dict.get("props")
                or vector_dict.get("metadata")
                or vector_dict.get("typed_fields")
                or {}
            )
            record = {
                "id": vector_dict.get("id")
                or vector_dict.get("oid")
                or f"record_{index}",
                "vector": vector_dict.get("vector"),
                "props": props,
            }
            for field in (
                "timestamp_ms",
                "timestamp",
                "updated_at_ms",
                "updated_at",
                "expires_at_ms",
                "expires_at",
                "version",
                "source",
            ):
                if vector_dict.get(field) is not None:
                    record[field] = vector_dict[field]
            records.append(record)
        return records

    def _record_proto_for_grpc(
        self, record: Union[ProximaRecord, Dict[str, Any]], index: int = 0
    ):
        if v2_record_pb2 is None:
            raise ProximaDBError("v2 record protobuf stubs not available")

        if hasattr(record, "model_dump"):
            record = record.model_dump(exclude_none=True)
        elif hasattr(record, "dict"):
            record = record.dict(exclude_none=True)
        if not isinstance(record, dict):
            raise TypeError(f"Unsupported record input: {type(record)!r}")

        vector = record.get("vector")
        if vector is None and record.get("embeddings"):
            first_embedding = record["embeddings"][0]
            vector = (
                first_embedding.get("values")
                if isinstance(first_embedding, dict)
                else first_embedding
            )
        if vector is None:
            raise ValueError("record is missing vector")

        proto = v2_record_pb2.ProximaRecord()
        proto.id = str(record.get("id") or record.get("oid") or f"record_{index}")
        proto.vector.extend(float(v) for v in vector)
        if record.get("vector_dimension") is not None:
            proto.vector_dimension = int(record["vector_dimension"])

        for source in ("props", "metadata", "flexible_fields"):
            values = record.get(source)
            if isinstance(values, dict):
                for key, value in values.items():
                    proto.props[str(key)].CopyFrom(
                        self._python_to_v2_typed_value(value)
                    )

        typed_fields = record.get("typed_fields")
        if isinstance(typed_fields, dict):
            for key, value in typed_fields.items():
                if hasattr(value, "model_dump"):
                    value = value.model_dump(exclude_none=True)
                if isinstance(value, dict) and "value" in value:
                    value = {
                        "type": value.get("value_type") or value.get("type"),
                        "value": value["value"],
                    }
                proto.props[str(key)].CopyFrom(self._python_to_v2_typed_value(value))

        for text_field in record.get("text_fields") or []:
            if hasattr(text_field, "model_dump"):
                text_field = text_field.model_dump(exclude_none=True)
            if isinstance(text_field, dict):
                proto.text_fields.add(
                    name=str(text_field.get("name") or ""),
                    content=str(text_field.get("content") or ""),
                    storage_hint=str(text_field.get("storage_hint") or ""),
                    chunk_count=int(text_field.get("chunk_count") or 0),
                    chunk_reference=str(text_field.get("chunk_reference") or ""),
                )

        if record.get("timestamp_ms") is not None:
            proto.timestamp_ms = int(record["timestamp_ms"])
        for field in (
            "updated_at_ms",
            "expires_at_ms",
            "version",
            "source",
            "source_type",
            "schema_id",
            "partition_key",
            "created_by",
            "updated_by",
        ):
            if record.get(field) is not None:
                setattr(proto, field, record[field])
        if isinstance(record.get("partition_values"), dict):
            proto.partition_values.update(
                {str(k): str(v) for k, v in record["partition_values"].items()}
            )
        if isinstance(record.get("custom_metadata"), dict):
            proto.custom_metadata.update(
                {str(k): str(v) for k, v in record["custom_metadata"].items()}
            )
        return proto

    def _v2_record_batch_result(self, response) -> BatchResult:
        errors = [
            f"{error.record_id or error.record_index}: {error.error_message}"
            for error in response.errors
        ]
        return BatchResult(
            total=int(response.total_processed),
            success=int(response.success_count),
            failed=int(response.failed_count),
            errors=errors,
            metrics=OperationMetrics(
                total_processed=int(response.total_processed),
                successful_count=int(response.success_count),
                failed_count=int(response.failed_count),
                processing_time_us=int(response.processing_time_us),
            ),
        )

    def insert_records(
        self,
        collection_id: str,
        records: List[Union[ProximaRecord, Dict[str, Any]]],
        **kwargs,
    ) -> BatchResult:
        upsert = bool(kwargs.pop("upsert", False))
        if upsert:
            return self.upsert_records(collection_id, records, **kwargs)

        if v2_record_pb2 is None or v2_record_pb2_grpc is None:
            raise ProximaDBError("v2 ProximaRecord gRPC stubs are required")

        request = v2_record_pb2.ProximaRecordBatch(
            collection_id=collection_id,
            write_mode=v2_record_pb2.INSERT,
            validate_schema=bool(kwargs.get("validate_schema", True)),
            return_ids=bool(kwargs.get("return_ids", True)),
            return_errors=bool(kwargs.get("return_errors", True)),
        )
        request.records.extend(
            self._record_proto_for_grpc(record, index)
            for index, record in enumerate(records)
        )
        if kwargs.get("schema_id"):
            request.schema_id = str(kwargs["schema_id"])

        def _insert_records_operation(stub):
            return stub.InsertRecords(request, timeout=self.timeout)

        return self._v2_record_batch_result(
            self._execute_record_with_pool("insert_records", _insert_records_operation)
        )

    def upsert_records(
        self,
        collection_id: str,
        records: List[Union[ProximaRecord, Dict[str, Any]]],
        **kwargs,
    ) -> BatchResult:
        """Upsert ProximaRecord-shaped payloads."""
        kwargs.pop("upsert", None)
        if v2_record_pb2 is None or v2_record_pb2_grpc is None:
            raise ProximaDBError("v2 ProximaRecord gRPC stubs are required")

        request = v2_record_pb2.ProximaRecordBatch(
            collection_id=collection_id,
            write_mode=v2_record_pb2.UPSERT,
            validate_schema=bool(kwargs.get("validate_schema", True)),
            return_ids=bool(kwargs.get("return_ids", True)),
            return_errors=bool(kwargs.get("return_errors", True)),
        )
        request.records.extend(
            self._record_proto_for_grpc(record, index)
            for index, record in enumerate(records)
        )
        if kwargs.get("schema_id"):
            request.schema_id = str(kwargs["schema_id"])

        def _upsert_records_operation(stub):
            return stub.UpsertRecords(request, timeout=self.timeout)

        return self._v2_record_batch_result(
            self._execute_record_with_pool("upsert_records", _upsert_records_operation)
        )

    # Vector Compatibility Aliases
    def insert_vectors(
        self, collection_id: str, vectors: List[Dict[str, Any]], upsert: bool = False
    ) -> VectorOperationResponse:
        """Insert vectors through the v2 ProximaRecord gRPC surface.

        Args:
            collection_id: Target collection ID
            vectors: List of vector objects with format:
                    [{"id": "vec1", "vector": [0.1, 0.2, ...], "metadata": {...}}, ...]
            upsert: Whether to update existing vectors

        Returns:
            VectorOperationResponse with operation details
        """
        records = self._normalize_vector_alias_records(vectors)
        batch_result = (
            self.upsert_records(collection_id, records)
            if upsert
            else self.insert_records(collection_id, records)
        )
        return VectorOperationResponse(
            success=batch_result.failed == 0,
            operation="UPSERT" if upsert else "INSERT",
            metrics=batch_result.metrics,
            vector_ids=[record["id"] for record in records],
            error_message=(
                "; ".join(batch_result.errors) if batch_result.errors else None
            ),
        )

    def search_vectors(
        self,
        collection_id: str,
        query_vectors: List[List[float]] = None,
        query_vector: List[float] = None,
        top_k: int = 10,
        metadata_filters: Optional[Dict[str, Any]] = None,
        include_vectors: bool = False,
        include_metadata: bool = True,
        search_hints: Optional[Dict[str, Any]] = None,
    ) -> SearchResult:
        """Search vectors through the v2 ProximaRecord gRPC surface.

        Args:
            collection_id: Target collection ID
            query_vector: Query vector
            top_k: Number of results to return
            metadata_filters: Metadata filter conditions
            include_vectors: Include vector data in results
            include_metadata: Include metadata in results

        Returns:
            SearchResult with found vectors
        """
        # Handle both query_vector and query_vectors params
        if query_vectors is None and query_vector is not None:
            query_vectors = [query_vector]
        elif query_vectors is None:
            raise ValueError("Either query_vector or query_vectors must be provided")

        def _search_vectors_operation(stub):
            search_results = []
            for qv in query_vectors:
                request = v2_record_pb2.TypedSearchRequest(
                    collection_id=collection_id,
                    top_k=top_k,
                    include_vector=include_vectors,
                    include_text_fields=False,
                )
                request.query_vector.extend(float(value) for value in qv)
                request.filter_logic = v2_record_pb2.AND
                if metadata_filters:
                    for key, value in metadata_filters.items():
                        filter_condition = request.filters.add()
                        filter_condition.field_name = str(key)
                        filter_condition.operator = v2_record_pb2.EQ
                        filter_condition.value.CopyFrom(
                            self._python_to_v2_typed_value(value)
                        )
                if search_hints:
                    request.search_hints.update(
                        {str(key): str(value) for key, value in search_hints.items()}
                    )

                response = stub.Search(request, timeout=self.timeout)
                for rank, result in enumerate(response.results):
                    metadata = None
                    if include_metadata:
                        metadata = {
                            key: self._v2_typed_value_to_python(value)
                            for key, value in result.props.items()
                        }
                    search_results.append(
                        SearchResult(
                            id=result.id,
                            score=result.score,
                            rank=rank,
                            vector=(
                                list(result.vector)
                                if include_vectors and result.vector
                                else None
                            ),
                            metadata=metadata,
                            timestamp=(
                                result.timestamp_ms
                                if result.HasField("timestamp_ms")
                                else None
                            ),
                            version=(
                                result.version if result.HasField("version") else None
                            ),
                            source=(
                                result.source if result.HasField("source") else None
                            ),
                        )
                    )

            return SearchResultsWrapper(search_results)

        return self._execute_record_with_pool(
            "search_vectors", _search_vectors_operation
        )

    def search(
        self,
        collection_id: str = None,  # Can be positional or keyword
        query_vector: List[float] = None,  # Can be positional
        query_vectors: List[List[float]] = None,
        top_k: int = None,
        k: int = None,  # Backward compatibility alias for top_k
        collection_name: str = None,  # Backward compatibility alias
        metadata_filters: Optional[Dict[str, Any]] = None,
        include_vectors: bool = False,
        include_metadata: bool = True,
        search_hints: Optional[Dict[str, Any]] = None,
    ) -> SearchResult:
        """
        Alias for search_vectors() for backward compatibility and convenience

        This method provides the same functionality as search_vectors() but with
        a shorter, more intuitive name commonly expected by users.

        Args:
            collection_id: Target collection ID (can use collection_name instead)
            query_vector: Single query vector (convenience param)
            query_vectors: Multiple query vectors (batch search)
            top_k: Number of results to return per query
            k: Alias for top_k (backward compatibility)
            collection_name: Alias for collection_id (backward compatibility)
            metadata_filters: Metadata filter conditions
            include_vectors: Include vector data in results
            include_metadata: Include metadata in results
            search_hints: Optional search optimization hints

        Returns:
            SearchResult with found vectors
        """
        # Handle backward compatibility aliases
        if collection_name is not None and collection_id is None:
            collection_id = collection_name
        if collection_id is None:
            raise ValueError("Either collection_id or collection_name must be provided")
        if k is not None and top_k is None:
            top_k = k
        if top_k is None:
            top_k = 10  # Default value

        return self.search_vectors(
            collection_id=collection_id,
            query_vector=query_vector,
            query_vectors=query_vectors,
            top_k=top_k,
            metadata_filters=metadata_filters,
            include_vectors=include_vectors,
            include_metadata=include_metadata,
            search_hints=search_hints,
        )

    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True,
    ) -> Dict[str, Any]:
        """Get single vector by ID"""

        def _get_vector_operation(stub):
            # v1 proto uses direct boolean fields, not IncludeFields object
            request = v1_vector_types_pb2.VectorGetRequest(
                collection_id=collection_id,
                vector_id=vector_id,
                include_vector=include_vector,
                include_metadata=include_metadata,
            )
            response = stub.VectorGet(request, timeout=self.timeout)

            # Convert response to dict
            if not response.success:
                raise ProximaDBError(f"Vector {vector_id} not found")

            # Extract from results if available
            if response.results and response.results.results:
                result_item = response.results.results[0]
                result = {
                    "id": result_item.id,
                }
                if include_vector and result_item.vector:
                    result["vector"] = list(result_item.vector)
                if include_metadata and result_item.metadata:
                    # Convert map<string, SqlValue> to dict
                    metadata_dict = {}
                    for key in result_item.metadata:
                        sql_value = result_item.metadata[key]
                        if sql_value.HasField("string_value"):
                            metadata_dict[key] = sql_value.string_value
                        elif sql_value.HasField("int64_value"):
                            metadata_dict[key] = sql_value.int64_value
                        elif sql_value.HasField("number_value"):
                            metadata_dict[key] = sql_value.number_value
                        elif sql_value.HasField("bool_value"):
                            metadata_dict[key] = sql_value.bool_value
                    result["metadata"] = metadata_dict

                # Add timestamp field (SearchVectorRecord has timestamp at field 7)
                if result_item.HasField("timestamp"):
                    result["timestamp_ms"] = result_item.timestamp

                # Add version field (SearchVectorRecord has version at field 5)
                if result_item.HasField("version"):
                    result["version"] = result_item.version

                # Add source field (SearchVectorRecord has source at field 8)
                if result_item.HasField("source"):
                    result["source"] = result_item.source

                # NOTE: SearchVectorRecord does NOT have updated_at or expires_at fields
                # Those fields only exist in the insert VectorRecord proto

                # Wrap result to provide attribute access
                return VectorWrapper(result)
            else:
                raise ProximaDBError(f"Vector {vector_id} not found")

        return self._execute_with_pool("get_vector", _get_vector_operation)

    def update_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Optional[List[float]] = None,
        metadata: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]:
        """Update vector data and/or metadata"""
        # Treat vector update as an upsert over the v2 ProximaRecord contract.
        vector_data = {"id": vector_id}
        if vector is not None:
            vector_data["vector"] = vector
        if metadata is not None:
            vector_data["metadata"] = metadata

        # Use upsert functionality
        result = self.insert_vectors(
            collection_id=collection_id, vectors=[vector_data], upsert=True
        )

        return {
            "status": "updated" if result.success else "failed",
            "vector_id": vector_id,
            "success": result.success,
        }

    def delete_vector(self, collection_id: str, vector_id: str) -> Dict[str, Any]:
        """Delete a vector through the v2 ProximaRecord gRPC surface."""

        def _delete_vector_operation(stub):
            record = v2_record_pb2.ProximaRecord(id=vector_id)
            request = v2_record_pb2.ProximaRecordBatch(
                collection_id=collection_id,
                write_mode=v2_record_pb2.DELETE,
                return_ids=True,
                return_errors=True,
            )
            request.records.append(record)
            response = stub.DeleteRecords(request, timeout=self.timeout)
            return DictWrapper(
                {
                    "status": "deleted" if response.failed_count == 0 else "failed",
                    "vector_id": vector_id,
                    "success": response.failed_count == 0,
                }
            )

        return self._execute_record_with_pool("delete_vector", _delete_vector_operation)

    def delete_vectors(
        self, collection_id: str, vector_ids: List[str]
    ) -> Dict[str, Any]:
        """Delete multiple vectors through the v2 ProximaRecord gRPC surface."""

        def _delete_vectors_operation(stub):
            request = v2_record_pb2.ProximaRecordBatch(
                collection_id=collection_id,
                write_mode=v2_record_pb2.DELETE,
                return_ids=True,
                return_errors=True,
            )
            for vector_id in vector_ids:
                request.records.append(v2_record_pb2.ProximaRecord(id=vector_id))
            response = stub.DeleteRecords(request, timeout=self.timeout)

            return {
                "status": "completed",
                "deleted_count": int(response.success_count),
                "failed_count": int(response.failed_count),
                "total_requested": len(vector_ids),
            }

        return self._execute_record_with_pool(
            "delete_vectors", _delete_vectors_operation
        )

    def insert_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: List[float],
        metadata: Optional[Dict[str, Any]] = None,
        upsert: bool = False,
    ) -> VectorOperationResponse:
        """Insert a single vector - alias for batch insert with one vector

        Args:
            collection_id: Collection ID or name
            vector_id: Vector identifier
            vector: Vector data
            metadata: Optional metadata
            upsert: If True, update existing vector

        Returns:
            VectorOperationResponse
        """
        # Use the batch insert with a single vector
        vector_data = {"id": vector_id, "vector": vector}
        if metadata:
            vector_data["metadata"] = metadata

        return self.insert_vectors(
            collection_id=collection_id, vectors=[vector_data], upsert=upsert
        )

    # === GRAPH OPERATIONS (v1) ===

    def _convert_to_property_value(self, value: Any):
        """Convert Python value to PropertyValue proto"""
        if v1_graph_pb2 is None:
            raise ProximaDBError(
                "Graph protos not available. Run: make -C clients/python gen-proto"
            )

        if isinstance(value, str):
            return v1_graph_pb2.PropertyValue(string_value=value)
        elif isinstance(value, bool):
            return v1_graph_pb2.PropertyValue(bool_value=value)
        elif isinstance(value, int):
            return v1_graph_pb2.PropertyValue(int_value=value)
        elif isinstance(value, float):
            return v1_graph_pb2.PropertyValue(double_value=value)
        elif isinstance(value, bytes):
            return v1_graph_pb2.PropertyValue(bytes_value=value)
        elif isinstance(value, list):
            array_values = [self._convert_to_property_value(item) for item in value]
            return v1_graph_pb2.PropertyValue(
                array_value=v1_graph_pb2.PropertyArray(values=array_values)
            )
        elif isinstance(value, dict):
            object_fields = {
                k: self._convert_to_property_value(v) for k, v in value.items()
            }
            return v1_graph_pb2.PropertyValue(
                object_value=v1_graph_pb2.PropertyObject(fields=object_fields)
            )
        else:
            return v1_graph_pb2.PropertyValue(string_value=str(value))

    def _convert_from_property_value(self, prop_value) -> Any:
        """Convert PropertyValue proto to Python value"""
        if prop_value.HasField("string_value"):
            return prop_value.string_value
        elif prop_value.HasField("int_value"):
            return prop_value.int_value
        elif prop_value.HasField("double_value"):
            return prop_value.double_value
        elif prop_value.HasField("bool_value"):
            return prop_value.bool_value
        elif prop_value.HasField("bytes_value"):
            return prop_value.bytes_value
        elif prop_value.HasField("array_value"):
            return [
                self._convert_from_property_value(item)
                for item in prop_value.array_value.values
            ]
        elif prop_value.HasField("object_value"):
            return {
                k: self._convert_from_property_value(v)
                for k, v in prop_value.object_value.fields.items()
            }
        else:
            return None

    def _convert_node_from_proto(self, node) -> Dict[str, Any]:
        """Convert Node proto to dictionary"""
        from datetime import datetime, timezone

        return {
            "id": node.id,
            "labels": list(node.labels),
            "properties": {
                k: self._convert_from_property_value(v)
                for k, v in node.properties.items()
            },
            "created_at": (
                datetime.fromtimestamp(
                    node.created_at_ms / 1000, tz=timezone.utc
                ).isoformat()
                if node.created_at_ms
                else None
            ),
            "updated_at": (
                datetime.fromtimestamp(
                    node.updated_at_ms / 1000, tz=timezone.utc
                ).isoformat()
                if node.updated_at_ms
                else None
            ),
        }

    def _convert_edge_from_proto(self, edge) -> Dict[str, Any]:
        """Convert Edge proto to dictionary"""
        from datetime import datetime, timezone

        return {
            "id": edge.id,
            "from_node_id": edge.from_node_id,
            "to_node_id": edge.to_node_id,
            "edge_type": edge.edge_type,
            "properties": {
                k: self._convert_from_property_value(v)
                for k, v in edge.properties.items()
            },
            "weight": edge.weight if edge.HasField("weight") else None,
            "created_at": (
                datetime.fromtimestamp(
                    edge.created_at_ms / 1000, tz=timezone.utc
                ).isoformat()
                if edge.created_at_ms
                else None
            ),
            "updated_at": (
                datetime.fromtimestamp(
                    edge.updated_at_ms / 1000, tz=timezone.utc
                ).isoformat()
                if edge.updated_at_ms
                else None
            ),
        }

    def _convert_path_from_proto(self, path) -> List[str]:
        """Convert GraphPath proto to list of node IDs"""
        if hasattr(path, "node_ids"):
            return list(path.node_ids)
        else:
            return []

    def create_node(
        self,
        node_id: str,
        labels: List[str],
        properties: Optional[Dict[str, Any]] = None,
        embedding: Optional[List[float]] = None,
        graph_id: str = "default",
    ) -> Dict[str, Any]:
        """Create a graph node via gRPC

        Args:
            node_id: Unique identifier for the node
            labels: List of labels for the node
            properties: Optional dictionary of node properties
            embedding: Optional embedding vector for the node
            graph_id: Graph collection ID (defaults to "default")

        Returns:
            Dictionary representation of the created node
        """
        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )
        if v1_graph_pb2_grpc is None or v1_graph_pb2 is None:
            raise ProximaDBError(
                "GraphService stubs not found. Run: make -C clients/python gen-proto"
            )

        def _op(channel):
            stub = v1_graph_pb2_grpc.GraphServiceStub(channel)

            node_properties = {}
            if properties:
                for key, value in properties.items():
                    node_properties[key] = self._convert_to_property_value(value)

            node = v1_graph_pb2.Node(
                id=node_id, labels=labels, properties=node_properties
            )

            request = v1_graph_pb2.CreateNodeRequest(graph_id=graph_id, node=node)
            response = stub.CreateNode(request, timeout=self.timeout)
            return self._convert_node_from_proto(response)

        try:
            with GrpcChannelContext(self._connection_pool) as channel:
                return _op(channel)
        except grpc.RpcError as e:
            logger.error(f"gRPC create_node RPC error: {e.code()} - {e.details()}")
            raise ProximaDBError(f"create_node RPC failed: {e.details()}")
        except Exception as e:
            logger.error(f"gRPC create_node failed: {e}")
            raise ProximaDBError(f"create_node failed: {e}")

    def create_edge(
        self,
        edge_id: str,
        from_node_id: str,
        to_node_id: str,
        edge_type: str,
        properties: Optional[Dict[str, Any]] = None,
        weight: Optional[float] = None,
        graph_id: str = "default",
    ) -> Dict[str, Any]:
        """Create a graph edge via gRPC

        Args:
            edge_id: Unique identifier for the edge
            from_node_id: Source node ID
            to_node_id: Target node ID
            edge_type: Type/label of the edge
            properties: Optional dictionary of edge properties
            weight: Optional edge weight
            graph_id: Graph collection ID (defaults to "default")

        Returns:
            Dictionary representation of the created edge
        """
        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )
        if v1_graph_pb2_grpc is None or v1_graph_pb2 is None:
            raise ProximaDBError(
                "GraphService stubs not found. Run: make -C clients/python gen-proto"
            )

        def _op(channel):
            stub = v1_graph_pb2_grpc.GraphServiceStub(channel)

            edge_properties = {}
            if properties:
                for key, value in properties.items():
                    edge_properties[key] = self._convert_to_property_value(value)

            edge = v1_graph_pb2.Edge(
                id=edge_id,
                from_node_id=from_node_id,
                to_node_id=to_node_id,
                edge_type=edge_type,
                properties=edge_properties,
            )

            if weight is not None:
                edge.weight = weight

            request = v1_graph_pb2.CreateEdgeRequest(graph_id=graph_id, edge=edge)
            response = stub.CreateEdge(request, timeout=self.timeout)
            return self._convert_edge_from_proto(response)

        try:
            with GrpcChannelContext(self._connection_pool) as channel:
                return _op(channel)
        except grpc.RpcError as e:
            logger.error(f"gRPC create_edge RPC error: {e.code()} - {e.details()}")
            raise ProximaDBError(f"create_edge RPC failed: {e.details()}")
        except Exception as e:
            logger.error(f"gRPC create_edge failed: {e}")
            raise ProximaDBError(f"create_edge failed: {e}")

    def traverse_graph(
        self,
        start_node_id: str,
        max_depth: int = 3,
        edge_types: Optional[List[str]] = None,
        node_labels: Optional[List[str]] = None,
        algorithm: str = "BFS",
        limit: Optional[int] = None,
        graph_id: str = "default",
    ) -> Dict[str, Any]:
        """Traverse graph from a starting node via gRPC

        Args:
            start_node_id: ID of the node to start traversal from
            max_depth: Maximum depth to traverse (default: 3)
            edge_types: Optional list of edge types to follow
            node_labels: Optional list of node labels to include
            algorithm: Traversal algorithm - "BFS", "DFS", or "PARALLEL_BFS" (default: "BFS")
            limit: Optional limit on number of results
            graph_id: Graph collection ID (defaults to "default")

        Returns:
            Dictionary with nodes, edges, paths, and traversal statistics
        """
        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )
        if v1_graph_pb2_grpc is None or v1_graph_pb2 is None:
            raise ProximaDBError(
                "GraphService stubs not found. Run: make -C clients/python gen-proto"
            )

        def _op(channel):
            stub = v1_graph_pb2_grpc.GraphServiceStub(channel)

            # Map algorithm string to enum
            algorithm_enum = v1_graph_pb2.TRAVERSAL_ALGORITHM_BFS
            if algorithm.upper() == "DFS":
                algorithm_enum = v1_graph_pb2.TRAVERSAL_ALGORITHM_DFS
            elif algorithm.upper() == "PARALLEL_BFS":
                algorithm_enum = v1_graph_pb2.TRAVERSAL_ALGORITHM_PARALLEL_BFS

            request = v1_graph_pb2.TraversalRequest(
                graph_id=graph_id,
                start_node_id=start_node_id,
                max_depth=max_depth,
                edge_types=edge_types or [],
                node_labels=node_labels or [],
                algorithm=algorithm_enum,
            )

            if limit is not None:
                request.limit = limit

            response = stub.TraverseGraph(request, timeout=self.timeout)

            return {
                "nodes": [
                    self._convert_node_from_proto(node) for node in response.nodes
                ],
                "edges": [
                    self._convert_edge_from_proto(edge) for edge in response.edges
                ],
                "paths": [
                    self._convert_path_from_proto(path) for path in response.paths
                ],
                "stats": {
                    "nodes_visited": (
                        response.stats.nodes_visited
                        if hasattr(response, "stats")
                        else 0
                    ),
                    "edges_traversed": (
                        response.stats.edges_traversed
                        if hasattr(response, "stats")
                        else 0
                    ),
                    "max_depth_reached": (
                        response.stats.max_depth_reached
                        if hasattr(response, "stats")
                        else 0
                    ),
                    "execution_time_microseconds": (
                        response.stats.execution_time_microseconds
                        if hasattr(response, "stats")
                        else 0
                    ),
                },
            }

        try:
            with GrpcChannelContext(self._connection_pool) as channel:
                return _op(channel)
        except grpc.RpcError as e:
            logger.error(f"gRPC traverse_graph RPC error: {e.code()} - {e.details()}")
            raise ProximaDBError(f"traverse_graph RPC failed: {e.details()}")
        except Exception as e:
            logger.error(f"gRPC traverse_graph failed: {e}")
            raise ProximaDBError(f"traverse_graph failed: {e}")

    def query_nodes(
        self,
        labels: Optional[List[str]] = None,
        properties: Optional[Dict[str, Any]] = None,
        limit: Optional[int] = None,
        offset: Optional[int] = None,
        graph_id: str = "default",
    ) -> Dict[str, Any]:
        """Query nodes by labels and properties via gRPC

        Args:
            labels: Optional list of labels to filter by
            properties: Optional dictionary of properties to filter by
            limit: Optional maximum number of results
            offset: Optional offset for pagination
            graph_id: Graph collection ID (defaults to "default")

        Returns:
            Dictionary with success status, nodes list, and total count
        """
        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )
        if v1_graph_pb2_grpc is None or v1_graph_pb2 is None:
            raise ProximaDBError(
                "GraphService stubs not found. Run: make -C clients/python gen-proto"
            )

        def _op(channel):
            stub = v1_graph_pb2_grpc.GraphServiceStub(channel)

            filters = []
            if properties:
                for key, value in properties.items():
                    filters.append(
                        v1_graph_pb2.PropertyFilter(
                            key=key,
                            operator=v1_graph_pb2.PROPERTY_FILTER_OPERATOR_EQUALS,
                            value=self._convert_to_property_value(value),
                        )
                    )

            request = v1_graph_pb2.NodeQuery(
                graph_id=graph_id, labels=labels or [], filters=filters
            )

            if limit is not None:
                request.limit = limit
            if offset is not None:
                request.offset = offset

            response = stub.QueryNodes(request, timeout=self.timeout)
            return {
                "success": response.success if hasattr(response, "success") else True,
                "nodes": [
                    self._convert_node_from_proto(node) for node in response.nodes
                ],
                "total_count": len(response.nodes),
            }

        try:
            with GrpcChannelContext(self._connection_pool) as channel:
                return _op(channel)
        except grpc.RpcError as e:
            logger.error(f"gRPC query_nodes RPC error: {e.code()} - {e.details()}")
            raise ProximaDBError(f"query_nodes RPC failed: {e.details()}")
        except Exception as e:
            logger.error(f"gRPC query_nodes failed: {e}")
            raise ProximaDBError(f"query_nodes failed: {e}")

    def query_edges(
        self,
        edge_type: str = "",
        from_node_id: Optional[str] = None,
        to_node_id: Optional[str] = None,
        properties: Optional[Dict[str, Any]] = None,
        limit: Optional[int] = None,
        offset: Optional[int] = None,
        graph_id: str = "default",
    ) -> Dict[str, Any]:
        """Query edges by endpoints, type, and properties via gRPC."""
        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )
        if v1_graph_pb2_grpc is None or v1_graph_pb2 is None:
            raise ProximaDBError(
                "GraphService stubs not found. Run: make -C clients/python gen-proto"
            )

        def _op(channel):
            stub = v1_graph_pb2_grpc.GraphServiceStub(channel)

            filters = []
            if properties:
                for key, value in properties.items():
                    filters.append(
                        v1_graph_pb2.PropertyFilter(
                            key=key,
                            operator=v1_graph_pb2.PROPERTY_FILTER_OPERATOR_EQUALS,
                            value=self._convert_to_property_value(value),
                        )
                    )

            request = v1_graph_pb2.EdgeQuery(
                graph_id=graph_id,
                edge_types=[edge_type] if edge_type else [],
                filters=filters,
            )

            if from_node_id is not None:
                request.from_node_id = from_node_id
            if to_node_id is not None:
                request.to_node_id = to_node_id
            if limit is not None:
                request.limit = limit
            if offset is not None:
                request.offset = offset

            response = stub.QueryEdges(request, timeout=self.timeout)
            return {
                "success": response.success if hasattr(response, "success") else True,
                "edges": [
                    self._convert_edge_from_proto(edge) for edge in response.edges
                ],
                "total_count": len(response.edges),
                "next_token": (
                    response.next_token if hasattr(response, "next_token") else None
                ),
            }

        try:
            with GrpcChannelContext(self._connection_pool) as channel:
                return _op(channel)
        except grpc.RpcError as e:
            logger.error(f"gRPC query_edges RPC error: {e.code()} - {e.details()}")
            raise ProximaDBError(f"query_edges RPC failed: {e.details()}")
        except Exception as e:
            logger.error(f"gRPC query_edges failed: {e}")
            raise ProximaDBError(f"query_edges failed: {e}")

    def get_node(
        self,
        node_id: str,
        graph_id: str = "default",
    ) -> Optional[Dict[str, Any]]:
        """Get a graph node by ID via gRPC."""
        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )
        if v1_graph_pb2_grpc is None or v1_graph_pb2 is None:
            raise ProximaDBError(
                "GraphService stubs not found. Run: make -C clients/python gen-proto"
            )

        def _op(channel):
            stub = v1_graph_pb2_grpc.GraphServiceStub(channel)
            request = v1_graph_pb2.GetNodeRequest(graph_id=graph_id, node_id=node_id)
            response = stub.GetNode(request, timeout=self.timeout)
            return self._convert_node_from_proto(response)

        try:
            with GrpcChannelContext(self._connection_pool) as channel:
                return _op(channel)
        except grpc.RpcError as e:
            logger.error(f"gRPC get_node RPC error: {e.code()} - {e.details()}")
            raise ProximaDBError(f"get_node RPC failed: {e.details()}")
        except Exception as e:
            logger.error(f"gRPC get_node failed: {e}")
            raise ProximaDBError(f"get_node failed: {e}")

    def get_outgoing_edges(
        self,
        node_id: str,
        edge_types: Optional[List[str]] = None,
        graph_id: str = "default",
    ) -> List[Dict[str, Any]]:
        """Get outgoing graph edges for a node via gRPC."""
        edge_types = edge_types or [""]
        edges: List[Dict[str, Any]] = []
        for edge_type in edge_types:
            result = self.query_edges(
                edge_type=edge_type,
                from_node_id=node_id,
                graph_id=graph_id,
                limit=10000,
            )
            edges.extend(result.get("edges", []))
        return edges

    def get_incoming_edges(
        self,
        node_id: str,
        edge_types: Optional[List[str]] = None,
        graph_id: str = "default",
    ) -> List[Dict[str, Any]]:
        """Get incoming graph edges for a node via gRPC."""
        edge_types = edge_types or [""]
        edges: List[Dict[str, Any]] = []
        for edge_type in edge_types:
            result = self.query_edges(
                edge_type=edge_type,
                to_node_id=node_id,
                graph_id=graph_id,
                limit=10000,
            )
            edges.extend(result.get("edges", []))
        return edges

    def delete_node(
        self,
        node_id: str,
        graph_id: str = "default",
    ) -> Dict[str, Any]:
        """Delete a graph node by ID via gRPC."""
        if not GRPC_AVAILABLE:
            raise ProximaDBError(
                "gRPC not available. Install with: pip install grpcio grpcio-tools"
            )
        if v1_graph_pb2_grpc is None or v1_graph_pb2 is None:
            raise ProximaDBError(
                "GraphService stubs not found. Run: make -C clients/python gen-proto"
            )

        def _op(channel):
            stub = v1_graph_pb2_grpc.GraphServiceStub(channel)
            request = v1_graph_pb2.DeleteNodeRequest(graph_id=graph_id, node_id=node_id)
            response = stub.DeleteNode(request, timeout=self.timeout)
            return self._convert_node_from_proto(response)

        try:
            with GrpcChannelContext(self._connection_pool) as channel:
                return _op(channel)
        except grpc.RpcError as e:
            logger.error(f"gRPC delete_node RPC error: {e.code()} - {e.details()}")
            raise ProximaDBError(f"delete_node RPC failed: {e.details()}")
        except Exception as e:
            logger.error(f"gRPC delete_node failed: {e}")
            raise ProximaDBError(f"delete_node failed: {e}")


# Alias for consistency
ProximaDBClient = ProximaDBSyncGrpcClient
