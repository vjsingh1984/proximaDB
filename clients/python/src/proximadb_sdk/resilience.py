"""
ProximaDB Resilience Configuration

Consolidated resilience patterns configuration including retry policies,
circuit breaker settings, and other fault tolerance mechanisms.
"""

from enum import Enum

from pydantic import BaseModel, Field


class RetryStrategy(str, Enum):
    """Strategies for retry backoff"""

    FIXED_DELAY = "fixed_delay"  # Fixed delay between retries
    EXPONENTIAL_BACKOFF = "exponential_backoff"  # Exponential backoff
    LINEAR_BACKOFF = "linear_backoff"  # Linear backoff
    JITTERED = "jittered"  # Random jitter to avoid thundering herd


class CircuitState(str, Enum):
    """Circuit breaker states"""

    CLOSED = "closed"  # Normal operation
    OPEN = "open"  # Blocking requests
    HALF_OPEN = "half_open"  # Testing if service recovered


class NetworkRetryPolicy(BaseModel):
    """
    Basic retry policy for network requests.
    Used by HTTP clients and basic network operations.
    """

    max_retries: int = Field(default=3, ge=0, le=10)
    backoff_factor: float = Field(default=2.0, ge=1.0, le=10.0)
    max_backoff: float = Field(
        default=60.0, ge=1.0, le=300.0
    )  # Keep existing field name for compatibility
    retry_on_timeout: bool = Field(default=True)
    retry_on_connection_error: bool = Field(default=True)
    retry_on_server_error: bool = Field(default=True)
    retry_status_codes: list[int] = Field(
        default_factory=lambda: [429, 500, 502, 503, 504]
    )


class AdvancedRetryPolicy(BaseModel):
    """
    Advanced retry policy with multiple strategies and fine-grained control.
    Used by circuit breakers and resilient operations.
    """

    max_attempts: int = Field(default=3, ge=1, le=10)
    strategy: RetryStrategy = Field(default=RetryStrategy.EXPONENTIAL_BACKOFF)

    # Delay settings
    initial_delay_ms: int = Field(default=100, ge=10, le=5000)
    max_delay_ms: int = Field(default=30000, ge=100)
    backoff_multiplier: float = Field(default=2.0, ge=1.0, le=10.0)

    # Jitter to prevent thundering herd
    jitter_enabled: bool = Field(default=True)
    jitter_max_ms: int = Field(default=1000, ge=0)

    # Retry conditions
    retry_on_timeout: bool = Field(default=True)
    retry_on_network_error: bool = Field(default=True)
    retry_on_server_error: bool = Field(default=False)

    # Custom retry conditions
    retryable_exceptions: list[str] = Field(default_factory=list)


class CircuitBreakerPolicy(BaseModel):
    """
    Circuit breaker policy for preventing cascading failures.
    """

    failure_threshold: int = Field(default=5, ge=1, le=100)
    success_threshold: int = Field(default=2, ge=1, le=10)
    timeout_seconds: float = Field(default=60.0, ge=1.0, le=3600.0)

    # Failure detection
    failure_rate_threshold: float = Field(default=0.5, ge=0.1, le=1.0)
    minimum_requests: int = Field(default=10, ge=1)

    # Half-open state
    half_open_max_calls: int = Field(default=3, ge=1, le=10)

    # Monitoring
    metrics_window_seconds: int = Field(default=300, ge=60)
    slow_call_threshold_ms: int = Field(default=5000, ge=100)


class ResilienceConfig(BaseModel):
    """
    Comprehensive resilience configuration combining all patterns.
    """

    # Retry policies
    network_retry: NetworkRetryPolicy = Field(default_factory=NetworkRetryPolicy)
    advanced_retry: AdvancedRetryPolicy = Field(default_factory=AdvancedRetryPolicy)

    # Circuit breaker
    circuit_breaker: CircuitBreakerPolicy = Field(default_factory=CircuitBreakerPolicy)

    # Global settings
    enable_circuit_breaker: bool = Field(default=True)
    enable_retry: bool = Field(default=True)
    enable_metrics: bool = Field(default=True)


# Backward compatibility aliases
RetryConfig = NetworkRetryPolicy  # For config.py compatibility
CircuitBreakerConfig = CircuitBreakerPolicy  # Clear naming
