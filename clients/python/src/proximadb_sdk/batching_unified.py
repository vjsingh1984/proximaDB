"""
Unified Batching Module for ProximaDB Python SDK

Combines async and sync batching capabilities with a clean, unified interface.
Supports both REST and gRPC protocols with appropriate strategies.

Features:
- Protocol-aware batching (async for gRPC, thread-based for REST)
- Configurable batch sizes and timeouts
- Adaptive batching based on performance metrics
- Memory-efficient batch processing
- Unified metrics and monitoring

Performance Targets:
- REST: +30-50% throughput improvement
- gRPC: +15-25% throughput improvement
"""

import asyncio
import logging
import threading
import time
import uuid
from abc import ABC, abstractmethod
from collections import defaultdict, deque
from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Callable, Dict, List, Optional, Tuple, Union

from .exceptions import BatchError, ProximaDBError
from .models import VectorOperationResponse, VectorRecord
from .models_v2 import ProximaRecord

logger = logging.getLogger(__name__)


class BatchStrategy(str, Enum):
    """Unified batching strategies"""

    SIZE_BASED = "size_based"  # Batch when size threshold reached
    TIME_BASED = "time_based"  # Batch after time window
    ADAPTIVE = "adaptive"  # Dynamic batching based on load
    HYBRID = "hybrid"  # Combine size and time thresholds
    IMMEDIATE = "immediate"  # No batching, immediate execution


class BatchOperationType(str, Enum):
    """Types of operations that can be batched"""

    INSERT_RECORDS = "insert_records"
    UPSERT_RECORDS = "upsert_records"
    INSERT_VECTORS = "insert_vectors"  # compatibility alias
    UPSERT_VECTORS = "upsert_vectors"  # compatibility alias
    DELETE_VECTORS = "delete_vectors"
    SEARCH_VECTORS = "search_vectors"
    GET_VECTORS = "get_vectors"
    UPDATE_VECTORS = "update_vectors"


@dataclass
class BatchConfig:
    """Unified configuration for request batching"""

    max_batch_size: int = 1000
    min_batch_size: int = 10
    max_wait_time_ms: float = 100.0
    strategy: BatchStrategy = BatchStrategy.HYBRID
    enable_compression: bool = True
    max_memory_mb: float = 50.0
    performance_window_size: int = 100
    adaptive_threshold: float = 0.8
    max_concurrent_batches: int = 10


@dataclass
class BatchMetrics:
    """Unified metrics for batch operations"""

    total_requests: int = 0
    batched_requests: int = 0
    total_batches: int = 0
    avg_batch_size: float = 0.0
    total_latency_ms: float = 0.0
    avg_latency_ms: float = 0.0
    throughput_qps: float = 0.0
    cache_hit_ratio: float = 0.0
    memory_usage_mb: float = 0.0
    last_updated: float = field(default_factory=time.time)


@dataclass
class BatchRequest:
    """Unified batch request container"""

    request_id: str = field(default_factory=lambda: str(uuid.uuid4()))
    operation: BatchOperationType = None
    collection_id: str = None
    data: Any = None
    callback: Optional[Callable] = None
    priority: int = 1
    timestamp: float = field(default_factory=time.time)
    future: Optional[Union[Future, asyncio.Future]] = None

    def __lt__(self, other):
        """Priority comparison for heap operations"""
        return self.priority > other.priority  # Higher priority = smaller value


class BatchProcessor(ABC):
    """Abstract base class for batch processors"""

    def __init__(self, config: BatchConfig):
        self.config = config
        self.metrics = BatchMetrics()
        self._running = False

    @abstractmethod
    def start(self):
        """Start the batch processor"""
        pass

    @abstractmethod
    def stop(self):
        """Stop the batch processor"""
        pass

    @abstractmethod
    def submit_request(self, request: BatchRequest) -> Any:
        """Submit a request for batching"""
        pass

    def get_metrics(self) -> BatchMetrics:
        """Get current batch metrics"""
        return self.metrics

    def _estimate_request_size(self, request: BatchRequest) -> float:
        """Estimate memory size of request in MB"""
        if request.data is None:
            return 0.001  # 1KB minimum

        if isinstance(request.data, list):
            # For vector operations, estimate based on vector count and dimensions
            if request.data and isinstance(request.data[0], VectorRecord):
                vector_size = len(request.data[0].vector) * 4  # 4 bytes per float
                metadata_size = 1024  # Estimate 1KB per metadata
                return len(request.data) * (vector_size + metadata_size) / (1024 * 1024)

        # Default estimation
        return 0.01  # 10KB default


