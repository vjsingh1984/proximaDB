"""
Unified Intelligent Routing System for ProximaDB Python SDK

Combines operation-aware routing with health-based protocol selection for optimal
performance and reliability. This module unifies the functionality of both
OperationRouter and ProtocolSelector into a single, cohesive system.

Features:
- Operation-specific routing rules (bulk operations → gRPC, debugging → REST)
- Health monitoring with periodic checks for all protocols
- Performance metrics tracking (latency, throughput, error rates)
- Automatic failover between protocols
- Load balancing across protocol endpoints
- Circuit breaker pattern for failed protocols
- Custom routing rules and overrides
- Multiple selection strategies (performance, reliability, balanced, operation-aware)

Performance Target: 20-40% improvement in overall throughput and reliability
"""

import asyncio
import logging
import threading
import time
from typing import Any, Dict, List, Optional, Union, Tuple, Callable, Set
from dataclasses import dataclass, field
from enum import Enum
from collections import defaultdict, deque
import statistics
import weakref

from .config import Protocol, ClientConfig
from .exceptions import ProximaDBError, NetworkError, TimeoutError

logger = logging.getLogger(__name__)


class OperationType(str, Enum):
    """Types of operations for routing decisions"""
    # Bulk operations
    BULK_INSERT = "bulk_insert"
    BULK_UPSERT = "bulk_upsert"
    BULK_DELETE = "bulk_delete"
    BATCH_SEARCH = "batch_search"
    
    # Single operations
    SINGLE_INSERT = "single_insert"
    SINGLE_UPSERT = "single_upsert"
    SINGLE_DELETE = "single_delete"
    SINGLE_SEARCH = "single_search"
    GET_VECTOR = "get_vector"
    
    # Administrative
    CREATE_COLLECTION = "create_collection"
    DELETE_COLLECTION = "delete_collection"
    UPDATE_COLLECTION = "update_collection"
    LIST_COLLECTIONS = "list_collections"
    GET_COLLECTION_INFO = "get_collection_info"
    
    # System operations
    HEALTH_CHECK = "health_check"
    GET_METRICS = "get_metrics"
    SQL_QUERY = "sql_query"


class RoutingStrategy(str, Enum):
    """Unified routing strategies"""
    OPERATION_BASED = "operation_based"      # Route based on operation type
    PERFORMANCE_BASED = "performance_based"  # Route based on protocol performance
    RELIABILITY_BASED = "reliability_based"  # Route based on protocol health
    BALANCED = "balanced"                    # Balance all factors
    HYBRID = "hybrid"                        # Combine operation-based with performance
    ROUND_ROBIN = "round_robin"              # Simple round-robin
    STICKY = "sticky"                        # Stick to one protocol
    ADAPTIVE = "adaptive"                    # Learn from historical data


class ProtocolHealth(Enum):
    """Health status of a protocol"""
    HEALTHY = "healthy"
    DEGRADED = "degraded"
    UNHEALTHY = "unhealthy"
    UNKNOWN = "unknown"


@dataclass
class RoutingRule:
    """Rule for routing specific operations to protocols"""
    operation: OperationType
    preferred_protocol: Protocol
    priority: int = 1
    min_data_size_bytes: Optional[int] = None
    max_data_size_bytes: Optional[int] = None
    required_features: Set[str] = field(default_factory=set)
    fallback_allowed: bool = True
    
    def matches(self, operation: OperationType, data_size: Optional[int] = None) -> bool:
        """Check if this rule matches the given operation"""
        if self.operation != operation:
            return False
            
        if data_size is not None:
            if self.min_data_size_bytes and data_size < self.min_data_size_bytes:
                return False
            if self.max_data_size_bytes and data_size > self.max_data_size_bytes:
                return False
                
        return True


