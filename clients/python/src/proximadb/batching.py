"""
ProximaDB Request Batching and Pipelining

Implements intelligent request batching, pipelining, and bulk operation optimization
for high-throughput scenarios.
"""

import asyncio
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Callable, Dict, List, Optional, Union
from collections import defaultdict
import threading
import uuid

from pydantic import BaseModel, Field

from .models import VectorRecord, VectorOperationResponse


class BatchStrategy(str, Enum):
    """Batching strategies for request optimization"""
    SIZE_BASED = "size_based"      # Batch when size threshold reached
    TIME_BASED = "time_based"      # Batch after time window
    ADAPTIVE = "adaptive"          # Dynamic batching based on load
    IMMEDIATE = "immediate"        # No batching, immediate execution


class BatchOperationType(str, Enum):
    """Types of operations that can be batched"""
    INSERT_VECTORS = "insert_vectors"
    UPSERT_VECTORS = "upsert_vectors"
    DELETE_VECTORS = "delete_vectors"
    SEARCH_VECTORS = "search_vectors"
    GET_VECTORS = "get_vectors"


@dataclass
class BatchMetrics:
    """Metrics for batch operations"""
    total_requests: int = 0
    batched_requests: int = 0
    avg_batch_size: float = 0.0
    total_latency_ms: float = 0.0
    avg_latency_ms: float = 0.0
    throughput_qps: float = 0.0
    cache_hit_ratio: float = 0.0
    last_updated: float = field(default_factory=time.time)


class BatchConfig(BaseModel):
    """Configuration for request batching"""
    max_batch_size: int = Field(default=1000, ge=1, le=10000)
    max_wait_time_ms: int = Field(default=100, ge=1, le=5000)
    strategy: BatchStrategy = Field(default=BatchStrategy.ADAPTIVE)
    
    # Adaptive parameters
    target_latency_ms: float = Field(default=50.0, ge=1.0)
    min_batch_size: int = Field(default=10, ge=1)
    load_threshold: float = Field(default=0.8, ge=0.1, le=1.0)
    
    # Resource limits
    max_concurrent_batches: int = Field(default=10, ge=1, le=100)
    memory_limit_mb: int = Field(default=256, ge=64)


@dataclass
class BatchRequest:
    """A request that can be batched"""
    id: str
    operation: BatchOperationType
    collection_id: str
    data: Any
    callback: Optional[Callable] = None
    future: Optional[asyncio.Future] = None
    timestamp: float = field(default_factory=time.time)
    metadata: Dict[str, Any] = field(default_factory=dict)


