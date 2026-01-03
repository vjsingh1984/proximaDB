// Copyright 2025 Vijaykumar Singh
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package proximadb

import (
	"context"
	"log"
	"time"
)

// LoggingMiddleware creates a middleware that logs all operations.
func LoggingMiddleware(logger *log.Logger) Middleware {
	return func(next OperationFunc) OperationFunc {
		return func(ctx context.Context) (interface{}, error) {
			start := time.Now()
			result, err := next(ctx)
			duration := time.Since(start)

			if err != nil {
				logger.Printf("operation failed: %v (duration: %v)", err, duration)
			} else {
				logger.Printf("operation succeeded (duration: %v)", duration)
			}

			return result, err
		}
	}
}

// MetricsMiddleware creates a middleware that records operation metrics.
type MetricsRecorder interface {
	RecordRequest(operation string, duration time.Duration, err error)
}

func MetricsMiddleware(recorder MetricsRecorder, operation string) Middleware {
	return func(next OperationFunc) OperationFunc {
		return func(ctx context.Context) (interface{}, error) {
			start := time.Now()
			result, err := next(ctx)
			duration := time.Since(start)

			recorder.RecordRequest(operation, duration, err)

			return result, err
		}
	}
}

// TimeoutMiddleware creates a middleware that enforces a timeout.
func TimeoutMiddleware(timeout time.Duration) Middleware {
	return func(next OperationFunc) OperationFunc {
		return func(ctx context.Context) (interface{}, error) {
			ctx, cancel := context.WithTimeout(ctx, timeout)
			defer cancel()

			return next(ctx)
		}
	}
}

// CircuitBreakerConfig configures the circuit breaker middleware.
type CircuitBreakerConfig struct {
	// FailureThreshold is the number of failures before opening the circuit.
	FailureThreshold int
	// SuccessThreshold is the number of successes to close the circuit.
	SuccessThreshold int
	// Timeout is how long the circuit stays open before trying again.
	Timeout time.Duration
}

// DefaultCircuitBreakerConfig returns default circuit breaker settings.
func DefaultCircuitBreakerConfig() *CircuitBreakerConfig {
	return &CircuitBreakerConfig{
		FailureThreshold: 5,
		SuccessThreshold: 2,
		Timeout:          30 * time.Second,
	}
}

// circuitBreaker implements a simple circuit breaker pattern.
type circuitBreaker struct {
	config      *CircuitBreakerConfig
	failures    int
	successes   int
	state       circuitState
	lastFailure time.Time
}

type circuitState int

const (
	circuitClosed circuitState = iota
	circuitOpen
	circuitHalfOpen
)

// CircuitBreakerMiddleware creates a middleware that implements the circuit breaker pattern.
func CircuitBreakerMiddleware(config *CircuitBreakerConfig) Middleware {
	if config == nil {
		config = DefaultCircuitBreakerConfig()
	}

	cb := &circuitBreaker{
		config: config,
		state:  circuitClosed,
	}

	return func(next OperationFunc) OperationFunc {
		return func(ctx context.Context) (interface{}, error) {
			// Check circuit state
			if cb.state == circuitOpen {
				if time.Since(cb.lastFailure) > cb.config.Timeout {
					cb.state = circuitHalfOpen
					cb.successes = 0
				} else {
					return nil, NewError(ErrCodeUnavailable, "circuit breaker is open")
				}
			}

			result, err := next(ctx)

			// Update circuit state based on result
			if err != nil {
				cb.failures++
				cb.lastFailure = time.Now()
				if cb.failures >= cb.config.FailureThreshold {
					cb.state = circuitOpen
				}
				if cb.state == circuitHalfOpen {
					cb.state = circuitOpen
				}
			} else {
				if cb.state == circuitHalfOpen {
					cb.successes++
					if cb.successes >= cb.config.SuccessThreshold {
						cb.state = circuitClosed
						cb.failures = 0
					}
				} else {
					cb.failures = 0
				}
			}

			return result, err
		}
	}
}

// RateLimiterConfig configures the rate limiter middleware.
type RateLimiterConfig struct {
	// RequestsPerSecond is the maximum number of requests per second.
	RequestsPerSecond float64
	// BurstSize is the maximum burst size.
	BurstSize int
}

// DefaultRateLimiterConfig returns default rate limiter settings.
func DefaultRateLimiterConfig() *RateLimiterConfig {
	return &RateLimiterConfig{
		RequestsPerSecond: 100,
		BurstSize:         10,
	}
}

// rateLimiter implements a simple token bucket rate limiter.
type rateLimiter struct {
	config    *RateLimiterConfig
	tokens    float64
	lastCheck time.Time
}

// RateLimiterMiddleware creates a middleware that limits the request rate.
func RateLimiterMiddleware(config *RateLimiterConfig) Middleware {
	if config == nil {
		config = DefaultRateLimiterConfig()
	}

	rl := &rateLimiter{
		config:    config,
		tokens:    float64(config.BurstSize),
		lastCheck: time.Now(),
	}

	return func(next OperationFunc) OperationFunc {
		return func(ctx context.Context) (interface{}, error) {
			now := time.Now()
			elapsed := now.Sub(rl.lastCheck).Seconds()
			rl.lastCheck = now

			// Add tokens based on elapsed time
			rl.tokens += elapsed * rl.config.RequestsPerSecond
			if rl.tokens > float64(rl.config.BurstSize) {
				rl.tokens = float64(rl.config.BurstSize)
			}

			// Check if we have tokens available
			if rl.tokens < 1 {
				return nil, NewError(ErrCodeRateLimited, "rate limit exceeded")
			}

			// Consume a token
			rl.tokens--

			return next(ctx)
		}
	}
}