@dataclass
class ProtocolMetrics:
    """Comprehensive metrics for a protocol"""
    protocol: Protocol
    
    # Health metrics
    health_status: ProtocolHealth = ProtocolHealth.UNKNOWN
    last_health_check: float = 0.0
    consecutive_failures: int = 0
    circuit_breaker_open: bool = False
    circuit_breaker_half_open_time: float = 0.0
    
    # Performance metrics
    latency_samples: deque = field(default_factory=lambda: deque(maxlen=100))
    throughput_samples: deque = field(default_factory=lambda: deque(maxlen=100))
    error_counts: Dict[str, int] = field(default_factory=lambda: defaultdict(int))
    
    # Operation counts
    total_requests: int = 0
    successful_requests: int = 0
    failed_requests: int = 0
    
    # Timing
    total_response_time_ms: float = 0.0
    last_request_time: float = 0.0
    
    def update_success(self, latency_ms: float, throughput_qps: Optional[float] = None):
        """Update metrics for successful request"""
        self.successful_requests += 1
        self.total_requests += 1
        self.consecutive_failures = 0
        self.total_response_time_ms += latency_ms
        self.latency_samples.append(latency_ms)
        
        if throughput_qps:
            self.throughput_samples.append(throughput_qps)
            
        self.last_request_time = time.time()
        
        # Update health status
        if self.health_status == ProtocolHealth.UNHEALTHY:
            self.health_status = ProtocolHealth.DEGRADED
        elif self.consecutive_failures == 0 and len(self.latency_samples) > 10:
            # Create copy to avoid deque mutation during iteration
            avg_latency = statistics.mean(list(self.latency_samples))
            if avg_latency < 100:  # < 100ms is healthy
                self.health_status = ProtocolHealth.HEALTHY
                
    def update_failure(self, error_type: str = "unknown"):
        """Update metrics for failed request"""
        self.failed_requests += 1
        self.total_requests += 1
        self.consecutive_failures += 1
        self.error_counts[error_type] += 1
        self.last_request_time = time.time()
        
        # Update health status
        if self.consecutive_failures >= 5:
            self.health_status = ProtocolHealth.UNHEALTHY
            self.circuit_breaker_open = True
            self.circuit_breaker_half_open_time = time.time() + 30.0  # 30s cooldown
        elif self.consecutive_failures >= 3:
            self.health_status = ProtocolHealth.DEGRADED
            
    def get_success_rate(self) -> float:
        """Get success rate percentage"""
        if self.total_requests == 0:
            return 0.0
        return (self.successful_requests / self.total_requests) * 100
        
    def get_avg_latency(self) -> float:
        """Get average latency in ms"""
        if not self.latency_samples:
            return float('inf')
        # Create copy to avoid deque mutation during iteration
        return statistics.mean(list(self.latency_samples))
        
    def get_p95_latency(self) -> float:
        """Get 95th percentile latency"""
        if len(self.latency_samples) < 2:
            return float('inf')
        # Create copy to avoid deque mutation during iteration
        sorted_samples = sorted(list(self.latency_samples))
        p95_index = int(len(sorted_samples) * 0.95)
        return sorted_samples[p95_index]
        
    def get_score(self, strategy: RoutingStrategy) -> float:
        """Get routing score based on strategy"""
        if self.circuit_breaker_open and time.time() < self.circuit_breaker_half_open_time:
            return 0.0
            
        if self.health_status == ProtocolHealth.UNHEALTHY:
            return 0.0
            
        if strategy == RoutingStrategy.PERFORMANCE_BASED:
            # Lower latency is better
            avg_latency = self.get_avg_latency()
            if avg_latency == float('inf'):
                return 0.5
            return max(0.0, min(1.0, 100.0 / avg_latency))
            
        elif strategy == RoutingStrategy.RELIABILITY_BASED:
            return self.get_success_rate() / 100.0
            
        elif strategy == RoutingStrategy.BALANCED:
            perf_score = max(0.0, min(1.0, 100.0 / self.get_avg_latency())) if self.get_avg_latency() != float('inf') else 0.5
            reliability_score = self.get_success_rate() / 100.0
            return (perf_score * 0.4) + (reliability_score * 0.6)
            
        else:
            return 1.0 if self.health_status in [ProtocolHealth.HEALTHY, ProtocolHealth.DEGRADED] else 0.0


