"""
ProximaDB Unified Python Client

Unified client interface that can use either REST or gRPC protocols.
Automatically selects gRPC for better performance when available,
with graceful fallback to REST for compatibility.

This client provides type conversion between proto and Pydantic models
to maintain a consistent interface regardless of the underlying protocol.
"""

import logging
import time
import warnings
from typing import Any, Dict, List, Optional, Union
from enum import Enum

import numpy as np
from tenacity import retry, stop_after_attempt, wait_exponential, retry_if_exception_type

from .config import ClientConfig, load_config, Protocol
from .protocol_selector import (
    ProtocolSelector, 
    SelectionStrategy, 
    create_protocol_selector
)
from .operation_router import (
    OperationRouter,
    RoutingConfig,
    RoutingStrategy,
    create_operation_router
)
from .models import (
    Collection,
    CollectionConfig,
    SearchResult,
    VectorOperationResponse,
    OperationMetrics,
    HealthStatus,
    VectorRecord,
    VectorArray,
    MetadataDict,
    FilterDict,
    DistanceMetric,
    StorageEngine,
    IndexingAlgorithm,
    QuantizationConfig,
    QuantizationType,
)
from .exceptions import (
    ProximaDBError,
    NetworkError,
    TimeoutError,
    RateLimitError,
    map_http_error,
)

try:
    from . import proximadb_pb2 as pb2
    GRPC_AVAILABLE = True
except ImportError:
    GRPC_AVAILABLE = False

logger = logging.getLogger(__name__)


# Protocol enum imported from config module