class AsyncBatchProcessor(BatchProcessor):
    """Async batch processor for protocols that support async operations"""

    def __init__(self, config: BatchConfig, execute_batch_fn: Callable):
        super().__init__(config)
        self.execute_batch_fn = execute_batch_fn
        self._batches: Dict[str, List[BatchRequest]] = defaultdict(list)
        self._timers: Dict[str, asyncio.Task] = {}
        self._lock = asyncio.Lock()

    async def start(self):
        """Start the async batch processor"""
        self._running = True

    async def stop(self):
        """Stop the async batch processor"""
        self._running = False

        # Cancel timers
        for timer in self._timers.values():
            timer.cancel()
        self._timers.clear()

        # Process remaining batches
        async with self._lock:
            for batch_key, requests in self._batches.items():
                if requests:
                    await self._execute_batch(batch_key, requests)

    async def submit_request(self, request: BatchRequest) -> Any:
        """Submit a request for async batching"""
        if not self._running:
            raise RuntimeError("Batch processor is not running")

        # Create future for result
        future = asyncio.Future()
        request.future = future

        batch_key = f"{request.operation.value}_{request.collection_id}"

        async with self._lock:
            self._batches[batch_key].append(request)

            # Check if we should execute batch
            if await self._should_execute_batch(batch_key):
                await self._execute_batch(batch_key, self._batches[batch_key])
                self._batches[batch_key] = []

                # Cancel timer if exists
                if batch_key in self._timers:
                    self._timers[batch_key].cancel()
                    del self._timers[batch_key]
            else:
                # Set timer if not exists
                if batch_key not in self._timers:
                    self._timers[batch_key] = asyncio.create_task(
                        self._batch_timer(batch_key)
                    )

        return await future

    async def _should_execute_batch(self, batch_key: str) -> bool:
        """Determine if batch should be executed"""
        requests = self._batches[batch_key]

        if not requests:
            return False

        if self.config.strategy == BatchStrategy.IMMEDIATE:
            return True

        if self.config.strategy in (BatchStrategy.SIZE_BASED, BatchStrategy.HYBRID):
            if len(requests) >= self.config.max_batch_size:
                return True

        if self.config.strategy in (BatchStrategy.TIME_BASED, BatchStrategy.HYBRID):
            oldest_request = min(requests, key=lambda r: r.timestamp)
            time_elapsed = (time.time() - oldest_request.timestamp) * 1000
            if time_elapsed >= self.config.max_wait_time_ms:
                return True

        return False

    async def _batch_timer(self, batch_key: str):
        """Timer for time-based batching"""
        await asyncio.sleep(self.config.max_wait_time_ms / 1000.0)

        async with self._lock:
            if batch_key in self._batches and self._batches[batch_key]:
                await self._execute_batch(batch_key, self._batches[batch_key])
                self._batches[batch_key] = []

    async def _execute_batch(self, batch_key: str, requests: List[BatchRequest]):
        """Execute a batch of requests"""
        if not requests:
            return

        start_time = time.time()
        batch_data = [req.data for req in requests]

        try:
            # Execute batch through provided function
            results = await self.execute_batch_fn(
                requests[0].operation, requests[0].collection_id, batch_data
            )

            # Distribute results to futures
            for i, req in enumerate(requests):
                if req.future and not req.future.done():
                    if isinstance(results, list) and i < len(results):
                        req.future.set_result(results[i])
                    else:
                        req.future.set_result(results)

        except Exception as e:
            # Set exception for all futures
            for req in requests:
                if req.future and not req.future.done():
                    req.future.set_exception(e)

        finally:
            # Update metrics
            elapsed_ms = (time.time() - start_time) * 1000
            self._update_metrics(len(requests), elapsed_ms)

    def _update_metrics(self, batch_size: int, latency_ms: float):
        """Update batch metrics"""
        self.metrics.total_requests += batch_size
        self.metrics.batched_requests += batch_size
        self.metrics.total_batches += 1
        self.metrics.total_latency_ms += latency_ms

        if self.metrics.total_batches > 0:
            self.metrics.avg_batch_size = (
                self.metrics.batched_requests / self.metrics.total_batches
            )
            self.metrics.avg_latency_ms = (
                self.metrics.total_latency_ms / self.metrics.total_batches
            )

        self.metrics.last_updated = time.time()


