"""
ProximaDB Circuit Breaker Pattern

Implements circuit breaker, retry patterns, and resilience mechanisms
for handling failures and preventing cascading system failures.
"""

import asyncio
import logging
import random
import time
from collections.abc import Callable
from contextlib import asynccontextmanager
from dataclasses import dataclass
from typing import Any

from .exceptions import NetworkError, ProximaDBError, ServerError, TimeoutError
from .resilience import (
    AdvancedRetryPolicy,
    CircuitBreakerPolicy,
    CircuitState,
    RetryStrategy,
)

# Enums and policies imported from resilience module


@dataclass
class CircuitBreakerMetrics:
    """Metrics for circuit breaker"""

    total_requests: int = 0
    successful_requests: int = 0
    failed_requests: int = 0
    rejected_requests: int = 0
    state_changes: int = 0
    last_failure_time: float | None = None
    last_success_time: float | None = None
    current_failures: int = 0

    @property
    def failure_rate(self) -> float:
        """Calculate current failure rate"""
        total = self.successful_requests + self.failed_requests
        if total == 0:
            return 0.0
        return self.failed_requests / total

    @property
    def success_rate(self) -> float:
        """Calculate current success rate"""
        return 1.0 - self.failure_rate


# Configuration classes imported from resilience module
CircuitBreakerConfig = CircuitBreakerPolicy  # Alias for clarity
RetryConfig = AdvancedRetryPolicy  # Alias for clarity


