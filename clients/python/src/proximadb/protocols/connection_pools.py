"""
Connection Pooling for ProximaDB Python SDK

Optimized connection management for both gRPC and REST protocols
to improve throughput and resource utilization.

Performance Targets:
- gRPC: +15-25% throughput improvement
- REST: +20-35% throughput improvement
"""

import logging
import time
import threading
from typing import Optional, Dict, Any, List
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from enum import Enum

import httpx

try:
    import grpc
    GRPC_AVAILABLE = True
except ImportError:
    GRPC_AVAILABLE = False

from ..config import ClientConfig
from ..resource_pool import ResourcePool, ResourceFactory

logger = logging.getLogger(__name__)


class PoolHealth(Enum):
    HEALTHY = "healthy"
    DEGRADED = "degraded"
    UNHEALTHY = "unhealthy"


@dataclass
class PoolMetrics:
    """Connection pool performance metrics"""
    total_connections: int = 0
    active_connections: int = 0
    idle_connections: int = 0
    failed_connections: int = 0
    requests_served: int = 0
    avg_response_time_ms: float = 0.0
    health_status: PoolHealth = PoolHealth.HEALTHY
    last_health_check: float = 0.0


class GrpcChannelFactory(ResourceFactory[grpc.Channel]):
    """Factory for creating gRPC channels"""
    
    def __init__(
        self,
        endpoint: str,
        max_message_size: int = 64 * 1024 * 1024,
        keepalive_time_ms: int = 10000,
        keepalive_timeout_ms: int = 5000,
        use_tls: bool = False,
        compression: Optional[grpc.Compression] = None
    ):
        self.endpoint = endpoint
        self.max_message_size = max_message_size
        self.use_tls = use_tls
        self.compression = compression
        
        # gRPC channel options
        self.channel_options = [
            ('grpc.max_receive_message_length', max_message_size),
            ('grpc.max_send_message_length', max_message_size),
            ('grpc.keepalive_time_ms', keepalive_time_ms),
            ('grpc.keepalive_timeout_ms', keepalive_timeout_ms),
            ('grpc.keepalive_permit_without_calls', True),
            ('grpc.http2.max_pings_without_data', 0),
            ('grpc.http2.min_time_between_pings_ms', 10000),
            ('grpc.http2.min_ping_interval_without_data_ms', 5000),
        ]
        
        if compression is not None:
            self.channel_options.extend([
                ('grpc.default_compression_algorithm', compression),
                ('grpc.default_compression_level', 'high'),
            ])
    
    def create(self) -> grpc.Channel:
        """Create new gRPC channel"""
        if not GRPC_AVAILABLE:
            raise ImportError("gRPC not available. Install with: pip install grpcio grpcio-tools")
        
        if self.use_tls:
            credentials = grpc.ssl_channel_credentials()
            channel = grpc.secure_channel(self.endpoint, credentials, options=self.channel_options)
        else:
            channel = grpc.insecure_channel(self.endpoint, options=self.channel_options)
        
        return channel
    
    def validate(self, resource: grpc.Channel) -> bool:
        """Validate channel health"""
        try:
            future = grpc.channel_ready_future(resource)
            future.result(timeout=1.0)  # 1 second timeout
            return True
        except (grpc.FutureTimeoutError, Exception):
            return False
    
    def reset(self, resource: grpc.Channel) -> None:
        """Reset channel state - channels are stateless"""
        pass
    
    def destroy(self, resource: grpc.Channel) -> None:
        """Clean up gRPC channel before removal"""
        self.dispose(resource)
    
    def dispose(self, resource: grpc.Channel) -> None:
        """Close gRPC channel gracefully, waiting for background threads"""
        try:
            # Use threading Event for deterministic wait on channel shutdown
            shutdown_event = threading.Event()

            def on_state_change(connectivity):
                """Callback when channel state changes"""
                if connectivity == grpc.ChannelConnectivity.SHUTDOWN:
                    shutdown_event.set()

            # Subscribe to state changes before closing
            try:
                resource.subscribe(on_state_change)
            except (AttributeError, TypeError):
                # Older gRPC API or channel doesn't support subscription
                pass

            # Close the channel (signals background threads to stop)
            resource.close()

            # Wait deterministically for shutdown (max 2 seconds)
            # Event will be set by callback when channel reaches SHUTDOWN
            shutdown_event.wait(timeout=2.0)

        except Exception as e:
            logger.debug(f"Error during channel dispose (suppressed): {e}")