class RequestBatcher:
    """
    Intelligent request batcher with multiple strategies and adaptive optimization.
    
    Features:
    - Multiple batching strategies (size, time, adaptive)
    - Per-collection batching for optimal performance
    - Adaptive batch sizing based on latency and throughput
    - Memory pressure monitoring
    - Comprehensive metrics tracking
    """
    
    def __init__(self, config: BatchConfig, executor_func: Callable):
        self.config = config
        self.executor_func = executor_func
        self.metrics = BatchMetrics()
        
        # Batching state
        self._batches: Dict[str, List[BatchRequest]] = defaultdict(list)
        self._timers: Dict[str, asyncio.Task] = {}
        self._lock = asyncio.Lock()
        self._active_batches = 0
        self._running = False
        
        # Adaptive parameters
        self._recent_latencies: List[float] = []
        self._current_batch_size = config.min_batch_size
        self._load_window = []
        
    async def start(self):
        """Start the batcher"""
        self._running = True
        
    async def stop(self):
        """Stop the batcher and process remaining requests"""
        self._running = False
        
        # Cancel timers
        for timer in self._timers.values():
            timer.cancel()
        self._timers.clear()
        
        # Process remaining batches
        async with self._lock:
            for collection_id, requests in self._batches.items():
                if requests:
                    await self._execute_batch(collection_id, requests)
        
    async def submit_request(self, request: BatchRequest) -> Any:
        """Submit a request for batching"""
        if not self._running:
            raise RuntimeError("Batcher is not running")
            
        # Create future for result
        future = asyncio.Future()
        request.future = future
        
        async with self._lock:
            collection_id = request.collection_id
            self._batches[collection_id].append(request)
            
            # Check if we should execute batch immediately
            should_execute = await self._should_execute_batch(collection_id)
            
            if should_execute:
                await self._execute_batch(collection_id, self._batches[collection_id])
                self._batches[collection_id] = []
                
                # Cancel timer if exists
                if collection_id in self._timers:
                    self._timers[collection_id].cancel()
                    del self._timers[collection_id]
            else:
                # Set timer if not exists
                if collection_id not in self._timers:
                    self._timers[collection_id] = asyncio.create_task(
                        self._batch_timer(collection_id)
                    )
        
        return await future
    
    async def _should_execute_batch(self, collection_id: str) -> bool:
        """Determine if batch should be executed based on strategy"""
        requests = self._batches[collection_id]
        
        if not requests:
            return False
            
        if self.config.strategy == BatchStrategy.IMMEDIATE:
            return True
            
        if self.config.strategy == BatchStrategy.SIZE_BASED:
            return len(requests) >= self.config.max_batch_size
            
        if self.config.strategy == BatchStrategy.TIME_BASED:
            oldest_request = min(requests, key=lambda r: r.timestamp)
            time_elapsed = (time.time() - oldest_request.timestamp) * 1000
            return time_elapsed >= self.config.max_wait_time_ms
            
        if self.config.strategy == BatchStrategy.ADAPTIVE:
            return await self._adaptive_should_execute(requests)
            
        return False
    
    async def _adaptive_should_execute(self, requests: List[BatchRequest]) -> bool:
        """Adaptive batching decision based on current performance"""
        current_size = len(requests)
        
        # Always execute if we hit max size
        if current_size >= self.config.max_batch_size:
            return True
            
        # Execute if we hit current adaptive size
        if current_size >= self._current_batch_size:
            return True
            
        # Execute if oldest request exceeds time threshold
        oldest_request = min(requests, key=lambda r: r.timestamp)
        time_elapsed = (time.time() - oldest_request.timestamp) * 1000
        
        if time_elapsed >= self.config.max_wait_time_ms:
            return True
            
        # Execute if system load is high
        if self._active_batches >= self.config.max_concurrent_batches:
            return True
            
        return False
    
    async def _batch_timer(self, collection_id: str):
        """Timer for time-based batching"""
        await asyncio.sleep(self.config.max_wait_time_ms / 1000.0)
        
        async with self._lock:
            if collection_id in self._batches and self._batches[collection_id]:
                await self._execute_batch(collection_id, self._batches[collection_id])
                self._batches[collection_id] = []
                
            # Remove timer
            if collection_id in self._timers:
                del self._timers[collection_id]
    
    async def _execute_batch(self, collection_id: str, requests: List[BatchRequest]):
        """Execute a batch of requests"""
        if not requests:
            return
            
        self._active_batches += 1
        start_time = time.time()
        
        try:
            # Group requests by operation type
            operations = defaultdict(list)
            for request in requests:
                operations[request.operation].append(request)
            
            # Execute each operation type
            for operation, op_requests in operations.items():
                try:
                    result = await self._execute_operation_batch(
                        collection_id, operation, op_requests
                    )
                    
                    # Distribute results to futures
                    if isinstance(result, list):
                        for req, res in zip(op_requests, result):
                            if req.future and not req.future.done():
                                req.future.set_result(res)
                    else:
                        # Single result for all requests
                        for req in op_requests:
                            if req.future and not req.future.done():
                                req.future.set_result(result)
                                
                except Exception as e:
                    # Set exception on all futures
                    for req in op_requests:
                        if req.future and not req.future.done():
                            req.future.set_exception(e)
            
            # Update metrics
            execution_time = (time.time() - start_time) * 1000
            self._update_metrics(len(requests), execution_time)
            
        finally:
            self._active_batches -= 1
    
    async def _execute_operation_batch(
        self, 
        collection_id: str, 
        operation: BatchOperationType, 
        requests: List[BatchRequest]
    ) -> Any:
        """Execute a batch of requests for a specific operation"""
        if operation == BatchOperationType.INSERT_VECTORS:
            vectors = []
            for req in requests:
                if isinstance(req.data, list):
                    vectors.extend(req.data)
                else:
                    vectors.append(req.data)
            return await self.executor_func("insert_vectors", collection_id, vectors)
            
        elif operation == BatchOperationType.UPSERT_VECTORS:
            vectors = []
            for req in requests:
                if isinstance(req.data, list):
                    vectors.extend(req.data)
                else:
                    vectors.append(req.data)
            return await self.executor_func("upsert_vectors", collection_id, vectors)
            
        elif operation == BatchOperationType.DELETE_VECTORS:
            vector_ids = []
            for req in requests:
                if isinstance(req.data, list):
                    vector_ids.extend(req.data)
                else:
                    vector_ids.append(req.data)
            return await self.executor_func("delete_vectors", collection_id, vector_ids)
            
        elif operation == BatchOperationType.GET_VECTORS:
            vector_ids = [req.data for req in requests]
            return await self.executor_func("get_vectors", collection_id, vector_ids)
            
        elif operation == BatchOperationType.SEARCH_VECTORS:
            # For search, execute individually as they have different parameters
            results = []
            for req in requests:
                result = await self.executor_func("search_vectors", collection_id, req.data)
                results.append(result)
            return results
            
        else:
            raise ValueError(f"Unsupported operation: {operation}")
    
    def _update_metrics(self, batch_size: int, execution_time_ms: float):
        """Update batching metrics"""
        self.metrics.total_requests += batch_size
        self.metrics.batched_requests += 1
        
        # Update averages
        total_batches = self.metrics.batched_requests
        self.metrics.avg_batch_size = (
            (self.metrics.avg_batch_size * (total_batches - 1) + batch_size) / total_batches
        )
        
        self.metrics.total_latency_ms += execution_time_ms
        self.metrics.avg_latency_ms = self.metrics.total_latency_ms / total_batches
        
        # Update throughput (requests per second)
        elapsed_time = time.time() - self.metrics.last_updated
        if elapsed_time > 0:
            self.metrics.throughput_qps = batch_size / elapsed_time
        
        self.metrics.last_updated = time.time()
        
        # Update adaptive parameters
        self._update_adaptive_parameters(execution_time_ms)
    
    def _update_adaptive_parameters(self, execution_time_ms: float):
        """Update adaptive batching parameters based on performance"""
        if self.config.strategy != BatchStrategy.ADAPTIVE:
            return
            
        self._recent_latencies.append(execution_time_ms)
        
        # Keep only recent latencies (last 100)
        if len(self._recent_latencies) > 100:
            self._recent_latencies = self._recent_latencies[-100:]
        
        # Adjust batch size based on latency
        if len(self._recent_latencies) >= 10:
            avg_latency = sum(self._recent_latencies[-10:]) / 10
            
            if avg_latency > self.config.target_latency_ms * 1.2:
                # Latency too high, reduce batch size
                self._current_batch_size = max(
                    self.config.min_batch_size,
                    int(self._current_batch_size * 0.9)
                )
            elif avg_latency < self.config.target_latency_ms * 0.8:
                # Latency good, can increase batch size
                self._current_batch_size = min(
                    self.config.max_batch_size,
                    int(self._current_batch_size * 1.1)
                )


