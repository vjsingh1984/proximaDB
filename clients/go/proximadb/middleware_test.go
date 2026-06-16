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

package proximadb_test

import (
	"bytes"
	"context"
	"errors"
	"log"
	"testing"
	"time"

	"github.com/proximadb/proximadb-go/proximadb"
)

// TestLoggingMiddleware tests the logging middleware.
func TestLoggingMiddleware(t *testing.T) {
	var buf bytes.Buffer
	logger := log.New(&buf, "", 0)

	middleware := proximadb.LoggingMiddleware(logger)

	// Test successful operation
	op := middleware(func(ctx context.Context) (interface{}, error) {
		return "success", nil
	})

	result, err := op(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result != "success" {
		t.Errorf("expected 'success', got %v", result)
	}
	if !bytes.Contains(buf.Bytes(), []byte("succeeded")) {
		t.Error("expected log to contain 'succeeded'")
	}

	// Test failed operation
	buf.Reset()
	failOp := middleware(func(ctx context.Context) (interface{}, error) {
		return nil, errors.New("test error")
	})

	_, err = failOp(context.Background())
	if err == nil {
		t.Error("expected error, got nil")
	}
	if !bytes.Contains(buf.Bytes(), []byte("failed")) {
		t.Error("expected log to contain 'failed'")
	}
}

// TestTimeoutMiddleware tests the timeout middleware.
func TestTimeoutMiddleware(t *testing.T) {
	middleware := proximadb.TimeoutMiddleware(50 * time.Millisecond)

	// Test operation that completes in time
	t.Run("completes in time", func(t *testing.T) {
		op := middleware(func(ctx context.Context) (interface{}, error) {
			return "done", nil
		})

		result, err := op(context.Background())
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if result != "done" {
			t.Errorf("expected 'done', got %v", result)
		}
	})

	// Test operation that times out
	t.Run("times out", func(t *testing.T) {
		op := middleware(func(ctx context.Context) (interface{}, error) {
			select {
			case <-time.After(200 * time.Millisecond):
				return "done", nil
			case <-ctx.Done():
				return nil, ctx.Err()
			}
		})

		_, err := op(context.Background())
		if err == nil {
			t.Error("expected timeout error")
		}
	})
}

// TestCircuitBreakerMiddleware tests the circuit breaker middleware.
func TestCircuitBreakerMiddleware(t *testing.T) {
	config := &proximadb.CircuitBreakerConfig{
		FailureThreshold: 2,
		SuccessThreshold: 1,
		Timeout:          100 * time.Millisecond,
	}

	middleware := proximadb.CircuitBreakerMiddleware(config)

	failCount := 0
	op := middleware(func(ctx context.Context) (interface{}, error) {
		failCount++
		if failCount <= 3 {
			return nil, errors.New("simulated failure")
		}
		return "success", nil
	})

	// First failure
	_, err := op(context.Background())
	if err == nil {
		t.Error("expected error on first call")
	}

	// Second failure - should trip the circuit
	_, err = op(context.Background())
	if err == nil {
		t.Error("expected error on second call")
	}

	// Third call - circuit should be open
	_, err = op(context.Background())
	if err == nil {
		t.Error("expected circuit breaker error")
	}
	// Circuit breaker returns unavailable error which may or may not be retryable
	// depending on implementation; just verify we got an error above

	// Wait for timeout
	time.Sleep(150 * time.Millisecond)

	// Circuit should be half-open, reset counter for success
	failCount = 10 // Skip failures
	_, err = op(context.Background())
	if err != nil {
		t.Errorf("expected success after timeout, got: %v", err)
	}
}

// TestRateLimiterMiddleware tests the rate limiter middleware.
func TestRateLimiterMiddleware(t *testing.T) {
	config := &proximadb.RateLimiterConfig{
		RequestsPerSecond: 10,
		BurstSize:         2,
	}

	middleware := proximadb.RateLimiterMiddleware(config)

	op := middleware(func(ctx context.Context) (interface{}, error) {
		return "done", nil
	})

	// First two calls should succeed (burst)
	for i := 0; i < 2; i++ {
		_, err := op(context.Background())
		if err != nil {
			t.Errorf("expected success on call %d, got: %v", i+1, err)
		}
	}

	// Third call should be rate limited
	_, err := op(context.Background())
	if err == nil {
		t.Error("expected rate limit error")
	}
	if !proximadb.IsRateLimited(err) {
		t.Errorf("expected rate limited error, got: %v", err)
	}
}

// TestRetryMiddleware tests the retry middleware.
func TestRetryMiddleware(t *testing.T) {
	config := &proximadb.RetryConfig{
		MaxAttempts:     3,
		InitialDelay:    10 * time.Millisecond,
		MaxDelay:        100 * time.Millisecond,
		Multiplier:      2.0,
		RetryableErrors: proximadb.IsRetryable,
	}

	t.Run("succeeds on first try", func(t *testing.T) {
		middleware := proximadb.RetryMiddleware(config)
		calls := 0
		op := middleware(func(ctx context.Context) (interface{}, error) {
			calls++
			return "success", nil
		})

		result, err := op(context.Background())
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if result != "success" {
			t.Errorf("expected 'success', got %v", result)
		}
		if calls != 1 {
			t.Errorf("expected 1 call, got %d", calls)
		}
	})

	t.Run("succeeds on retry", func(t *testing.T) {
		middleware := proximadb.RetryMiddleware(config)
		calls := 0
		op := middleware(func(ctx context.Context) (interface{}, error) {
			calls++
			if calls < 2 {
				return nil, proximadb.NewError(proximadb.ErrCodeTimeout, "simulated timeout")
			}
			return "success", nil
		})

		result, err := op(context.Background())
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if result != "success" {
			t.Errorf("expected 'success', got %v", result)
		}
		if calls != 2 {
			t.Errorf("expected 2 calls, got %d", calls)
		}
	})

	t.Run("fails after max retries", func(t *testing.T) {
		middleware := proximadb.RetryMiddleware(config)
		calls := 0
		op := middleware(func(ctx context.Context) (interface{}, error) {
			calls++
			return nil, proximadb.NewError(proximadb.ErrCodeTimeout, "simulated timeout")
		})

		_, err := op(context.Background())
		if err == nil {
			t.Error("expected error after max retries")
		}
		if calls != 3 {
			t.Errorf("expected 3 calls, got %d", calls)
		}
	})

	t.Run("does not retry non-retryable errors", func(t *testing.T) {
		middleware := proximadb.RetryMiddleware(config)
		calls := 0
		op := middleware(func(ctx context.Context) (interface{}, error) {
			calls++
			return nil, proximadb.NewError(proximadb.ErrCodeNotFound, "not found")
		})

		_, err := op(context.Background())
		if err == nil {
			t.Error("expected error")
		}
		if calls != 1 {
			t.Errorf("expected 1 call (no retry), got %d", calls)
		}
	})
}

// TestTraceContext tests trace context functions.
func TestTraceContext(t *testing.T) {
	ctx := context.Background()

	// No trace context initially
	tc := proximadb.GetTraceContext(ctx)
	if tc != nil {
		t.Error("expected nil trace context")
	}

	// Add trace context
	tc = &proximadb.TraceContext{
		TraceID:   "trace-123",
		SpanID:    "span-456",
		Operation: "test",
		StartTime: time.Now(),
	}
	ctx = proximadb.WithTraceContext(ctx, tc)

	// Retrieve trace context
	retrieved := proximadb.GetTraceContext(ctx)
	if retrieved == nil {
		t.Fatal("expected trace context")
	}
	if retrieved.TraceID != "trace-123" {
		t.Errorf("expected trace ID 'trace-123', got '%s'", retrieved.TraceID)
	}
	if retrieved.SpanID != "span-456" {
		t.Errorf("expected span ID 'span-456', got '%s'", retrieved.SpanID)
	}
}

// TestChainMiddleware tests chaining multiple middlewares.
func TestChainMiddleware(t *testing.T) {
	order := make([]int, 0)

	m1 := func(next proximadb.OperationFunc) proximadb.OperationFunc {
		return func(ctx context.Context) (interface{}, error) {
			order = append(order, 1)
			result, err := next(ctx)
			order = append(order, -1)
			return result, err
		}
	}

	m2 := func(next proximadb.OperationFunc) proximadb.OperationFunc {
		return func(ctx context.Context) (interface{}, error) {
			order = append(order, 2)
			result, err := next(ctx)
			order = append(order, -2)
			return result, err
		}
	}

	m3 := func(next proximadb.OperationFunc) proximadb.OperationFunc {
		return func(ctx context.Context) (interface{}, error) {
			order = append(order, 3)
			result, err := next(ctx)
			order = append(order, -3)
			return result, err
		}
	}

	chain := proximadb.ChainMiddleware(m1, m2, m3)

	op := chain(func(ctx context.Context) (interface{}, error) {
		order = append(order, 0)
		return "done", nil
	})

	result, err := op(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result != "done" {
		t.Errorf("expected 'done', got %v", result)
	}

	// Check order: should be 1, 2, 3, 0, -3, -2, -1
	expected := []int{1, 2, 3, 0, -3, -2, -1}
	if len(order) != len(expected) {
		t.Fatalf("expected %d calls, got %d: %v", len(expected), len(order), order)
	}
	for i, v := range expected {
		if order[i] != v {
			t.Errorf("expected order[%d] = %d, got %d", i, v, order[i])
		}
	}
}

// TestDefaultConfigs tests default configuration values.
func TestDefaultConfigs(t *testing.T) {
	t.Run("CircuitBreakerConfig", func(t *testing.T) {
		config := proximadb.DefaultCircuitBreakerConfig()
		if config.FailureThreshold != 5 {
			t.Errorf("expected failure threshold 5, got %d", config.FailureThreshold)
		}
		if config.SuccessThreshold != 2 {
			t.Errorf("expected success threshold 2, got %d", config.SuccessThreshold)
		}
		if config.Timeout != 30*time.Second {
			t.Errorf("expected timeout 30s, got %v", config.Timeout)
		}
	})

	t.Run("RateLimiterConfig", func(t *testing.T) {
		config := proximadb.DefaultRateLimiterConfig()
		if config.RequestsPerSecond != 100 {
			t.Errorf("expected requests per second 100, got %f", config.RequestsPerSecond)
		}
		if config.BurstSize != 10 {
			t.Errorf("expected burst size 10, got %d", config.BurstSize)
		}
	})

	t.Run("RetryConfig", func(t *testing.T) {
		config := proximadb.DefaultRetryConfig()
		if config.MaxAttempts != 3 {
			t.Errorf("expected max attempts 3, got %d", config.MaxAttempts)
		}
		if config.InitialDelay != 100*time.Millisecond {
			t.Errorf("expected initial delay 100ms, got %v", config.InitialDelay)
		}
		if config.MaxDelay != 10*time.Second {
			t.Errorf("expected max delay 10s, got %v", config.MaxDelay)
		}
		if config.Multiplier != 2.0 {
			t.Errorf("expected multiplier 2.0, got %f", config.Multiplier)
		}
	})

	t.Run("CacheConfig", func(t *testing.T) {
		config := proximadb.DefaultCacheConfig()
		if config.TTL != 5*time.Minute {
			t.Errorf("expected TTL 5m, got %v", config.TTL)
		}
		if config.MaxEntries != 1000 {
			t.Errorf("expected max entries 1000, got %d", config.MaxEntries)
		}
	})
}

// mockCache implements the Cache interface for testing.
type mockCache struct {
	data map[string]interface{}
}

func newMockCache() *mockCache {
	return &mockCache{data: make(map[string]interface{})}
}

func (c *mockCache) Get(key string) (interface{}, bool) {
	v, ok := c.data[key]
	return v, ok
}

func (c *mockCache) Set(key string, value interface{}, ttl time.Duration) {
	c.data[key] = value
}

func (c *mockCache) Delete(key string) {
	delete(c.data, key)
}

// TestCacheMiddleware tests the cache middleware.
func TestCacheMiddleware(t *testing.T) {
	cache := newMockCache()
	config := proximadb.DefaultCacheConfig()

	keyGen := func(ctx context.Context) string {
		return "test-key"
	}

	middleware := proximadb.CacheMiddleware(cache, config, keyGen)

	calls := 0
	op := middleware(func(ctx context.Context) (interface{}, error) {
		calls++
		return "result", nil
	})

	// First call - cache miss
	result, err := op(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result != "result" {
		t.Errorf("expected 'result', got %v", result)
	}
	if calls != 1 {
		t.Errorf("expected 1 call, got %d", calls)
	}

	// Second call - cache hit
	result, err = op(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result != "result" {
		t.Errorf("expected 'result', got %v", result)
	}
	if calls != 1 {
		t.Errorf("expected still 1 call (cached), got %d", calls)
	}
}

// mockMetricsRecorder implements MetricsRecorder for testing.
type mockMetricsRecorder struct {
	operations []string
	durations  []time.Duration
	errors     []error
}

func (r *mockMetricsRecorder) RecordRequest(operation string, duration time.Duration, err error) {
	r.operations = append(r.operations, operation)
	r.durations = append(r.durations, duration)
	r.errors = append(r.errors, err)
}

// TestMetricsMiddleware tests the metrics middleware.
func TestMetricsMiddleware(t *testing.T) {
	recorder := &mockMetricsRecorder{}

	middleware := proximadb.MetricsMiddleware(recorder, "test-operation")

	op := middleware(func(ctx context.Context) (interface{}, error) {
		time.Sleep(10 * time.Millisecond)
		return "done", nil
	})

	_, err := op(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(recorder.operations) != 1 {
		t.Fatalf("expected 1 recorded operation, got %d", len(recorder.operations))
	}
	if recorder.operations[0] != "test-operation" {
		t.Errorf("expected operation 'test-operation', got '%s'", recorder.operations[0])
	}
	if recorder.durations[0] < 10*time.Millisecond {
		t.Errorf("expected duration >= 10ms, got %v", recorder.durations[0])
	}
	if recorder.errors[0] != nil {
		t.Errorf("expected nil error, got %v", recorder.errors[0])
	}
}