class ProximaDBClient:
    """
    Unified ProximaDB Python Client
    
    Supports both REST and gRPC protocols with automatic selection
    for optimal performance and compatibility. Provides a consistent
    interface using Pydantic models regardless of the underlying protocol.
    """
    
    def __init__(
        self,
        url: Optional[str] = None,
        api_key: Optional[str] = None,
        protocol: Union[Protocol, str] = Protocol.AUTO,
        config: Optional[ClientConfig] = None,
        enable_http2: bool = True,
        pool_size: int = 10,
        pool_maxsize: int = 50,
        verify_ssl: bool = True,
        cert_file: Optional[str] = None,
        key_file: Optional[str] = None,
        enable_intelligent_selection: bool = False,
        selection_strategy: SelectionStrategy = SelectionStrategy.BALANCED,
        enable_operation_routing: bool = False,
        routing_strategy: RoutingStrategy = RoutingStrategy.HYBRID,
        routing_config: Optional[RoutingConfig] = None,
        sks_warmup_collection: Optional[str] = None,
        **kwargs
    ):
        """
        Initialize ProximaDB client
        
        Args:
            url: ProximaDB server URL
            api_key: API key for authentication  
            protocol: Communication protocol (auto, grpc, rest)
            config: Client configuration object
            enable_http2: Enable HTTP/2 support for better performance
            pool_size: Connection pool size for keepalive connections
            pool_maxsize: Maximum connection pool size
            verify_ssl: Verify SSL certificates
            cert_file: Client certificate file path for mTLS
            key_file: Client key file path for mTLS
            enable_intelligent_selection: Enable intelligent protocol selection (Phase 2 optimization)
            selection_strategy: Strategy for intelligent protocol selection
            enable_operation_routing: Enable operation-specific routing (Phase 3 optimization)
            routing_strategy: Strategy for operation routing (HYBRID, PERFORMANCE_BASED, etc.)
            routing_config: Custom routing configuration
            **kwargs: Additional configuration parameters
        """
        if config is None:
            config = load_config(url=url, api_key=api_key, **kwargs)
        
        # Update config with connection parameters
        if hasattr(config, 'connection'):
            config.connection.pool_size = pool_size
            config.connection.pool_maxsize = pool_maxsize
        if hasattr(config, 'tls'):
            config.tls.verify = verify_ssl
            config.tls.cert_file = cert_file
            config.tls.key_file = key_file
        if hasattr(config, 'enable_http2'):
            config.enable_http2 = enable_http2
        
        self.config = config
        self.protocol = Protocol(protocol) if isinstance(protocol, str) else protocol
        self.enable_intelligent_selection = enable_intelligent_selection
        self.selection_strategy = selection_strategy
        self.enable_operation_routing = enable_operation_routing
        self.routing_strategy = routing_strategy
        self._sks_warmup_collection = sks_warmup_collection
        
        # Client state
        self._client = None
        self._protocol_selector: Optional[ProtocolSelector] = None
        self._operation_router: Optional[OperationRouter] = None
        self._rest_client = None
        self._grpc_client = None
        
        # Setup operation routing if enabled
        if self.enable_operation_routing:
            self._setup_operation_routing(routing_config)
        
        self._setup_client()
    
    def _setup_client(self):
        """Setup the underlying client based on protocol preference"""
        if self.enable_intelligent_selection and self.protocol == Protocol.AUTO:
            # Use intelligent protocol selection (Phase 2 optimization)
            logger.info(f"🧠 Enabling intelligent protocol selection with {self.selection_strategy.value} strategy")
            self._setup_intelligent_selection()
        elif self.protocol == Protocol.AUTO:
            # Traditional auto-selection (try gRPC first, fallback to REST)
            try:
                if not GRPC_AVAILABLE:
                    raise ImportError("gRPC dependencies not available")
                self._client = self._create_grpc_client()
                self._active_protocol = Protocol.GRPC
                logger.info("🔗 Using gRPC client for high performance")
            except ImportError:
                logger.warning("⚠️ gRPC dependencies not available, falling back to REST")
                self._client = self._create_rest_client()
                self._active_protocol = Protocol.REST
            except Exception as e:
                logger.warning(f"⚠️ gRPC client failed: {e}, falling back to REST")
                self._client = self._create_rest_client()
                self._active_protocol = Protocol.REST
            # Optional SKS warmup when REST is active
            if self._active_protocol == Protocol.REST and self._sks_warmup_collection:
                try:
                    # Warmup only for REST path
                    rest_client = self._client
                    if hasattr(rest_client, 'warmup_sks_capabilities'):
                        rest_client.warmup_sks_capabilities(self._sks_warmup_collection)
                except Exception as e:
                    logger.debug(f"SKS warmup skipped due to error: {e}")
                    
        elif self.protocol == Protocol.GRPC:
            # Force gRPC
            if not GRPC_AVAILABLE:
                raise ImportError("gRPC dependencies not available. Install with: pip install grpcio grpcio-tools protobuf")
            self._client = self._create_grpc_client()
            self._active_protocol = Protocol.GRPC
            logger.info("🔗 Using gRPC client (forced)")
            
        elif self.protocol == Protocol.REST:
            # Force REST
            self._client = self._create_rest_client()
            self._active_protocol = Protocol.REST
            logger.info("🌐 Using REST client (forced)")
            # Optional SKS warmup when REST is forced
            if self._sks_warmup_collection:
                try:
                    rest_client = self._client
                    if hasattr(rest_client, 'warmup_sks_capabilities'):
                        rest_client.warmup_sks_capabilities(self._sks_warmup_collection)
                except Exception as e:
                    logger.debug(f"SKS warmup skipped due to error: {e}")
        
        else:
            raise ValueError(f"Unknown protocol: {self.protocol}")
    
    def _setup_intelligent_selection(self):
        """Setup intelligent protocol selection system"""
        try:
            # Create protocol selector with client factories
            self._protocol_selector = create_protocol_selector(
                config=self.config,
                grpc_factory=self._create_grpc_client,
                rest_factory=self._create_rest_client,
                strategy=self.selection_strategy
            )
            
            # Get initial client
            self._client = self._protocol_selector.get_client()
            self._active_protocol = self._protocol_selector.select_protocol()
            
            logger.info(f"🧠 Intelligent protocol selection initialized: {self._active_protocol.value}")
            # Optional SKS warmup if REST is initially selected
            if self._active_protocol == Protocol.REST and self._sks_warmup_collection:
                try:
                    rest_client = self._client if hasattr(self._client, 'warmup_sks_capabilities') else None
                    if rest_client:
                        rest_client.warmup_sks_capabilities(self._sks_warmup_collection)
                except Exception as e:
                    logger.debug(f"SKS warmup skipped due to error: {e}")
            
        except Exception as e:
            logger.warning(f"⚠️ Intelligent selection failed: {e}, falling back to traditional auto-selection")
            # Fallback to traditional selection
            self.enable_intelligent_selection = False
            self._setup_client()
    
    def _setup_operation_routing(self, routing_config: Optional[RoutingConfig]):
        """Setup operation-specific routing system"""
        try:
            # Create routing configuration if not provided
            if routing_config is None:
                routing_config = RoutingConfig(
                    strategy=self.routing_strategy,
                    default_protocol=Protocol.GRPC if GRPC_AVAILABLE else Protocol.REST,
                    enable_adaptive_routing=True,
                    enable_fallback=True,
                    enable_load_balancing=True
                )
            
            # Create operation router
            self._operation_router = OperationRouter(routing_config)
            
            # Pre-create both clients for routing
            self._rest_client = self._create_rest_client()
            if GRPC_AVAILABLE:
                try:
                    self._grpc_client = self._create_grpc_client()
                except Exception as e:
                    logger.warning(f"⚠️ gRPC client creation failed: {e}")
                    self._grpc_client = None
            
            logger.info(f"🎯 Operation-specific routing enabled with {self.routing_strategy.value} strategy")
            
        except Exception as e:
            logger.warning(f"⚠️ Operation routing setup failed: {e}, disabling routing")
            self.enable_operation_routing = False
            self._operation_router = None
    
    def _create_grpc_client(self):
        """Create gRPC client"""
        from .protocols.grpc_sync import ProximaDBSyncGrpcClient
        from .config import Protocol
        
        # Use the proper protocol URL generation for gRPC
        grpc_url = self.config.get_protocol_url(Protocol.GRPC)
        
        # Pass compression settings from config
        return ProximaDBSyncGrpcClient(
            server_address=grpc_url,
            timeout=60.0,
            enable_compression=self.config.compression.enabled if hasattr(self.config, 'compression') else True,
            compression_algorithm=self.config.compression.algorithm if hasattr(self.config, 'compression') else 'gzip'
        )
    
    def _create_rest_client(self):
        """Create REST client with enhanced configuration"""
        from .protocols.rest_sync import ProximaDBClient as RestClient
        
        # Add retry configuration if not present
        if not hasattr(self.config, 'retry'):
            from dataclasses import dataclass
            @dataclass
            class RetryConfig:
                max_retries: int = 3
                backoff_factor: float = 0.5
                max_backoff: float = 10.0
            self.config.retry = RetryConfig()
        
        return RestClient(config=self.config)
    
    @property
    def active_protocol(self) -> Protocol:
        """Get the currently active protocol"""
        return self._active_protocol
    
    def get_performance_info(self) -> Dict[str, Any]:
        """Get performance information about the active protocol"""
        if self._active_protocol == Protocol.GRPC:
            return {
                "protocol": "gRPC",
                "advantages": [
                    "40% smaller payloads (binary protobuf vs JSON)",
                    "90% less overhead (HTTP/2 vs HTTP/1.1)",
                    "Better type safety with schema evolution",
                    "Streaming support for real-time operations"
                ],
                "serialization": "Binary Protocol Buffers",
                "transport": "HTTP/2"
            }
        else:
            return {
                "protocol": "REST",
                "advantages": [
                    "Universal compatibility",
                    "Easy debugging with standard tools",
                    "Human-readable JSON format"
                ],
                "serialization": "JSON",
                "transport": "HTTP/1.1"
            }
    
    # Intelligent protocol selection methods (Phase 2 optimization)
    
    def get_protocol_metrics(self) -> Dict[str, Any]:
        """Get detailed metrics for all available protocols"""
        if self._protocol_selector:
            return self._protocol_selector.get_protocol_metrics()
        else:
            return {"error": "Intelligent protocol selection not enabled"}
    
    # Operation-specific routing methods (Phase 3 optimization)
    
    def _get_client_for_operation(
        self, 
        operation_name: str, 
        data_size_hint: Optional[int] = None,
        context: Optional[Dict[str, Any]] = None,
        preferred_protocol: Optional[Protocol] = None
    ) -> Any:
        """Get appropriate client for specific operation"""
        if not self.enable_operation_routing or not self._operation_router:
            # Fallback to default client selection
            return self._client
        
        # Route operation to appropriate protocol
        selected_protocol = self._operation_router.route_operation(
            operation_name=operation_name,
            data_size_hint=data_size_hint,
            context=context,
            preferred_protocol=preferred_protocol
        )
        
        # Return appropriate client
        if selected_protocol == Protocol.GRPC and self._grpc_client:
            return self._grpc_client
        elif selected_protocol == Protocol.REST and self._rest_client:
            return self._rest_client
        else:
            # Fallback to default client
            logger.warning(f"Requested protocol {selected_protocol.value} not available, using default")
            return self._client
    
    def _record_operation_result(
        self,
        operation_name: str,
        protocol: Protocol,
        success: bool,
        response_time_ms: float,
        error: Optional[str] = None,
        throughput_ops_per_sec: float = 0.0
    ):
        """Record operation result for adaptive routing"""
        if self._operation_router:
            self._operation_router.record_operation_result(
                protocol=protocol,
                success=success,
                response_time_ms=response_time_ms,
                error=error,
                throughput_ops_per_sec=throughput_ops_per_sec
            )
    
    def get_routing_stats(self) -> Dict[str, Any]:
        """Get operation routing statistics"""
        if self._operation_router:
            return self._operation_router.get_routing_stats()
        else:
            return {"error": "Operation routing not enabled"}
    
    def add_routing_rule(self, rule) -> None:
        """Add custom routing rule"""
        if self._operation_router:
            self._operation_router.add_routing_rule(rule)
        else:
            logger.warning("Operation routing not enabled, cannot add routing rule")
    
    def reset_routing_metrics(self) -> None:
        """Reset routing performance metrics"""
        if self._operation_router:
            self._operation_router.reset_metrics()
        else:
            logger.warning("Operation routing not enabled, cannot reset metrics")
    
    def get_selection_stats(self) -> Dict[str, Any]:
        """Get protocol selection statistics"""
        if self._protocol_selector:
            return self._protocol_selector.get_selection_stats()
        else:
            return {"error": "Intelligent protocol selection not enabled"}
    
    def force_protocol_switch(self, target_protocol: Protocol):
        """Force switch to specific protocol (for testing/debugging)"""
        if self._protocol_selector:
            self._protocol_selector.force_protocol_switch(target_protocol)
            # Update client reference
            self._client = self._protocol_selector.get_client(target_protocol)
            self._active_protocol = target_protocol
        else:
            raise ProximaDBError("Intelligent protocol selection not enabled")
    
    
    def _get_optimal_client(self, operation_hint: Optional[str] = None):
        """Get optimal client for operation (with intelligent selection)"""
        if self._protocol_selector:
            # Get optimal protocol for this operation
            optimal_protocol = self._protocol_selector.select_protocol(operation_hint)
            
            # Switch if different from current
            if optimal_protocol != self._active_protocol:
                self._client = self._protocol_selector.get_client(optimal_protocol)
                self._active_protocol = optimal_protocol
                logger.debug(f"Switched to {optimal_protocol.value} for {operation_hint or 'operation'}")
        
        return self._client
    
    # Type conversion helpers
    def _proto_to_pydantic_collection(self, proto_collection: 'pb2.Collection') -> Collection:
        """Convert proto Collection to Pydantic Collection"""
        config = CollectionConfig(
            name=proto_collection.config.name,
            dimension=proto_collection.config.dimension,
            distance_metric=self._proto_to_pydantic_distance_metric(proto_collection.config.distance_metric),
            storage_engine=self._proto_to_pydantic_storage_engine(proto_collection.config.storage_engine),
            storage_config=proto_collection.config.storage_config if proto_collection.config.HasField('storage_config') else None,
            quantization=proto_collection.config.quantization if proto_collection.config.HasField('quantization') else None,
            primary_index=proto_collection.config.primary_index if proto_collection.config.primary_index else None,
            auto_index_selection=proto_collection.config.auto_index_selection if proto_collection.config.auto_index_selection else None,
            description=proto_collection.config.description if proto_collection.config.description else None,
            tags=list(proto_collection.config.tags) if proto_collection.config.tags else None,
            owner=proto_collection.config.owner if proto_collection.config.owner else None,
        )
        
        return Collection(
            id=proto_collection.id,
            config=config,
            created_at=proto_collection.created_at,
            updated_at=proto_collection.updated_at,
        )
    
    def _proto_to_pydantic_distance_metric(self, proto_metric: int) -> DistanceMetric:
        """Convert proto DistanceMetric to Pydantic DistanceMetric"""
        mapping = {
            1: DistanceMetric.COSINE,
            2: DistanceMetric.EUCLIDEAN,
            3: DistanceMetric.DOT_PRODUCT,
            4: DistanceMetric.HAMMING,
            5: DistanceMetric.MANHATTAN,
            6: DistanceMetric.JACCARD,
            7: DistanceMetric.CUSTOM,
            8: DistanceMetric.CHEBYSHEV,
            9: DistanceMetric.CANBERRA,
            10: DistanceMetric.MINKOWSKI,
            11: DistanceMetric.ANGULAR,
            12: DistanceMetric.BRAY_CURTIS,
            13: DistanceMetric.HELLINGER,
        }
        return mapping.get(proto_metric, DistanceMetric.COSINE)
    
    def _proto_to_pydantic_storage_engine(self, proto_engine: int) -> StorageEngine:
        """Convert proto StorageEngine to Pydantic StorageEngine"""
        mapping = {
            1: StorageEngine.VIPER,
            2: StorageEngine.SST,
            3: StorageEngine.MMAP,
            4: StorageEngine.HYBRID,
        }
        return mapping.get(proto_engine, StorageEngine.VIPER)
    
    def _proto_to_pydantic_indexing_algorithm(self, proto_algo: int) -> IndexingAlgorithm:
        """Convert proto IndexingAlgorithm to Pydantic IndexingAlgorithm"""
        mapping = {
            1: IndexingAlgorithm.HNSW,
            2: IndexingAlgorithm.IVF,
            3: IndexingAlgorithm.PQ,
            4: IndexingAlgorithm.FLAT,
            5: IndexingAlgorithm.ANNOY,
            6: IndexingAlgorithm.LSH,
        }
        return mapping.get(proto_algo, IndexingAlgorithm.HNSW)
    
    def _pydantic_to_proto_collection_config(self, config: CollectionConfig) -> 'pb2.CollectionConfig':
        """Convert Pydantic CollectionConfig to proto CollectionConfig"""
        proto_config = pb2.CollectionConfig(
            name=config.name,
            dimension=config.dimension,
            distance_metric=self._pydantic_to_proto_distance_metric(config.distance_metric),
            storage_engine=self._pydantic_to_proto_storage_engine(config.storage_engine),
        )
        
        if config.description:
            proto_config.description = config.description
        if config.tags:
            proto_config.tags.extend(config.tags)
        if config.owner:
            proto_config.owner = config.owner
        
        # Handle new field names
        if config.primary_index:
            proto_config.primary_index = config.primary_index
        if config.auto_index_selection is not None:
            proto_config.auto_index_selection = config.auto_index_selection
        
        # Handle quantization config (renamed field)
        if config.quantization:
            proto_config.quantization.CopyFrom(
                self._pydantic_to_proto_quantization_config(config.quantization)
            )
        
        # Handle storage config
        if config.storage_config:
            proto_config.storage_config.CopyFrom(config.storage_config)
        
        return proto_config
    
    def _pydantic_to_proto_distance_metric(self, metric: DistanceMetric) -> int:
        """Convert Pydantic DistanceMetric to proto DistanceMetric"""
        mapping = {
            DistanceMetric.COSINE: pb2.DistanceMetric.COSINE,
            DistanceMetric.EUCLIDEAN: pb2.DistanceMetric.EUCLIDEAN,
            DistanceMetric.DOT_PRODUCT: pb2.DistanceMetric.DOT_PRODUCT,
            DistanceMetric.HAMMING: pb2.DistanceMetric.HAMMING,
            DistanceMetric.MANHATTAN: pb2.DistanceMetric.MANHATTAN,
            DistanceMetric.JACCARD: pb2.DistanceMetric.JACCARD,
            DistanceMetric.CUSTOM: pb2.DistanceMetric.CUSTOM,
            DistanceMetric.CHEBYSHEV: pb2.DistanceMetric.CHEBYSHEV,
            DistanceMetric.CANBERRA: pb2.DistanceMetric.CANBERRA,
            DistanceMetric.MINKOWSKI: pb2.DistanceMetric.MINKOWSKI,
            DistanceMetric.ANGULAR: pb2.DistanceMetric.ANGULAR,
            DistanceMetric.BRAY_CURTIS: pb2.DistanceMetric.BRAY_CURTIS,
            DistanceMetric.HELLINGER: pb2.DistanceMetric.HELLINGER,
        }
        return mapping.get(metric, pb2.DistanceMetric.COSINE)
    
    def _pydantic_to_proto_storage_engine(self, engine: StorageEngine) -> int:
        """Convert Pydantic StorageEngine to proto StorageEngine"""
        mapping = {
            StorageEngine.VIPER: pb2.StorageEngine.VIPER,
            StorageEngine.SST: pb2.StorageEngine.SST,
            StorageEngine.MMAP: pb2.StorageEngine.MMAP,
            StorageEngine.HYBRID: pb2.StorageEngine.HYBRID,
        }
        return mapping.get(engine, pb2.StorageEngine.VIPER)
    
    def _pydantic_to_proto_indexing_algorithm(self, algo: IndexingAlgorithm) -> int:
        """Convert Pydantic IndexingAlgorithm to proto IndexingAlgorithm"""
        mapping = {
            IndexingAlgorithm.HNSW: pb2.IndexingAlgorithm.HNSW,
            IndexingAlgorithm.IVF: pb2.IndexingAlgorithm.IVF,
            IndexingAlgorithm.PQ: pb2.IndexingAlgorithm.PQ,
            IndexingAlgorithm.FLAT: pb2.IndexingAlgorithm.FLAT,
            IndexingAlgorithm.ANNOY: pb2.IndexingAlgorithm.ANNOY,
            IndexingAlgorithm.LSH: pb2.IndexingAlgorithm.LSH,
        }
        return mapping.get(algo, pb2.IndexingAlgorithm.HNSW)
    
    def _pydantic_to_proto_quantization_config(self, config: QuantizationConfig) -> 'pb2.QuantizationConfig':
        """Convert Pydantic QuantizationConfig to proto QuantizationConfig"""
        proto_config = pb2.QuantizationConfig(enabled=config.enabled)
        
        # For simple quantization config, we need to map to the comprehensive proto structure
        if config.enabled and config.type != QuantizationType.NONE:
            # Create a search quantization config based on the simple config
            search_config = pb2.SearchQuantizationConfig(
                enabled=True,
                adaptive_precision=True,
                accuracy_threshold=config.accuracy_threshold or 0.95,
                candidate_multiplier=3
            )
            
            # Create the appropriate quantization level
            level = pb2.QuantizationLevel()
            if config.type == QuantizationType.BINARY:
                level.binary.CopyFrom(pb2.BinaryQuantization(
                    threshold=config.threshold or 0.0,
                    sign_based=True
                ))
            elif config.type == QuantizationType.SCALAR:
                level.scalar.CopyFrom(pb2.ScalarQuantization(
                    bits=config.bits_per_vector or 8,
                    scale=1.0,
                    offset=0.0,
                    clamp_values=True
                ))
            elif config.type == QuantizationType.PRODUCT:
                level.pq.CopyFrom(pb2.ProductQuantization(
                    bits_per_code=config.bits_per_subvector or 8,
                    num_subvectors=config.num_subvectors or 8,
                    adaptive_subvectors=True
                ))
            elif config.type == QuantizationType.UNIFORM:
                level.uniform.CopyFrom(pb2.UniformQuantization(
                    bits=8,
                    scale=1.0,
                    offset=0.0
                ))
            
            search_config.default_level.CopyFrom(level)
            proto_config.search_quantization.CopyFrom(search_config)
            
            if config.compression_ratio_target:
                proto_config.compression_ratio_target = config.compression_ratio_target
        
        return proto_config
    
    def _proto_to_pydantic_health_status(self, proto_health: 'pb2.HealthResponse') -> HealthStatus:
        """Convert proto HealthResponse to Pydantic HealthStatus"""
        return HealthStatus(
            status=proto_health.status,
            version=proto_health.version,
            uptime_seconds=proto_health.uptime_seconds,
            services={},  # gRPC health doesn't include services info
            timestamp=int(time.time() * 1000000)  # Current timestamp in microseconds
        )
    
    # Public API methods
    def health(self) -> HealthStatus:
        """Check server health status"""
        if self._active_protocol == Protocol.GRPC:
            proto_health = self._client.health_check()
            return self._proto_to_pydantic_health_status(proto_health)
        else:
            return self._client.health()
    
    def create_collection(
        self,
        name: str,
        config: Optional[CollectionConfig] = None,
        **kwargs
    ) -> Collection:
        """Create a new vector collection with optional storage engine configuration
        
        Args:
            name: Collection name
            config: Full collection configuration including storage_engine_config
            **kwargs: Additional configuration parameters
            
        Examples:
            # Simple collection with defaults
            client.create_collection("my_vectors", dimension=768)
            
            # Collection with storage optimization hints
            from proximadb.models import CollectionConfig, StorageEngineConfig, AccessPattern
            config = CollectionConfig(
                name="optimized_vectors",
                dimension=768,
                storage_engine_config=StorageEngineConfig(
                    access_pattern=AccessPattern.READ_HEAVY,
                    expected_size_gb=100,
                    preset="cloud_optimized"
                )
            )
            client.create_collection(config=config)
            
            # Collection with specific Parquet settings
            config = CollectionConfig(
                name="custom_vectors",
                dimension=768,
                storage_engine_config=StorageEngineConfig(
                    enable_all_optimizations=True,
                    parquet_writer=ParquetWriterSettings(
                        enable_bloom_filters=True,
                        enable_pq_sorting=True,
                        row_group_size=50000
                    )
                )
            )
            client.create_collection(config=config)
        """
        if config is None:
            config = CollectionConfig(name=name, **kwargs)
        
        if self._active_protocol == Protocol.GRPC:
            proto_config = self._pydantic_to_proto_collection_config(config)
            # Note: gRPC client expects individual parameters, not the full config
            # Compression and storage_engine_config are embedded in the proto_config
            # and passed through the collection metadata on the server side
            # Build optional IndexConfig if primary_indexing_algorithm is set
            index_configs = []
            if getattr(config, 'primary_indexing_algorithm', None):
                index_configs.append(
                    pb2.IndexConfig(
                        index_name=f"{config.name}_primary",
                        algorithm=self._pydantic_to_proto_indexing_algorithm(config.primary_indexing_algorithm),
                        is_primary=True,
                    )
                )
            # Quantization config (converted to proto)
            qcfg = None
            if getattr(config, 'quantization', None):
                qcfg = self._pydantic_to_proto_quantization_config(config.quantization)

            response = self._client.create_collection(
                name=config.name,
                dimension=config.dimension,
                distance_metric=self._pydantic_to_proto_distance_metric(config.distance_metric),
                indexing_algorithm=self._pydantic_to_proto_indexing_algorithm(getattr(config, 'primary_indexing_algorithm', None)) if getattr(config, 'primary_indexing_algorithm', None) else None,
                storage_engine=self._pydantic_to_proto_storage_engine(config.storage_engine),
                index_configs=index_configs,
                quantization_config=qcfg,
            )
            # Handle VectorOperationResponse
            if hasattr(response, 'collection') and response.collection:
                return self._proto_to_pydantic_collection(response.collection)
            else:
                # Return a simple collection object if successful
                return Collection(
                    id=response.collection.id if hasattr(response, 'collection') else config.name,
                    config=config,
                    created_at=int(time.time() * 1e6),
                    updated_at=int(time.time() * 1e6)
                )
        else:
            return self._client.create_collection(name, config, **kwargs)
    
    def get_collection(self, collection_id: str) -> Optional[Collection]:
        """Get collection metadata"""
        if self._active_protocol == Protocol.GRPC:
            proto_collection = self._client.get_collection(collection_id)
            if proto_collection:
                return self._proto_to_pydantic_collection(proto_collection)
            return None
        else:
            return self._client.get_collection(collection_id)
    
    def list_collections(self) -> List[Collection]:
        """List all collections"""
        operation_name = "list_collections"
        start_time = time.time()
        
        try:
            # Get appropriate client for this operation
            client = self._get_client_for_operation(operation_name)
            
            # Determine which protocol we're using
            if client == self._grpc_client:
                protocol_used = Protocol.GRPC
                proto_collections = client.list_collections()
                result = [self._proto_to_pydantic_collection(col) for col in proto_collections]
            elif client == self._rest_client:
                protocol_used = Protocol.REST
                result = client.list_collections()
            else:
                # Fallback to active protocol
                protocol_used = self._active_protocol
                if protocol_used == Protocol.GRPC:
                    proto_collections = client.list_collections()
                    result = [self._proto_to_pydantic_collection(col) for col in proto_collections]
                else:
                    result = client.list_collections()
            
            # Record successful operation
            response_time = (time.time() - start_time) * 1000
            self._record_operation_result(operation_name, protocol_used, True, response_time)
            
            return result
            
        except Exception as e:
            # Record failed operation
            response_time = (time.time() - start_time) * 1000
            protocol_used = getattr(self, '_active_protocol', Protocol.REST)
            self._record_operation_result(operation_name, protocol_used, False, response_time, str(e))
            raise
    
    def delete_collection(self, collection_id: str) -> bool:
        """Delete a collection"""
        return self._client.delete_collection(collection_id)
    
    def insert_vectors(
        self,
        collection_id: str,
        # Backward compatibility: support old calling style
        vectors: Optional[Union[List[List[float]], List[VectorRecord], np.ndarray]] = None,
        ids: Optional[List[str]] = None,
        metadata: Optional[List[Dict[str, Any]]] = None,
        # New API parameter  
        records: Optional[List[VectorRecord]] = None,
        **kwargs
    ) -> VectorOperationResponse:
        """Insert vectors into a collection
        
        Supports both new API (VectorRecord objects) and old API (separate vectors/ids/metadata)
        
        Note: For quantized collections, all vectors MUST have unique IDs to track
        quantized representations across storage and indexes.
        """
        # Check if collection has quantization enabled
        try:
            collection = self.get_collection(collection_id)
            if collection and hasattr(collection, 'config'):
                config = collection.config
                if hasattr(config, 'quantization_config') and config.quantization_config:
                    if hasattr(config.quantization_config, 'enabled') and config.quantization_config.enabled:
                        # Quantization is enabled - validate IDs
                        needs_id_validation = True
                        logger.info(f"Collection '{collection_id}' has quantization enabled - validating vector IDs")
                    else:
                        needs_id_validation = False
                else:
                    needs_id_validation = False
            else:
                needs_id_validation = False
        except Exception as e:
            # If we can't check, proceed without validation
            logger.debug(f"Could not check quantization status for collection {collection_id}: {e}")
            needs_id_validation = False
        
        # Handle backward compatibility: convert old API to new API
        if vectors is not None:
            # Handle numpy arrays first
            if hasattr(vectors, 'tolist'):
                vectors = vectors.tolist()
            
            # Check if vectors is a list of VectorRecord objects (new API called with vectors param)
            if (hasattr(vectors, '__len__') and len(vectors) > 0 and 
                hasattr(vectors[0], 'vector') and hasattr(vectors[0], 'id')):
                records = vectors
            else:
                # Old API: convert vectors/ids/metadata to VectorRecord objects
                records = []
                
                for i, vector in enumerate(vectors):
                    record = VectorRecord(
                        id=ids[i] if ids and i < len(ids) else None,
                        vector=vector if isinstance(vector, list) else vector.tolist() if hasattr(vector, 'tolist') else list(vector),
                        metadata=metadata[i] if metadata and i < len(metadata) else {}
                    )
                    records.append(record)
        elif records is None:
            # Neither vectors nor records provided
            pass
        
        # Handle numpy arrays and other array-like objects
        if records is None or (hasattr(records, '__len__') and len(records) == 0) or (not hasattr(records, '__len__') and not records):
            raise ValueError("Either 'records' or 'vectors' must be provided")
        
        # Validate IDs for quantized collections
        if needs_id_validation:
            for i, record in enumerate(records):
                if not record.id or record.id.strip() == "":
                    raise ValueError(
                        f"Vector at index {i} missing ID. "
                        f"Collection '{collection_id}' has quantization enabled. "
                        f"All vectors MUST have unique IDs for tracking quantized representations."
                    )
            logger.debug(f"✅ ID validation passed for {len(records)} vectors in quantized collection {collection_id}")
        
        # Estimate data size for routing
        data_size_hint = len(records) * len(records[0].vector) * 4 if records and records[0].vector else 1000  # Rough estimate
        operation_name = "bulk_insert_vectors" if len(records) > 10 else "insert_vectors"
        
        start_time = time.time()
        
        try:
            # Get appropriate client for this operation
            client = self._get_client_for_operation(
                operation_name=operation_name,
                data_size_hint=data_size_hint,
                context={"collection_id": collection_id, "vector_count": len(records)}
            )
            
            # Determine protocol and execute
            if client == self._grpc_client:
                protocol_used = Protocol.GRPC
                # Convert Pydantic VectorRecord to dict format for gRPC client
                vector_dicts = []
                for record in records:
                    vector_dict = {
                        "vector": record.vector,
                        "metadata": record.metadata
                    }
                    if record.id:
                        vector_dict["id"] = record.id
                    if record.timestamp:
                        vector_dict["timestamp"] = record.timestamp
                    if record.expires_at:
                        vector_dict["expires_at"] = record.expires_at
                    vector_dicts.append(vector_dict)
                
                proto_response = client.insert_vectors(collection_id, vector_dicts)
                # Convert proto response to Pydantic (simplified for now)
                # Handle case where metrics might not be present in the response
                metrics = None
                if hasattr(proto_response, 'metrics') and proto_response.metrics:
                    metrics = OperationMetrics(
                        total_processed=proto_response.metrics.total_processed,
                        successful_count=proto_response.metrics.successful_count,
                        failed_count=proto_response.metrics.failed_count
                    )
                else:
                    # Default metrics if not provided
                    metrics = OperationMetrics(
                        total_processed=len(vector_dicts),
                        successful_count=len(vector_dicts) if proto_response.success else 0,
                        failed_count=0 if proto_response.success else len(vector_dicts)
                    )
                
                result = VectorOperationResponse(
                    success=proto_response.success,
                    operation="insert",
                    metrics=metrics
                )
                
            elif client == self._rest_client:
                protocol_used = Protocol.REST
                # REST client expects separate arrays
                vectors = [r.vector for r in records]
                ids = [r.id for r in records if r.id]
                metadata = [r.metadata for r in records]
                
                # If no IDs provided, generate them
                if not ids:
                    ids = [f"vec_{i}" for i in range(len(vectors))]
                
                result = client.insert_vectors(collection_id, vectors, ids, metadata)
                
            else:
                # Fallback to active protocol
                protocol_used = self._active_protocol
                if protocol_used == Protocol.GRPC:
                    # Similar to gRPC path above
                    vector_dicts = []
                    for record in records:
                        vector_dict = {
                            "vector": record.vector,
                            "metadata": record.metadata
                        }
                        if record.id:
                            vector_dict["id"] = record.id
                        if record.timestamp:
                            vector_dict["timestamp"] = record.timestamp
                        if record.expires_at:
                            vector_dict["expires_at"] = record.expires_at
                        vector_dicts.append(vector_dict)
                    
                    proto_response = client.insert_vectors(collection_id, vector_dicts)
                    metrics = None
                    if hasattr(proto_response, 'metrics') and proto_response.metrics:
                        metrics = OperationMetrics(
                            total_processed=proto_response.metrics.total_processed,
                            successful_count=proto_response.metrics.successful_count,
                            failed_count=proto_response.metrics.failed_count
                        )
                    else:
                        metrics = OperationMetrics(
                            total_processed=len(vector_dicts),
                            successful_count=len(vector_dicts) if proto_response.success else 0,
                            failed_count=0 if proto_response.success else len(vector_dicts)
                        )
                    
                    result = VectorOperationResponse(
                        success=proto_response.success,
                        operation="insert", 
                        metrics=metrics
                    )
                else:
                    # REST fallback
                    vectors = [r.vector for r in records]
                    ids = [r.id for r in records if r.id]
                    metadata = [r.metadata for r in records]
                    
                    if not ids:
                        ids = [f"vec_{i}" for i in range(len(vectors))]
                    
                    result = client.insert_vectors(collection_id, vectors, ids, metadata)
            
            # Record successful operation
            response_time = (time.time() - start_time) * 1000
            throughput = len(records) / ((time.time() - start_time) + 0.001)  # Add small value to avoid division by zero
            self._record_operation_result(operation_name, protocol_used, True, response_time, throughput_ops_per_sec=throughput)
            
            return result
            
        except Exception as e:
            # Record failed operation
            response_time = (time.time() - start_time) * 1000
            protocol_used = getattr(self, '_active_protocol', Protocol.REST)
            self._record_operation_result(operation_name, protocol_used, False, response_time, str(e))
            raise
    
    def upsert_vectors(
        self,
        collection_id: str,
        records: List[VectorRecord]
    ) -> VectorOperationResponse:
        """Upsert vectors into a collection"""
        if self._active_protocol == Protocol.GRPC:
            # Convert Pydantic VectorRecord to dict format for gRPC client
            vector_dicts = []
            for record in records:
                vector_dict = {
                    "vector": record.vector,
                    "metadata": record.metadata
                }
                if record.id:
                    vector_dict["id"] = record.id
                if record.timestamp:
                    vector_dict["timestamp"] = record.timestamp
                if record.expires_at:
                    vector_dict["expires_at"] = record.expires_at
                vector_dicts.append(vector_dict)
            
            proto_response = self._client.insert_vectors(collection_id, vector_dicts, upsert=True)
            # Convert proto response to Pydantic (simplified for now)
            # Handle case where metrics might not be present in the response
            metrics = None
            if hasattr(proto_response, 'metrics') and proto_response.metrics:
                metrics = OperationMetrics(
                    total_processed=proto_response.metrics.total_processed,
                    successful_count=proto_response.metrics.successful_count,
                    failed_count=proto_response.metrics.failed_count,
                    updated_count=proto_response.metrics.updated_count if hasattr(proto_response.metrics, 'updated_count') else 0
                )
            else:
                # Default metrics if not provided
                metrics = OperationMetrics(
                    total_processed=len(vector_dicts),
                    successful_count=len(vector_dicts) if proto_response.success else 0,
                    failed_count=0 if proto_response.success else len(vector_dicts),
                    updated_count=len(vector_dicts) if proto_response.success else 0
                )
            
            return VectorOperationResponse(
                success=proto_response.success,
                operation="upsert",
                metrics=metrics
            )
        else:
            return self._client.upsert_vectors(collection_id, records)
    
    def search(
        self,
        collection_id: str,
        vector: Union[List[float], np.ndarray],
        top_k: int = 10,
        metadata_filter: Optional[Union[Dict[str, Any], 'FilterBuilder']] = None,
        include_metadata: bool = True,
        include_vectors: bool = False,
        **kwargs
    ) -> List[SearchResult]:
        """Search for similar vectors
        
        Args:
            collection_id: Target collection ID
            vector: Query vector
            top_k: Number of results to return
            metadata_filter: Optional metadata filter
            include_metadata: Include metadata in results
            include_vectors: Include vectors in results
            **kwargs: Additional search parameters
            
        Returns:
            List of search results ordered by similarity
        """
        # Validate top_k
        if top_k <= 0:
            raise ProximaDBError(f"top_k must be positive, got {top_k}")
        
        return self.search_single(
            collection_id=collection_id,
            vector=vector,
            top_k=top_k,
            metadata_filter=metadata_filter,
            include_metadata=include_metadata,
            include_vectors=include_vectors,
            **kwargs
        )

    def search_single(
        self,
        collection_id: str,
        vector: Union[List[float], np.ndarray],
        top_k: int = 10,
        metadata_filter: Optional[Union[Dict[str, Any], 'FilterBuilder']] = None,
        optimization_level: str = "high",
        use_storage_aware: bool = True,
        quantization_level: str = "FP32",
        enable_simd: bool = True,
        **kwargs
    ) -> List[SearchResult]:
        """Search for similar vectors with storage-aware optimizations
        
        Args:
            collection_id: Target collection ID
            vector: Query vector
            top_k: Number of results to return
            metadata_filter: Metadata filter conditions
            optimization_level: Search optimization level ('high', 'medium', 'low')
            use_storage_aware: Enable storage-aware polymorphic search
            quantization_level: Vector quantization level for search
            enable_simd: Enable SIMD vectorization optimizations
            **kwargs: Additional search parameters
            
        Returns:
            List of search results ordered by similarity
        """
        if self._active_protocol == Protocol.GRPC:
            # Convert vector to list if numpy array
            if isinstance(vector, np.ndarray):
                vector = vector.tolist()
            
            # Add search hints for gRPC
            search_hints = kwargs.get('search_hints', {})
            search_hints.update({
                "predicate_pushdown": True,
                "use_bloom_filters": True,
                "use_clustering": True,
                "quantization_level": quantization_level,
                "parallel_search": True,
                "engine_specific": {
                    "optimization_level": optimization_level,
                    "enable_simd": enable_simd,
                    "prefer_indices": True,
                    "storage_aware": use_storage_aware
                }
            })
            
            # grpc_sync.search_vectors already returns List[SearchResult]
            results = self._client.search_vectors(
                collection_id=collection_id,
                query_vectors=[vector],
                top_k=top_k,
                metadata_filters=metadata_filter,
                include_metadata=kwargs.get('include_metadata', True),
                include_vectors=kwargs.get('include_vectors', False)
                # Note: search_hints would need to be converted to SearchParameters proto
            )
            
            # Results are already SearchResult objects, just return them
            return results
        else:
            # For REST, use search method (filter out unsupported parameters)
            # Remove optimization_hints and other parameters not supported by REST client
            filtered_kwargs = {k: v for k, v in kwargs.items() 
                             if k not in {'optimization_hints', 'enable_two_stage_search', 
                                         'quantization_hint', 'candidate_multiplier',
                                         'enable_parallel_search'}}
            
            return self._client.search(
                collection_id=collection_id,
                vector=vector,
                top_k=top_k,
                metadata_filter=metadata_filter,
                optimization_level=optimization_level,
                use_storage_aware=use_storage_aware,
                quantization_level=quantization_level,
                enable_simd=enable_simd,
                **filtered_kwargs
            )

    def search_iter(
        self,
        collection_id: str,
        vector: Union[List[float], np.ndarray],
        top_k: int = 10,
        include_metadata: bool = True,
        include_vectors: bool = False,
        page_limit: Optional[int] = None,
    ):
        """Iterate across pages of search results. Uses SKS cursors on REST when available.

        For gRPC (no pagination), yields only the first page of results.
        """
        if self._active_protocol == Protocol.REST and hasattr(self._client, 'search_envelope'):
            env = self._client.search_envelope(
                collection_id=collection_id,
                vector=vector,
                top_k=top_k,
                include_vectors=include_vectors,
                include_metadata=include_metadata,
            )
            count = 0
            for item in env.items:
                yield item
                count += 1
            cursor = env.cursor
            pages = 1
            while env.has_more and cursor and (page_limit is None or pages < page_limit):
                env = self._client.search_next_page(collection_id, cursor, include_vectors=include_vectors, include_metadata=include_metadata)
                for item in env.items:
                    yield item
                    count += 1
                cursor = env.cursor
                pages += 1
        else:
            # gRPC: yield single page
            for item in self.search_single(
                collection_id=collection_id,
                vector=vector,
                top_k=top_k,
                include_metadata=include_metadata,
                include_vectors=include_vectors,
            ):
                yield item
    
    # The generic search method is defined above and forwards to search_single
    
    def search_batch(
        self,
        collection_id: str,
        vectors: Union[List[List[float]], np.ndarray],
        top_k: int = 10,
        metadata_filter: Optional[Union[Dict[str, Any], 'FilterBuilder']] = None,
        **kwargs
    ) -> List[List[SearchResult]]:
        """Search multiple queries in batch with optimizations
        
        Args:
            collection_id: Target collection ID
            vectors: Array of query vectors
            top_k: Number of results per query
            metadata_filter: Metadata filter conditions
            **kwargs: Additional search parameters
            
        Returns:
            List of search results for each query
        """
        # Convert to list if numpy array
        if isinstance(vectors, np.ndarray):
            vectors = vectors.tolist()
        
        # Perform batch search
        all_results = []
        for vector in vectors:
            results = self.search_single(
                collection_id=collection_id,
                vector=vector,
                top_k=top_k,
                metadata_filter=metadata_filter,
                **kwargs
            )
            all_results.append(results)
        
        return all_results
    
    def delete_vectors(
        self,
        collection_id: str,
        vector_ids: List[str]
    ) -> VectorOperationResponse:
        """Delete vectors from a collection"""
        if self._active_protocol == Protocol.GRPC:
            proto_response = self._client.delete_vectors(collection_id, vector_ids)
            # Handle case where proto_response is a dict or object
            if isinstance(proto_response, dict):
                success = proto_response.get('success', True)
                metrics_data = proto_response.get('metrics', {})
                metrics = OperationMetrics(
                    total_processed=metrics_data.get('total_processed', len(vector_ids)),
                    successful_count=metrics_data.get('successful_count', len(vector_ids) if success else 0),
                    failed_count=metrics_data.get('failed_count', 0 if success else len(vector_ids))
                )
            else:
                # Handle case where metrics might not be present in the response
                metrics = None
                if hasattr(proto_response, 'metrics') and proto_response.metrics:
                    metrics = OperationMetrics(
                        total_processed=proto_response.metrics.total_processed,
                        successful_count=proto_response.metrics.successful_count,
                        failed_count=proto_response.metrics.failed_count
                    )
                else:
                    # Default metrics if not provided
                    metrics = OperationMetrics(
                        total_processed=len(vector_ids),
                        successful_count=len(vector_ids) if proto_response.success else 0,
                        failed_count=0 if proto_response.success else len(vector_ids)
                    )
                success = proto_response.success
            
            return VectorOperationResponse(
                success=success,
                operation="delete",
                metrics=metrics
            )
        else:
            return self._client.delete_vectors(collection_id, vector_ids)
    
    def get_vector(
        self,
        collection_id: str,
        vector_id: str,
        include_vector: bool = True,
        include_metadata: bool = True,
    ):  # Changed return type to be flexible
        """Get a single vector by ID"""
        if self._active_protocol == Protocol.GRPC:
            result = self._client.get_vector(collection_id, vector_id, include_vector, include_metadata)
            return result  # Return dict directly to avoid pydantic validation issues
        else:
            result = self._client.get_vector(collection_id, vector_id, include_vector, include_metadata)
            return result
    
    def insert_vector(
        self,
        collection_id: str,
        vector_id: str,
        vector: Union[List[float], np.ndarray],
        metadata: Optional[Dict[str, Any]] = None,
        upsert: bool = False
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
        record = VectorRecord(
            id=vector_id,
            vector=vector,
            metadata=metadata or {}
        )
        if upsert:
            return self.upsert_vectors(collection_id, [record])
        else:
            return self.insert_vectors(collection_id, [record])
    
    def delete_vector(
        self,
        collection_id: str,
        vector_id: str
    ) -> VectorOperationResponse:
        """Delete a single vector - alias for batch delete with one vector
        
        Args:
            collection_id: Collection ID or name
            vector_id: Vector identifier to delete
            
        Returns:
            VectorOperationResponse
        """
        return self.delete_vectors(collection_id, [vector_id])
    
    def execute_sql(
        self,
        query: str,
        parameters: Optional[List[Any]] = None,
        collection: Optional[str] = None
    ) -> Dict[str, Any]:
        """Execute SQL query with vector similarity support
        
        Args:
            query: SQL query string (e.g., "SELECT * FROM my_collection ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, ...], 'cosine') LIMIT 10")
            parameters: Optional query parameters (not yet supported)
            collection: Optional collection hint (if not specified in FROM clause)
            
        Returns:
            Dict with 'rows', 'columns', and 'row_count' keys
            
        Example:
            >>> result = client.execute_sql(
            ...     "SELECT id, metadata FROM my_collection WHERE metadata.category = 'electronics' ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2], 'cosine') LIMIT 5"
            ... )
            >>> for row in result['rows']:
            ...     print(row['id'], row['metadata'])
        """
        if self._active_protocol == Protocol.GRPC:
            # gRPC doesn't support SQL yet, fall back to REST
            if hasattr(self._client, '_rest_url'):
                return self._execute_sql_rest(query, parameters, collection)
            else:
                raise ProximaDBError("SQL queries are only supported via REST API")
        else:
            # REST client
            return self._execute_sql_rest(query, parameters, collection)
    
    def _execute_sql_rest(
        self,
        query: str,
        parameters: Optional[List[Any]] = None,
        collection: Optional[str] = None
    ) -> Dict[str, Any]:
        """Execute SQL query via REST API"""
        # Build request payload
        payload = {
            "query": query
        }
        if parameters is not None:
            payload["parameters"] = parameters
        if collection is not None:
            payload["collection"] = collection
        
        # Make REST request
        if hasattr(self._client, '_session'):
            # Using REST client directly
            response = self._client._session.post(
                f"{self._client._base_url}/api/v1/sql/execute",
                json=payload
            )
            response.raise_for_status()
            return response.json()
        else:
            # Need to use requests directly
            import requests
            headers = {}
            if hasattr(self._client, '_api_key') and self._client._api_key:
                headers['X-API-Key'] = self._client._api_key
            
            base_url = getattr(self._client, '_rest_url', None) or getattr(self._client, '_base_url', 'http://localhost:5678')
            response = requests.post(
                f"{base_url}/api/v1/sql/execute",
                json=payload,
                headers=headers
            )
            if not response.ok:
                # Try to get error details from response
                try:
                    error_data = response.json()
                    error_msg = error_data.get('message', response.text)
                except:
                    error_msg = response.text
                raise Exception(f"SQL execution failed (HTTP {response.status_code}): {error_msg}")
            return response.json()
    
    def close(self):
        """Close the client and cleanup resources"""
        if self._client and hasattr(self._client, 'close'):
            self._client.close()
        
        # Close protocol selector if enabled
        if self._protocol_selector:
            self._protocol_selector.close()
            self._protocol_selector = None
    
    def __enter__(self):
        """Context manager entry"""
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        """Context manager exit"""
        self.close()
    
    def __del__(self):
        """Destructor - cleanup resources"""
        try:
            self.close()
        except Exception:
            pass
    
    # Legacy compatibility methods
    def insert(
        self,
        collection_id: str,
        vectors: Union[List[List[float]], np.ndarray],
        ids: Optional[List[str]] = None,
        metadata: Optional[List[Dict[str, Any]]] = None
    ) -> VectorOperationResponse:
        """Legacy insert method for backward compatibility"""
        records = []
        
        # Convert vectors to list format
        if isinstance(vectors, np.ndarray):
            vectors = vectors.tolist()
        
        # Build records
        for i, vector in enumerate(vectors):
            record = VectorRecord(
                vector=vector,
                id=ids[i] if ids and i < len(ids) else None,
                metadata=metadata[i] if metadata and i < len(metadata) else {}
            )
            records.append(record)
        
        return self.insert_vectors(collection_id, records)
    
    def upsert(
        self,
        collection_id: str,
        vectors: Union[List[List[float]], np.ndarray],
        ids: List[str],
        metadata: Optional[List[Dict[str, Any]]] = None
    ) -> VectorOperationResponse:
        """Legacy upsert method for backward compatibility"""
        records = []
        
        # Convert vectors to list format
        if isinstance(vectors, np.ndarray):
            vectors = vectors.tolist()
        
        # Build records
        for i, (vector, vector_id) in enumerate(zip(vectors, ids)):
            record = VectorRecord(
                vector=vector,
                id=vector_id,
                metadata=metadata[i] if metadata and i < len(metadata) else {}
            )
            records.append(record)
        
        return self.upsert_vectors(collection_id, records)
    
    def delete(
        self,
        collection_id: str,
        ids: List[str]
    ) -> VectorOperationResponse:
        """Legacy delete method for backward compatibility"""
        return self.delete_vectors(collection_id, ids)
    
    def get_collection_stats(self, collection_id: str) -> Dict[str, Any]:
        """Get collection statistics (legacy compatibility)"""
        collection = self.get_collection(collection_id)
        if collection:
            return {
                "id": collection.id,
                "name": collection.config.name,
                "dimension": collection.config.dimension,
                "created_at": collection.created_at,
                "updated_at": collection.updated_at,
                "vector_count": getattr(collection, 'vector_count', 0),
                "index_count": getattr(collection, 'index_count', 0),
                "status": getattr(collection, 'status', 'active')
            }
        return {}


# Convenience functions for backward compatibility
def connect(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    protocol: Union[Protocol, str] = Protocol.AUTO,
    **kwargs
) -> ProximaDBClient:
    """Create a ProximaDB client with simplified parameters"""
    return ProximaDBClient(url=url, api_key=api_key, protocol=protocol, **kwargs)


def connect_grpc(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    **kwargs
) -> ProximaDBClient:
    """Create a ProximaDB client using gRPC protocol (good performance, ecosystem compatibility)"""
    try:
        return ProximaDBClient(url=url, api_key=api_key, protocol=Protocol.GRPC, **kwargs)
    except ProximaDBError as e:
        # If gRPC fails due to import issues, fall back to AUTO (which will use REST)
        if "import" in str(e).lower() or "pb2" in str(e).lower():
            logger.warning(f"gRPC client failed due to import issues, falling back to AUTO mode: {e}")
            return ProximaDBClient(url=url, api_key=api_key, protocol=Protocol.AUTO, **kwargs)
        else:
            raise


def connect_rest(
    url: Optional[str] = None,
    api_key: Optional[str] = None,
    **kwargs
) -> ProximaDBClient:
    """Create a ProximaDB client using REST protocol (web compatibility)"""
    return ProximaDBClient(url=url, api_key=api_key, protocol=Protocol.REST, **kwargs)