@dataclass
class RoutingConfig:
    """Configuration for intelligent routing"""
    # Routing strategy
    strategy: RoutingStrategy = RoutingStrategy.BALANCED
    
    # Health monitoring
    health_check_interval_seconds: float = 30.0
    health_check_timeout_seconds: float = 5.0
    circuit_breaker_failure_threshold: int = 5
    circuit_breaker_recovery_timeout: float = 30.0
    
    # Performance thresholds
    healthy_latency_threshold_ms: float = 100.0
    degraded_latency_threshold_ms: float = 500.0
    
    # Fallback behavior
    enable_fallback: bool = True
    max_fallback_attempts: int = 2
    fallback_delay_ms: float = 100.0
    
    # Load balancing
    enable_load_balancing: bool = False
    load_balance_window_seconds: float = 60.0
    
    # Adaptive learning
    enable_adaptive_learning: bool = True
    learning_window_size: int = 1000
    learning_update_interval: float = 300.0  # 5 minutes
    
    # Custom rules
    custom_rules: List[RoutingRule] = field(default_factory=list)


class IntelligentRouter:
    """
    Unified intelligent routing system for ProximaDB
    
    Combines operation-aware routing with health-based protocol selection
    for optimal performance and reliability.
    """
    
    def __init__(self, config: RoutingConfig = None, client_config: ClientConfig = None):
        self.config = config or RoutingConfig()
        self.client_config = client_config
        
        # Protocol metrics
        self._metrics: Dict[Protocol, ProtocolMetrics] = {
            Protocol.REST: ProtocolMetrics(Protocol.REST),
            Protocol.GRPC: ProtocolMetrics(Protocol.GRPC)
        }
        
        # Thread safety
        self._metrics_lock = threading.RLock()
        self._routing_lock = threading.RLock()
        
        # Routing rules (built-in + custom)
        self._routing_rules = self._create_default_rules() + self.config.custom_rules
        self._routing_rules.sort(key=lambda r: r.priority, reverse=True)
        
        # Load balancing state
        self._load_balance_counters: Dict[Protocol, int] = defaultdict(int)
        self._load_balance_window_start = time.time()
        
        # Adaptive learning
        self._operation_history: Dict[Tuple[OperationType, Protocol], deque] = defaultdict(
            lambda: deque(maxlen=self.config.learning_window_size)
        )
        self._learned_preferences: Dict[OperationType, Protocol] = {}
        
        # Client management
        self._client_factories: Dict[Protocol, Callable] = {}
        self._clients: Dict[Protocol, Any] = {}  # Regular dict to properly cache clients
        
        # Background monitoring
        self._monitoring_thread: Optional[threading.Thread] = None
        self._stop_monitoring = threading.Event()
        
        # Selection state
        self._current_protocol: Optional[Protocol] = None
        self._round_robin_index = 0
        
        if self.config.health_check_interval_seconds > 0:
            self._start_monitoring()
            
        logger.info(f"Initialized IntelligentRouter with strategy: {self.config.strategy.value}")
        
    def _create_default_rules(self) -> List[RoutingRule]:
        """Create default routing rules based on operation characteristics"""
        return [
            # Bulk operations → gRPC (better for large payloads)
            RoutingRule(OperationType.BULK_INSERT, Protocol.GRPC, priority=10),
            RoutingRule(OperationType.BULK_UPSERT, Protocol.GRPC, priority=10),
            RoutingRule(OperationType.BULK_DELETE, Protocol.GRPC, priority=9),
            RoutingRule(OperationType.BATCH_SEARCH, Protocol.GRPC, priority=9),
            
            # Administrative operations → REST (better for debugging)
            RoutingRule(OperationType.HEALTH_CHECK, Protocol.REST, priority=8),
            RoutingRule(OperationType.GET_METRICS, Protocol.REST, priority=8),
            RoutingRule(OperationType.SQL_QUERY, Protocol.REST, priority=8),
            
            # Collection management → REST (simpler HTTP semantics)
            RoutingRule(OperationType.LIST_COLLECTIONS, Protocol.REST, priority=7),
            RoutingRule(OperationType.CREATE_COLLECTION, Protocol.REST, priority=7),
            RoutingRule(OperationType.DELETE_COLLECTION, Protocol.REST, priority=7),
            RoutingRule(OperationType.GET_COLLECTION_INFO, Protocol.REST, priority=7),
            
            # Small single operations → gRPC for efficiency
            RoutingRule(OperationType.SINGLE_INSERT, Protocol.GRPC, max_data_size_bytes=10240, priority=6),
            RoutingRule(OperationType.SINGLE_SEARCH, Protocol.GRPC, priority=6),
            RoutingRule(OperationType.GET_VECTOR, Protocol.GRPC, priority=6),
            
            # Large single operations → REST (easier debugging)
            RoutingRule(OperationType.SINGLE_INSERT, Protocol.REST, min_data_size_bytes=10240, priority=5),
            RoutingRule(OperationType.SINGLE_UPSERT, Protocol.REST, min_data_size_bytes=10240, priority=5),
        ]
        
    def register_client_factory(self, protocol: Protocol, factory: Callable):
        """Register client factory function for a protocol"""
        self._client_factories[protocol] = factory
        
    def _get_client(self, protocol: Protocol) -> Any:
        """Get or create client for protocol"""
        if protocol not in self._clients:
            if protocol in self._client_factories:
                self._clients[protocol] = self._client_factories[protocol]()
            else:
                raise ValueError(f"No client factory registered for {protocol}")
        return self._clients[protocol]
        
    def route_operation(
        self,
        operation: OperationType,
        data_size: Optional[int] = None,
        required_features: Optional[Set[str]] = None,
        preferred_protocol: Optional[Protocol] = None
    ) -> Tuple[Protocol, Any]:
        """
        Route an operation to the most suitable protocol
        
        Args:
            operation: Type of operation to route
            data_size: Size of data in bytes (for size-based routing)
            required_features: Set of required protocol features
            preferred_protocol: User preference (will be honored if healthy)
            
        Returns:
            Tuple of (selected protocol, client instance)
        """
        with self._routing_lock:
            # Honor user preference if protocol is healthy
            if preferred_protocol and self._is_protocol_healthy(preferred_protocol):
                try:
                    client = self._get_client(preferred_protocol)
                    return preferred_protocol, client
                except Exception as e:
                    logger.warning(f"Failed to use preferred protocol {preferred_protocol}: {e}")
                    
            # Select protocol based on strategy
            selected_protocol = self._select_protocol(operation, data_size, required_features)
            
            # Get client with fallback
            client = None
            attempts = 0
            
            while client is None and attempts < self.config.max_fallback_attempts:
                try:
                    client = self._get_client(selected_protocol)
                    
                    # Update load balancing counters
                    if self.config.enable_load_balancing:
                        self._load_balance_counters[selected_protocol] += 1
                        
                    # Record for adaptive learning
                    if self.config.enable_adaptive_learning:
                        self._record_operation_start(operation, selected_protocol)
                        
                    return selected_protocol, client
                    
                except Exception as e:
                    logger.warning(f"Failed to get client for {selected_protocol}: {e}")
                    self._metrics[selected_protocol].update_failure("client_creation_failed")
                    
                    # Try fallback protocol
                    if self.config.enable_fallback:
                        selected_protocol = self._get_fallback_protocol(selected_protocol)
                        if selected_protocol is None:
                            break
                        time.sleep(self.config.fallback_delay_ms / 1000.0)
                        
                    attempts += 1
                    
            raise ProximaDBError("No healthy protocol available for operation")
            
    def _select_protocol(
        self,
        operation: OperationType,
        data_size: Optional[int] = None,
        required_features: Optional[Set[str]] = None
    ) -> Protocol:
        """Select protocol based on routing strategy"""
        
        if self.config.strategy in [RoutingStrategy.OPERATION_BASED, RoutingStrategy.HYBRID]:
            # Use routing rules first
            for rule in self._routing_rules:
                if rule.matches(operation, data_size):
                    if self._is_protocol_healthy(rule.preferred_protocol):
                        # For HYBRID strategy, also check performance
                        if self.config.strategy == RoutingStrategy.HYBRID:
                            preferred_metrics = self._metrics[rule.preferred_protocol]
                            fallback_protocol = self._get_fallback_protocol(rule.preferred_protocol)
                            if fallback_protocol:
                                fallback_metrics = self._metrics[fallback_protocol]
                                # Choose based on performance if both are healthy
                                if (self._is_protocol_healthy(fallback_protocol) and
                                    fallback_metrics.get_score(RoutingStrategy.PERFORMANCE_BASED) > 
                                    preferred_metrics.get_score(RoutingStrategy.PERFORMANCE_BASED) * 1.2):
                                    return fallback_protocol
                        return rule.preferred_protocol
                    elif rule.fallback_allowed:
                        fallback = self._get_fallback_protocol(rule.preferred_protocol)
                        if fallback:
                            return fallback
                            
        elif self.config.strategy == RoutingStrategy.ROUND_ROBIN:
            # Simple round-robin (exclude AUTO which is not a real protocol)
            protocols = [p for p in Protocol if p != Protocol.AUTO and self._is_protocol_healthy(p)]
            if protocols:
                self._round_robin_index = (self._round_robin_index + 1) % len(protocols)
                return protocols[self._round_robin_index]
                
        elif self.config.strategy == RoutingStrategy.STICKY:
            # Stick to current protocol if healthy
            if self._current_protocol and self._is_protocol_healthy(self._current_protocol):
                return self._current_protocol
            # Otherwise select new one
            self._current_protocol = self._select_best_protocol()
            return self._current_protocol
            
        elif self.config.strategy == RoutingStrategy.ADAPTIVE:
            # Use learned preferences
            if operation in self._learned_preferences:
                preferred = self._learned_preferences[operation]
                if self._is_protocol_healthy(preferred):
                    return preferred
                    
        # Default: select based on metrics
        return self._select_best_protocol()
        
    def _select_best_protocol(self) -> Protocol:
        """Select best protocol based on current metrics"""
        with self._metrics_lock:
            scores = {
                protocol: metrics.get_score(self.config.strategy)
                for protocol, metrics in self._metrics.items()
            }
            
            # Add load balancing factor
            if self.config.enable_load_balancing:
                window_elapsed = time.time() - self._load_balance_window_start
                if window_elapsed > self.config.load_balance_window_seconds:
                    # Reset counters
                    self._load_balance_counters.clear()
                    self._load_balance_window_start = time.time()
                else:
                    # Adjust scores based on usage
                    total_requests = sum(self._load_balance_counters.values()) or 1
                    for protocol in scores:
                        usage_ratio = self._load_balance_counters[protocol] / total_requests
                        # Penalize overused protocols
                        scores[protocol] *= (1.0 - (usage_ratio * 0.3))
                        
            # Select protocol with highest score
            best_protocol = max(scores.items(), key=lambda x: x[1])[0]
            return best_protocol
            
    def _is_protocol_healthy(self, protocol: Protocol) -> bool:
        """Check if protocol is healthy enough to use"""
        metrics = self._metrics[protocol]
        return (
            metrics.health_status in [ProtocolHealth.HEALTHY, ProtocolHealth.DEGRADED] and
            not metrics.circuit_breaker_open
        )
        
    def _get_fallback_protocol(self, failed_protocol: Protocol) -> Optional[Protocol]:
        """Get fallback protocol for failed one"""
        # Simply return the other protocol if healthy
        fallback = Protocol.GRPC if failed_protocol == Protocol.REST else Protocol.REST
        return fallback if self._is_protocol_healthy(fallback) else None
        
    def _record_operation_start(self, operation: OperationType, protocol: Protocol):
        """Record operation start for adaptive learning"""
        key = (operation, protocol)
        self._operation_history[key].append({
            'start_time': time.time(),
            'completed': False
        })
        
    def record_operation_result(
        self,
        operation: OperationType,
        protocol: Protocol,
        success: bool,
        latency_ms: float,
        throughput_qps: Optional[float] = None
    ):
        """
        Record operation result for metrics and learning
        
        Should be called after each operation completes.
        """
        with self._metrics_lock:
            if success:
                self._metrics[protocol].update_success(latency_ms, throughput_qps)
            else:
                self._metrics[protocol].update_failure()
                
            # Update adaptive learning
            if self.config.enable_adaptive_learning:
                key = (operation, protocol)
                if key in self._operation_history and self._operation_history[key]:
                    # Mark as completed
                    self._operation_history[key][-1]['completed'] = True
                    self._operation_history[key][-1]['success'] = success
                    self._operation_history[key][-1]['latency_ms'] = latency_ms
                    
                    # Update learned preferences periodically
                    self._update_learned_preferences()
                    
    def _update_learned_preferences(self):
        """Update learned preferences based on historical data"""
        # This is called frequently, so only update periodically
        current_time = time.time()
        if not hasattr(self, '_last_learning_update'):
            self._last_learning_update = 0
            
        if current_time - self._last_learning_update < self.config.learning_update_interval:
            return
            
        self._last_learning_update = current_time
        
        # Analyze history for each operation
        for operation in OperationType:
            best_protocol = None
            best_score = -1
            
            for protocol in Protocol:
                key = (operation, protocol)
                history = self._operation_history.get(key, [])
                
                if len(history) < 10:  # Need minimum samples
                    continue
                    
                # Calculate success rate and avg latency
                completed = [h for h in history if h.get('completed', False)]
                if not completed:
                    continue
                    
                success_rate = sum(1 for h in completed if h.get('success', False)) / len(completed)
                avg_latency = statistics.mean(h.get('latency_ms', 1000) for h in completed)
                
                # Combined score (lower latency is better)
                score = success_rate * (100.0 / avg_latency)
                
                if score > best_score:
                    best_score = score
                    best_protocol = protocol
                    
            if best_protocol:
                self._learned_preferences[operation] = best_protocol
                
    def _start_monitoring(self):
        """Start background health monitoring"""
        self._monitoring_thread = threading.Thread(
            target=self._monitoring_loop,
            daemon=True,
            name="IntelligentRouter-Monitor"
        )
        self._monitoring_thread.start()
        logger.info("Started intelligent router monitoring")
        
    def _monitoring_loop(self):
        """Background monitoring loop"""
        while not self._stop_monitoring.wait(self.config.health_check_interval_seconds):
            try:
                self._perform_health_checks()
            except Exception as e:
                logger.error(f"Error in monitoring loop: {e}")
                
    def _perform_health_checks(self):
        """Perform health checks on all protocols"""
        for protocol in Protocol:
            try:
                if protocol in self._client_factories:
                    client = self._get_client(protocol)
                    start_time = time.time()
                    
                    # Perform health check
                    if hasattr(client, 'health_check'):
                        result = client.health_check()
                        latency_ms = (time.time() - start_time) * 1000
                        
                        if result.get('status') in ['healthy', 'ok', True]:
                            self._metrics[protocol].update_success(latency_ms)
                        else:
                            self._metrics[protocol].update_failure("health_check_failed")
                    else:
                        # No health check method, assume healthy if client exists
                        self._metrics[protocol].health_status = ProtocolHealth.HEALTHY
                        
            except Exception as e:
                logger.error(f"Health check failed for {protocol}: {e}")
                self._metrics[protocol].update_failure("health_check_exception")
                
    def get_metrics(self) -> Dict[str, Any]:
        """Get comprehensive routing metrics"""
        with self._metrics_lock:
            return {
                'strategy': self.config.strategy.value,
                'protocols': {
                    protocol.value: {
                        'health': metrics.health_status.value,
                        'success_rate': metrics.get_success_rate(),
                        'avg_latency_ms': metrics.get_avg_latency(),
                        'p95_latency_ms': metrics.get_p95_latency(),
                        'total_requests': metrics.total_requests,
                        'circuit_breaker_open': metrics.circuit_breaker_open,
                    }
                    for protocol, metrics in self._metrics.items()
                },
                'learned_preferences': {
                    op.value: prot.value 
                    for op, prot in self._learned_preferences.items()
                },
                'current_selection': self._current_protocol.value if self._current_protocol else None,
            }
            
    def stop(self):
        """Stop monitoring and cleanup"""
        if self._monitoring_thread:
            self._stop_monitoring.set()
            self._monitoring_thread.join(timeout=5.0)
            self._monitoring_thread = None
            
        # Clear clients
        self._clients.clear()
        
        logger.info("Stopped intelligent router")


# Convenience exports for backward compatibility
OperationRouter = IntelligentRouter
ProtocolSelector = IntelligentRouter