// TracingMiddleware creates a middleware that adds tracing information.
type TraceContext struct {
	TraceID   string
	SpanID    string
	ParentID  string
	Operation string
	StartTime time.Time
}

type traceContextKey struct{}

// WithTraceContext adds trace context to the context.
func WithTraceContext(ctx context.Context, tc *TraceContext) context.Context {
	return context.WithValue(ctx, traceContextKey{}, tc)
}

// GetTraceContext retrieves trace context from the context.
func GetTraceContext(ctx context.Context) *TraceContext {
	tc, _ := ctx.Value(traceContextKey{}).(*TraceContext)
	return tc
}

// TraceIDGenerator generates trace IDs.
type TraceIDGenerator interface {
	GenerateTraceID() string
	GenerateSpanID() string
}

// TracingMiddleware creates a middleware that adds distributed tracing.
func TracingMiddleware(generator TraceIDGenerator, operation string) Middleware {
	return func(next OperationFunc) OperationFunc {
		return func(ctx context.Context) (interface{}, error) {
			parentTC := GetTraceContext(ctx)

			tc := &TraceContext{
				TraceID:   generator.GenerateTraceID(),
				SpanID:    generator.GenerateSpanID(),
				Operation: operation,
				StartTime: time.Now(),
			}

			if parentTC != nil {
				tc.TraceID = parentTC.TraceID
				tc.ParentID = parentTC.SpanID
			}

			ctx = WithTraceContext(ctx, tc)

			return next(ctx)
		}
	}
}

// RetryConfig configures the retry middleware.
type RetryConfig struct {
	// MaxAttempts is the maximum number of retry attempts.
	MaxAttempts int
	// InitialDelay is the initial delay between retries.
	InitialDelay time.Duration
	// MaxDelay is the maximum delay between retries.
	MaxDelay time.Duration
	// Multiplier is the backoff multiplier.
	Multiplier float64
	// RetryableErrors is a function that determines if an error is retryable.
	RetryableErrors func(error) bool
}

// DefaultRetryConfig returns default retry settings.
func DefaultRetryConfig() *RetryConfig {
	return &RetryConfig{
		MaxAttempts:     3,
		InitialDelay:   100 * time.Millisecond,
		MaxDelay:       10 * time.Second,
		Multiplier:     2.0,
		RetryableErrors: IsRetryable,
	}
}

// RetryMiddleware creates a middleware that retries failed operations.
func RetryMiddleware(config *RetryConfig) Middleware {
	if config == nil {
		config = DefaultRetryConfig()
	}

	return func(next OperationFunc) OperationFunc {
		return func(ctx context.Context) (interface{}, error) {
			var lastErr error
			delay := config.InitialDelay

			for attempt := 0; attempt < config.MaxAttempts; attempt++ {
				result, err := next(ctx)
				if err == nil {
					return result, nil
				}

				lastErr = err

				// Check if error is retryable
				if !config.RetryableErrors(err) {
					return nil, err
				}

				// Check context
				select {
				case <-ctx.Done():
					return nil, ctx.Err()
				default:
				}

				// Wait before retry (except on last attempt)
				if attempt < config.MaxAttempts-1 {
					timer := time.NewTimer(delay)
					select {
					case <-ctx.Done():
						timer.Stop()
						return nil, ctx.Err()
					case <-timer.C:
					}

					// Increase delay with backoff
					delay = time.Duration(float64(delay) * config.Multiplier)
					if delay > config.MaxDelay {
						delay = config.MaxDelay
					}
				}
			}

			return nil, lastErr
		}
	}
}

// CacheConfig configures the cache middleware.
type CacheConfig struct {
	// TTL is the time-to-live for cached entries.
	TTL time.Duration
	// MaxEntries is the maximum number of cache entries.
	MaxEntries int
}

// DefaultCacheConfig returns default cache settings.
func DefaultCacheConfig() *CacheConfig {
	return &CacheConfig{
		TTL:        5 * time.Minute,
		MaxEntries: 1000,
	}
}

// Cache is the interface for cache implementations.
type Cache interface {
	Get(key string) (interface{}, bool)
	Set(key string, value interface{}, ttl time.Duration)
	Delete(key string)
}

// CacheKeyGenerator generates cache keys.
type CacheKeyGenerator func(ctx context.Context) string

// CacheMiddleware creates a middleware that caches operation results.
func CacheMiddleware(cache Cache, config *CacheConfig, keyGen CacheKeyGenerator) Middleware {
	if config == nil {
		config = DefaultCacheConfig()
	}

	return func(next OperationFunc) OperationFunc {
		return func(ctx context.Context) (interface{}, error) {
			key := keyGen(ctx)

			// Check cache
			if cached, ok := cache.Get(key); ok {
				return cached, nil
			}

			// Execute operation
			result, err := next(ctx)
			if err != nil {
				return nil, err
			}

			// Cache result
			cache.Set(key, result, config.TTL)

			return result, nil
		}
	}
}

// ChainMiddleware chains multiple middlewares together.
func ChainMiddleware(middlewares ...Middleware) Middleware {
	return func(next OperationFunc) OperationFunc {
		for i := len(middlewares) - 1; i >= 0; i-- {
			next = middlewares[i](next)
		}
		return next
	}
}