class ThreadedBatchProcessor(BatchProcessor):
    """Thread-based batch processor for sync operations"""

    def __init__(self, config: BatchConfig, execute_batch_fn: Callable):
        super().__init__(config)
        self.execute_batch_fn = execute_batch_fn
        self._request_queues: Dict[str, deque] = defaultdict(lambda: deque())
        self._queue_locks: Dict[str, threading.RLock] = defaultdict(threading.RLock)
        self._processing_threads: Dict[str, threading.Thread] = {}
        self._stop_events: Dict[str, threading.Event] = {}
        self._executor = ThreadPoolExecutor(max_workers=config.max_concurrent_batches)

    def start(self):
        """Start the threaded batch processor"""
        self._running = True

    def stop(self):
        """Stop the threaded batch processor"""
        self._running = False

        # Signal all threads to stop
        for event in self._stop_events.values():
            event.set()

        # Wait for threads to finish
        for thread in self._processing_threads.values():
            if thread.is_alive():
                thread.join(timeout=5.0)

        self._processing_threads.clear()
        self._stop_events.clear()
        self._executor.shutdown(wait=True)

    def submit_request(self, request: BatchRequest) -> Any:
        """Submit a request for threaded batching"""
        if not self._running:
            raise RuntimeError("Batch processor is not running")

        # Create future for result
        future = Future()
        request.future = future

        batch_key = f"{request.operation.value}_{request.collection_id}"

        # Add to queue
        with self._queue_locks[batch_key]:
            self._request_queues[batch_key].append(request)

        # Start processing thread if not exists
        if batch_key not in self._processing_threads:
            self._start_processing_thread(batch_key)

        return future.result()  # Block and wait for result

    def _start_processing_thread(self, batch_key: str):
        """Start a processing thread for a batch key"""
        stop_event = threading.Event()
        self._stop_events[batch_key] = stop_event

        thread = threading.Thread(
            target=self._batch_processing_loop,
            args=(batch_key, stop_event),
            daemon=True,
            name=f"Batch-{batch_key}",
        )
        self._processing_threads[batch_key] = thread
        thread.start()

    def _batch_processing_loop(self, batch_key: str, stop_event: threading.Event):
        """Main processing loop for a batch"""
        while not stop_event.is_set():
            batch = self._collect_batch(batch_key)

            if batch:
                self._execute_batch_sync(batch_key, batch)
            else:
                # No requests, sleep briefly
                time.sleep(0.01)  # 10ms

    def _collect_batch(self, batch_key: str) -> List[BatchRequest]:
        """Collect requests for batching"""
        batch = []
        start_time = time.time()

        with self._queue_locks[batch_key]:
            queue = self._request_queues[batch_key]

            while queue and len(batch) < self.config.max_batch_size:
                # Check time limit
                if (
                    batch
                    and (time.time() - start_time) * 1000
                    >= self.config.max_wait_time_ms
                ):
                    break

                batch.append(queue.popleft())

        return batch

    def _execute_batch_sync(self, batch_key: str, requests: List[BatchRequest]):
        """Execute a batch of requests synchronously"""
        if not requests:
            return

        start_time = time.time()
        batch_data = [req.data for req in requests]

        try:
            # Execute batch through provided function
            results = self.execute_batch_fn(
                requests[0].operation, requests[0].collection_id, batch_data
            )

            # Distribute results to futures
            for i, req in enumerate(requests):
                if req.future and not req.future.done():
                    if isinstance(results, list) and i < len(results):
                        req.future.set_result(results[i])
                    else:
                        req.future.set_result(results)

        except Exception as e:
            # Set exception for all futures
            for req in requests:
                if req.future and not req.future.done():
                    req.future.set_exception(e)

        finally:
            # Update metrics
            elapsed_ms = (time.time() - start_time) * 1000
            self._update_metrics(len(requests), elapsed_ms)

    def _update_metrics(self, batch_size: int, latency_ms: float):
        """Update batch metrics"""
        self.metrics.total_requests += batch_size
        self.metrics.batched_requests += batch_size
        self.metrics.total_batches += 1
        self.metrics.total_latency_ms += latency_ms

        if self.metrics.total_batches > 0:
            self.metrics.avg_batch_size = (
                self.metrics.batched_requests / self.metrics.total_batches
            )
            self.metrics.avg_latency_ms = (
                self.metrics.total_latency_ms / self.metrics.total_batches
            )

        self.metrics.last_updated = time.time()