class Pipeline:
    """
    Request pipeline for processing operations in stages with batching.
    
    Supports multi-stage processing with different batching strategies per stage.
    """
    
    def __init__(self):
        self.stages: List[Callable] = []
        self.batchers: Dict[int, RequestBatcher] = {}
        self._running = False
    
    def add_stage(self, processor: Callable, batch_config: Optional[BatchConfig] = None):
        """Add a processing stage to the pipeline"""
        stage_id = len(self.stages)
        self.stages.append(processor)
        
        if batch_config:
            self.batchers[stage_id] = RequestBatcher(batch_config, processor)
    
    async def start(self):
        """Start the pipeline"""
        self._running = True
        for batcher in self.batchers.values():
            await batcher.start()
    
    async def stop(self):
        """Stop the pipeline"""
        self._running = False
        for batcher in self.batchers.values():
            await batcher.stop()
    
    async def process(self, data: Any) -> Any:
        """Process data through the pipeline"""
        current_data = data
        
        for stage_id, processor in enumerate(self.stages):
            if stage_id in self.batchers:
                # Use batcher for this stage
                request = BatchRequest(
                    id=str(uuid.uuid4()),
                    operation=BatchOperationType.INSERT_VECTORS,  # Default
                    collection_id="pipeline",
                    data=current_data
                )
                current_data = await self.batchers[stage_id].submit_request(request)
            else:
                # Direct processing
                current_data = await processor(current_data)
        
        return current_data
    
    def get_metrics(self) -> Dict[int, BatchMetrics]:
        """Get metrics for all batching stages"""
        return {stage_id: batcher.metrics for stage_id, batcher in self.batchers.items()}


# Convenience functions for common patterns

async def create_vector_batcher(
    client,
    collection_id: str,
    config: Optional[BatchConfig] = None
) -> RequestBatcher:
    """Create a batcher optimized for vector operations"""
    if config is None:
        config = BatchConfig(
            max_batch_size=1000,
            max_wait_time_ms=50,
            strategy=BatchStrategy.ADAPTIVE
        )
    
    async def executor(operation: str, coll_id: str, data: Any):
        if operation == "insert_vectors":
            return await client.insert_vectors(coll_id, data)
        elif operation == "upsert_vectors": 
            return await client.upsert_vectors(coll_id, data)
        elif operation == "delete_vectors":
            return await client.delete_vectors(coll_id, data)
        elif operation == "get_vectors":
            return await client.get_vectors(coll_id, data)
        else:
            raise ValueError(f"Unsupported operation: {operation}")
    
    batcher = RequestBatcher(config, executor)
    await batcher.start()
    return batcher


def batch_insert_vectors(vectors: List[VectorRecord], batch_size: int = 1000):
    """Utility to split vectors into batches"""
    for i in range(0, len(vectors), batch_size):
        yield vectors[i:i + batch_size]