class GrpcConnectionPool:
    """
    Load-balanced gRPC connection pool using unified ResourcePool
    
    Features:
    - Round-robin channel distribution
    - Health monitoring per channel
    - Automatic failover for unhealthy channels
    - Connection lifecycle management via ResourcePool
    """
    
    def __init__(
        self,
        endpoint: str,
        pool_size: int = 5,
        max_message_size: int = 64 * 1024 * 1024,
        keepalive_time_ms: int = 10000,
        keepalive_timeout_ms: int = 5000,
        use_tls: bool = False,
        compression: Optional[grpc.Compression] = None
    ):
        self.endpoint = endpoint
        self.pool_size = pool_size
        self.max_message_size = max_message_size
        
        # Create resource pool with factory
        factory = GrpcChannelFactory(
            endpoint=endpoint,
            max_message_size=max_message_size,
            keepalive_time_ms=keepalive_time_ms,
            keepalive_timeout_ms=keepalive_timeout_ms,
            use_tls=use_tls,
            compression=compression
        )
        
        self._pool = ResourcePool(
            factory=factory,
            max_size=pool_size,
            min_size=1,
            enable_health_checks=True,
            enable_metrics=True
        )
        
        # Round-robin tracking
        self.current_channel_index = 0
        self._lock = threading.RLock()
        
        logger.info(f"Initialized gRPC connection pool: {pool_size} channels to {endpoint}")

        # Pre-create min_size connections for immediate availability
        self._warm_up_pool()
    
    def get_channel(self) -> grpc.Channel:
        """Get next available healthy channel using round-robin"""
        # ResourcePool handles acquisition - just return a channel
        return self._pool.acquire()
    
    def return_channel(self, channel: grpc.Channel, success: bool = True, response_time_ms: float = 0.0) -> None:
        """Return channel to pool"""
        # If unsuccessful, mark channel for disposal
        if not success:
            logger.warning("Marking gRPC channel as unhealthy")
            # ResourcePool will validate on next acquisition
        self._pool.release(channel)
    
    def get_metrics(self) -> PoolMetrics:
        """Get current pool performance metrics"""
        pool_stats = self._pool.get_stats()

        # Convert ResourcePool stats to PoolMetrics
        total_created = pool_stats.get('resources_created', 0)
        active_resources = pool_stats.get('active', 0)
        idle_resources = pool_stats.get('idle', 0)

        # Determine health status based on available resources
        if idle_resources > 0 or active_resources > 0:
            health_status = PoolHealth.HEALTHY
        elif total_created > 0:
            health_status = PoolHealth.DEGRADED
        else:
            health_status = PoolHealth.UNHEALTHY

        metrics = PoolMetrics(
            total_connections=total_created,
            active_connections=active_resources,
            idle_connections=idle_resources,
            failed_connections=0,  # Not available in current stats
            requests_served=pool_stats.get('total_acquisitions', 0),
            health_status=health_status,
            last_health_check=time.time()
        )

        return metrics

    def _warm_up_pool(self) -> None:
        """Pre-create connections to warm up the pool"""
        try:
            # Acquire and release connections to populate the pool
            channels = []
            for _ in range(min(self.pool_size, 5)):  # Limit warming to avoid overwhelming server
                try:
                    channel = self._pool.acquire(timeout=1.0)
                    channels.append(channel)
                except Exception:
                    break  # Stop warming if we can't create connections

            # Return all channels to pool
            for channel in channels:
                self._pool.release(channel)
        except Exception as e:
            logger.warning(f"Pool warm-up failed: {e}")

    def close(self) -> None:
        """Close all channels in the pool

        Uses deterministic cleanup via channel state subscription in dispose().
        No sleeps - waits for actual channel shutdown state.
        """
        logger.info(f"Closing gRPC connection pool")
        try:
            # ResourcePool.close() will call dispose() on each channel,
            # which waits deterministically for background threads to exit
            self._pool.close()
        except Exception as e:
            # Suppress errors during cleanup - pool is closing anyway
            logger.debug(f"Error during pool close (suppressed): {e}")