class UnifiedBatchManager:
    """
    Unified batch manager that provides appropriate batch processor
    based on protocol and operation requirements
    """

    def __init__(self, config: BatchConfig = None):
        self.config = config or BatchConfig()
        self._processors: Dict[str, BatchProcessor] = {}
        self._lock = threading.RLock()

    def get_processor(
        self,
        protocol: str,
        execute_batch_fn: Callable,
        processor_id: Optional[str] = None,
    ) -> BatchProcessor:
        """
        Get or create a batch processor for the given protocol

        Args:
            protocol: 'grpc' or 'rest'
            execute_batch_fn: Function to execute batched requests
            processor_id: Optional ID for multiple processors

        Returns:
            Appropriate BatchProcessor instance
        """
        key = f"{protocol}_{processor_id or 'default'}"

        with self._lock:
            if key not in self._processors:
                if protocol == "grpc":
                    processor = AsyncBatchProcessor(self.config, execute_batch_fn)
                else:
                    processor = ThreadedBatchProcessor(self.config, execute_batch_fn)

                self._processors[key] = processor

            return self._processors[key]

    def get_all_metrics(self) -> Dict[str, BatchMetrics]:
        """Get metrics from all processors"""
        with self._lock:
            return {
                key: processor.get_metrics()
                for key, processor in self._processors.items()
            }

    def stop_all(self):
        """Stop all batch processors"""
        with self._lock:
            for processor in self._processors.values():
                processor.stop()
            self._processors.clear()


# Helper functions for common batching operations
def create_vector_batcher(
    client, collection_id: str, max_batch_size: int = 100
) -> "VectorBatcher":
    """Create a vector-specific batcher"""
    return VectorBatcher(client, collection_id, max_batch_size)


def batch_insert_vectors(
    client, collection_id: str, vectors: List["VectorRecord"], batch_size: int = 100
) -> List[Dict]:
    """Compatibility alias for batch_insert_records."""
    return batch_insert_records(client, collection_id, vectors, batch_size)


def batch_insert_records(
    client,
    collection_id: str,
    records: List[Union[ProximaRecord, Dict[str, Any]]],
    batch_size: int = 100,
) -> List[Dict]:
    """Helper function to batch insert ProximaRecord-shaped records."""
    results = []
    for i in range(0, len(records), batch_size):
        batch = records[i : i + batch_size]
        if hasattr(client, "insert_records"):
            response = client.insert_records(collection_id, batch)
        else:
            response = client.insert_vectors(collection_id, records=batch)
        results.append(response)
    return results


class VectorBatcher:
    """Simple vector batcher for tests"""

    def __init__(self, client, collection_id: str, max_batch_size: int = 100):
        self.client = client
        self.collection_id = collection_id
        self.config = BatchConfig(max_batch_size=max_batch_size)

    def get_metrics(self) -> BatchMetrics:
        return BatchMetrics()


# Convenience exports for backward compatibility
RequestBatcher = UnifiedBatchManager
RestBatchProcessor = ThreadedBatchProcessor