class CircuitBreaker:
    """
    Circuit breaker implementation with state management and metrics.

    Provides automatic failure detection, request blocking during outages,
    and recovery testing to prevent cascading failures.
    """

    def __init__(self, name: str, config: CircuitBreakerConfig):
        self.name = name
        self.config = config
        self.state = CircuitState.CLOSED
        self.metrics = CircuitBreakerMetrics()

        self._state_change_time = time.time()
        self._half_open_calls = 0
        self._logger = logging.getLogger(__name__)

        # Sliding window for metrics
        self._recent_calls: list[Tuple[float, bool]] = []  # (timestamp, success)

    async def __aenter__(self):
        """Async context manager entry"""
        await self._check_state()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Async context manager exit"""
        if exc_type is None:
            await self._record_success()
        else:
            await self._record_failure()

    async def call(self, func: Callable, *args, **kwargs) -> Any:
        """Execute function with circuit breaker protection"""
        await self._check_state()

        if self.state == CircuitState.OPEN:
            self.metrics.rejected_requests += 1
            raise CircuitBreakerOpenError(f"Circuit breaker {self.name} is OPEN")

        start_time = time.time()
        self.metrics.total_requests += 1

        try:
            if asyncio.iscoroutinefunction(func):
                result = await func(*args, **kwargs)
            else:
                result = func(*args, **kwargs)

            execution_time = (time.time() - start_time) * 1000

            # Check for slow calls
            if execution_time > self.config.slow_call_threshold_ms:
                await self._record_failure()
            else:
                await self._record_success()

            return result

        except Exception:
            await self._record_failure()
            raise

    async def _check_state(self):
        """Check and potentially change circuit breaker state"""
        current_time = time.time()

        if self.state == CircuitState.OPEN:
            # Check if timeout has passed
            if current_time - self._state_change_time >= self.config.timeout_seconds:
                await self._change_state(CircuitState.HALF_OPEN)

        elif self.state == CircuitState.HALF_OPEN:
            # Reset call counter if too much time has passed
            if current_time - self._state_change_time >= self.config.timeout_seconds:
                self._half_open_calls = 0

        # Clean old metrics
        await self._clean_old_metrics()

    async def _record_success(self):
        """Record a successful call"""
        current_time = time.time()
        self.metrics.successful_requests += 1
        self.metrics.last_success_time = current_time
        self.metrics.current_failures = 0

        self._recent_calls.append((current_time, True))

        if self.state == CircuitState.HALF_OPEN:
            self._half_open_calls += 1
            if self._half_open_calls >= self.config.success_threshold:
                await self._change_state(CircuitState.CLOSED)

    async def _record_failure(self):
        """Record a failed call"""
        current_time = time.time()
        self.metrics.failed_requests += 1
        self.metrics.last_failure_time = current_time
        self.metrics.current_failures += 1

        self._recent_calls.append((current_time, False))

        # Check if we should open the circuit
        if self.state == CircuitState.CLOSED:
            if self.metrics.current_failures >= self.config.failure_threshold or (
                self.metrics.total_requests >= self.config.minimum_requests
                and self.metrics.failure_rate >= self.config.failure_rate_threshold
            ):
                await self._change_state(CircuitState.OPEN)

        elif self.state == CircuitState.HALF_OPEN:
            # Any failure in half-open state opens the circuit
            await self._change_state(CircuitState.OPEN)

    async def _change_state(self, new_state: CircuitState):
        """Change circuit breaker state"""
        old_state = self.state
        self.state = new_state
        self._state_change_time = time.time()
        self.metrics.state_changes += 1

        if new_state == CircuitState.HALF_OPEN:
            self._half_open_calls = 0
        elif new_state == CircuitState.CLOSED:
            self.metrics.current_failures = 0

        self._logger.info(
            f"Circuit breaker {self.name} state changed: {old_state} -> {new_state}"
        )

    async def _clean_old_metrics(self):
        """Clean old metrics outside the window"""
        current_time = time.time()
        cutoff_time = current_time - self.config.metrics_window_seconds

        self._recent_calls = [
            (timestamp, success)
            for timestamp, success in self._recent_calls
            if timestamp >= cutoff_time
        ]

    def get_metrics(self) -> CircuitBreakerMetrics:
        """Get current metrics"""
        return self.metrics

    def reset(self):
        """Reset circuit breaker to closed state"""
        self.state = CircuitState.CLOSED
        self.metrics = CircuitBreakerMetrics()
        self._state_change_time = time.time()
        self._half_open_calls = 0
        self._recent_calls.clear()

        self._logger.info(f"Circuit breaker {self.name} reset to CLOSED state")


class RetryMechanism:
    """
    Intelligent retry mechanism with multiple strategies and conditions.

    Provides exponential backoff, jitter, and configurable retry conditions
    to handle transient failures gracefully.
    """

    def __init__(self, config: RetryConfig):
        self.config = config
        self._logger = logging.getLogger(__name__)

    async def execute(self, func: Callable, *args, **kwargs) -> Any:
        """Execute function with retry logic"""
        last_exception = None

        for attempt in range(self.config.max_attempts):
            try:
                if asyncio.iscoroutinefunction(func):
                    return await func(*args, **kwargs)
                else:
                    return func(*args, **kwargs)

            except Exception as e:
                last_exception = e

                # Check if we should retry
                if not self._should_retry(e, attempt):
                    break

                # Calculate delay
                delay = await self._calculate_delay(attempt)

                self._logger.warning(
                    f"Attempt {attempt + 1} failed: {e}. " f"Retrying in {delay:.2f}s"
                )

                await asyncio.sleep(delay)

        # All attempts failed
        raise last_exception

    def _should_retry(self, exception: Exception, attempt: int) -> bool:
        """Determine if we should retry based on exception and attempt count"""
        if attempt >= self.config.max_attempts - 1:
            return False

        # Check exception types
        if isinstance(exception, TimeoutError) and self.config.retry_on_timeout:
            return True

        if isinstance(exception, NetworkError) and self.config.retry_on_network_error:
            return True

        if isinstance(exception, ServerError) and self.config.retry_on_server_error:
            return True

        # Check custom retryable exceptions
        exception_name = exception.__class__.__name__
        if exception_name in self.config.retryable_exceptions:
            return True

        return False

    async def _calculate_delay(self, attempt: int) -> float:
        """Calculate delay for next attempt"""
        if self.config.strategy == RetryStrategy.FIXED_DELAY:
            delay = self.config.initial_delay_ms / 1000.0

        elif self.config.strategy == RetryStrategy.EXPONENTIAL_BACKOFF:
            delay = (
                self.config.initial_delay_ms * (self.config.backoff_multiplier**attempt)
            ) / 1000.0

        elif self.config.strategy == RetryStrategy.LINEAR_BACKOFF:
            delay = (self.config.initial_delay_ms * (attempt + 1)) / 1000.0

        else:  # JITTERED
            base_delay = (
                self.config.initial_delay_ms * (self.config.backoff_multiplier**attempt)
            ) / 1000.0
            jitter = random.uniform(0, self.config.jitter_max_ms / 1000.0)
            delay = base_delay + jitter

        # Apply jitter if enabled
        if (
            self.config.jitter_enabled
            and self.config.strategy != RetryStrategy.JITTERED
        ):
            jitter = random.uniform(0, self.config.jitter_max_ms / 1000.0)
            delay += jitter

        # Ensure we don't exceed max delay
        max_delay = self.config.max_delay_ms / 1000.0
        return min(delay, max_delay)


class ResilientClient:
    """
    Resilient client wrapper that combines circuit breaker and retry mechanisms.

    Provides a unified interface for resilient operation execution with
    comprehensive failure handling and recovery.
    """

    def __init__(
        self,
        name: str,
        circuit_config: CircuitBreakerConfig | None = None,
        retry_config: RetryConfig | None = None,
    ):
        self.name = name
        self.circuit_breaker = CircuitBreaker(
            name, circuit_config or CircuitBreakerConfig()
        )
        self.retry_mechanism = RetryMechanism(retry_config or RetryConfig())
        self._logger = logging.getLogger(__name__)

    async def execute(self, func: Callable, *args, **kwargs) -> Any:
        """Execute function with full resilience (circuit breaker + retry)"""

        async def protected_call():
            return await self.circuit_breaker.call(func, *args, **kwargs)

        return await self.retry_mechanism.execute(protected_call)

    @asynccontextmanager
    async def resilient_context(self):
        """Context manager for resilient operations"""
        try:
            async with self.circuit_breaker:
                yield self
        except Exception as e:
            self._logger.error(f"Resilient operation failed: {e}")
            raise

    def get_health_status(self) -> dict[str, Any]:
        """Get health status of the resilient client"""
        metrics = self.circuit_breaker.get_metrics()

        return {
            "name": self.name,
            "circuit_breaker": {
                "state": self.circuit_breaker.state.value,
                "failure_rate": metrics.failure_rate,
                "success_rate": metrics.success_rate,
                "total_requests": metrics.total_requests,
                "rejected_requests": metrics.rejected_requests,
            },
            "health_score": self._calculate_health_score(metrics),
        }

    def _calculate_health_score(self, metrics: CircuitBreakerMetrics) -> float:
        """Calculate health score (0.0 to 1.0)"""
        if metrics.total_requests == 0:
            return 1.0

        # Base score on success rate
        base_score = metrics.success_rate

        # Penalty for being in OPEN state
        if self.circuit_breaker.state == CircuitState.OPEN:
            base_score *= 0.1
        elif self.circuit_breaker.state == CircuitState.HALF_OPEN:
            base_score *= 0.5

        # Penalty for recent failures
        current_time = time.time()
        if (
            metrics.last_failure_time and current_time - metrics.last_failure_time < 60
        ):  # Last minute
            base_score *= 0.8

        return max(0.0, min(1.0, base_score))


# Custom exceptions


class CircuitBreakerOpenError(ProximaDBError):
    """Raised when circuit breaker is open"""

    def __init__(self, message: str = "Circuit breaker is open"):
        super().__init__(message)
        self.error_code = "CIRCUIT_BREAKER_OPEN"


class RetryExhaustedError(ProximaDBError):
    """Raised when all retry attempts are exhausted"""

    def __init__(
        self, message: str = "All retry attempts exhausted", attempts: int = 0
    ):
        super().__init__(message)
        self.error_code = "RETRY_EXHAUSTED"
        self.attempts = attempts


# Convenience functions and decorators


def circuit_breaker(
    name: str, failure_threshold: int = 5, timeout_seconds: float = 60.0
):
    """Decorator for circuit breaker protection"""
    config = CircuitBreakerConfig(
        failure_threshold=failure_threshold, timeout_seconds=timeout_seconds
    )
    breaker = CircuitBreaker(name, config)

    def decorator(func):
        async def async_wrapper(*args, **kwargs):
            return await breaker.call(func, *args, **kwargs)

        def sync_wrapper(*args, **kwargs):
            return asyncio.run(async_wrapper(*args, **kwargs))

        if asyncio.iscoroutinefunction(func):
            return async_wrapper
        else:
            return sync_wrapper

    return decorator


def retry(
    max_attempts: int = 3, strategy: RetryStrategy = RetryStrategy.EXPONENTIAL_BACKOFF
):
    """Decorator for retry functionality"""
    config = RetryConfig(max_attempts=max_attempts, strategy=strategy)
    retry_mechanism = RetryMechanism(config)

    def decorator(func):
        async def async_wrapper(*args, **kwargs):
            return await retry_mechanism.execute(func, *args, **kwargs)

        def sync_wrapper(*args, **kwargs):
            return asyncio.run(async_wrapper(*args, **kwargs))

        if asyncio.iscoroutinefunction(func):
            return async_wrapper
        else:
            return sync_wrapper

    return decorator


def resilient(
    name: str,
    circuit_config: CircuitBreakerConfig | None = None,
    retry_config: RetryConfig | None = None,
):
    """Decorator combining circuit breaker and retry"""
    client = ResilientClient(name, circuit_config, retry_config)

    def decorator(func):
        async def async_wrapper(*args, **kwargs):
            return await client.execute(func, *args, **kwargs)

        def sync_wrapper(*args, **kwargs):
            return asyncio.run(async_wrapper(*args, **kwargs))

        if asyncio.iscoroutinefunction(func):
            return async_wrapper
        else:
            return sync_wrapper

    return decorator


async def create_resilient_client(
    name: str,
    circuit_config: CircuitBreakerConfig | None = None,
    retry_config: RetryConfig | None = None,
) -> ResilientClient:
    """Create a resilient client instance"""
    return ResilientClient(name, circuit_config, retry_config)