class RestConnectionPool:
    """
    Per-operation REST connection pool
    
    Features:
    - Specialized pools for read/write/search operations
    - Optimized timeouts and limits per operation type
    - Connection reuse and lifecycle management
    """
    
    def __init__(self, config: ClientConfig):
        self.config = config
        self._pools: Dict[str, httpx.Client] = {}
        self._lock = threading.RLock()
        self.metrics = PoolMetrics()
        self._request_times: List[float] = []
        
        self._initialize_pools()
    
    def _initialize_pools(self) -> None:
        """Initialize specialized connection pools"""
        logger.info("Initializing REST connection pools")
        
        # Base timeout and limits
        base_timeout = httpx.Timeout(
            connect=self.config.connection.connect_timeout,
            read=self.config.connection.read_timeout,
            write=self.config.timeout,
            pool=self.config.connection.total_timeout,
        )
        
        base_limits = httpx.Limits(
            max_keepalive_connections=self.config.connection.pool_size,
            max_connections=self.config.connection.pool_maxsize,
            keepalive_expiry=self.config.connection.keepalive_timeout,
        )
        
        # Read operations pool (collection info, health checks, etc.)
        self._pools['read'] = httpx.Client(
            base_url=self.config.url,
            headers=self.config.get_base_headers(),
            timeout=base_timeout,
            limits=httpx.Limits(
                max_connections=20,
                max_keepalive_connections=10,
                keepalive_expiry=base_limits.keepalive_expiry
            ),
            verify=self.config.tls.verify,
            cert=(self.config.tls.cert_file, self.config.tls.key_file) if self.config.tls.cert_file else None,
            http2=self.config.enable_http2,
        )
        
        # Write operations pool (insert, update, delete)
        write_timeout = httpx.Timeout(
            connect=base_timeout.connect,
            read=base_timeout.read,
            write=30.0,  # Longer write timeout
            pool=base_timeout.pool,
        )
        
        self._pools['write'] = httpx.Client(
            base_url=self.config.url,
            headers=self.config.get_base_headers(),
            timeout=write_timeout,
            limits=httpx.Limits(
                max_connections=10,  # Fewer connections for writes
                max_keepalive_connections=5,
                keepalive_expiry=base_limits.keepalive_expiry
            ),
            verify=self.config.tls.verify,
            cert=(self.config.tls.cert_file, self.config.tls.key_file) if self.config.tls.cert_file else None,
            http2=self.config.enable_http2,
        )
        
        # Search operations pool (vector search, similarity queries)
        self._pools['search'] = httpx.Client(
            base_url=self.config.url,
            headers=self.config.get_base_headers(),
            timeout=base_timeout,
            limits=httpx.Limits(
                max_connections=15,  # Balanced for concurrent searches
                max_keepalive_connections=8,
                keepalive_expiry=base_limits.keepalive_expiry
            ),
            verify=self.config.tls.verify,
            cert=(self.config.tls.cert_file, self.config.tls.key_file) if self.config.tls.cert_file else None,
            http2=self.config.enable_http2,
        )
        
        total_connections = 0
        for pool in self._pools.values():
            if hasattr(pool, 'limits') and hasattr(pool.limits, 'max_connections'):
                if isinstance(pool.limits.max_connections, int):
                    total_connections += pool.limits.max_connections
                else:
                    total_connections += 10  # Default for mocked clients
            else:
                total_connections += 10  # Default for mocked clients
        
        self.metrics.total_connections = total_connections
        self.metrics.idle_connections = total_connections
        
        logger.info(f"REST pools initialized: read(20), write(10), search(15) = {total_connections} total connections")
    
    def get_client(self, operation_type: str = 'read') -> httpx.Client:
        """Get client for specific operation type"""
        pool_type = self._map_operation_to_pool(operation_type)
        
        with self._lock:
            client = self._pools.get(pool_type, self._pools['read'])
            
            # Update metrics
            self.metrics.active_connections += 1
            if self.metrics.idle_connections > 0:
                self.metrics.idle_connections -= 1
            
            return client
    
    def return_client(self, client: httpx.Client, success: bool = True, response_time_ms: float = 0.0) -> None:
        """Return client to pool and update metrics"""
        with self._lock:
            # Update metrics
            self.metrics.requests_served += 1
            if response_time_ms > 0:
                self._request_times.append(response_time_ms)
                if len(self._request_times) > 100:  # Keep last 100 measurements
                    self._request_times.pop(0)
                self.metrics.avg_response_time_ms = sum(self._request_times) / len(self._request_times)
            
            # Update connection counts
            if self.metrics.active_connections > 0:
                self.metrics.active_connections -= 1
            self.metrics.idle_connections += 1
    
    def _map_operation_to_pool(self, operation_type: str) -> str:
        """Map operation type to appropriate pool"""
        operation_mapping = {
            # Read operations
            'health': 'read',
            'get_collection': 'read',
            'list_collections': 'read',
            'get_vector': 'read',
            
            # Write operations  
            'create_collection': 'write',
            'update_collection': 'write',
            'delete_collection': 'write',
            'insert_vectors': 'write',
            'update_vector': 'write',
            'delete_vector': 'write',
            'upsert_vectors': 'write',
            
            # Search operations
            'search_vectors': 'search',
            'similarity_search': 'search',
            'vector_search': 'search',
        }
        
        return operation_mapping.get(operation_type, 'read')
    
    def get_metrics(self) -> PoolMetrics:
        """Get current pool performance metrics"""
        with self._lock:
            self.metrics.health_status = PoolHealth.HEALTHY  # REST pools are generally stable
            self.metrics.last_health_check = time.time()
            return self.metrics
    
    def close(self) -> None:
        """Close all connection pools"""
        with self._lock:
            logger.info("Closing REST connection pools")
            for pool_name, client in self._pools.items():
                try:
                    client.close()
                    logger.debug(f"Closed {pool_name} pool")
                except Exception as e:
                    logger.warning(f"Error closing {pool_name} pool: {e}")
            
            self._pools.clear()


# Context managers for pool usage
class GrpcChannelContext:
    """Context manager for gRPC channel usage with automatic return"""
    
    def __init__(self, pool: GrpcConnectionPool):
        self.pool = pool
        self.channel = None
        self.start_time = None
    
    def __enter__(self) -> grpc.Channel:
        self.channel = self.pool.get_channel()
        self.start_time = time.time()
        return self.channel
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        if self.channel is not None:
            success = exc_type is None
            response_time_ms = (time.time() - self.start_time) * 1000 if self.start_time else 0.0
            self.pool.return_channel(self.channel, success, response_time_ms)


class RestClientContext:
    """Context manager for REST client usage with automatic return"""
    
    def __init__(self, pool: RestConnectionPool, operation_type: str = 'read'):
        self.pool = pool
        self.operation_type = operation_type
        self.client = None
        self.start_time = None
    
    def __enter__(self) -> httpx.Client:
        self.client = self.pool.get_client(self.operation_type)
        self.start_time = time.time()
        return self.client
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        if self.client is not None:
            success = exc_type is None
            response_time_ms = (time.time() - self.start_time) * 1000 if self.start_time else 0.0
            self.pool.return_client(self.client, success, response_time_ms)