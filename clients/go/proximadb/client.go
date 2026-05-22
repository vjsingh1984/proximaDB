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
	"sync"
	"sync/atomic"
	"time"
)

// Client is the main interface for interacting with ProximaDB.
type Client interface {
	// Collection operations

	// CreateCollection creates a new vector collection.
	CreateCollection(ctx context.Context, req *CreateCollectionRequest) (*CollectionInfo, error)
	// ListCollections returns all collections.
	ListCollections(ctx context.Context) ([]*CollectionInfo, error)
	// GetCollection returns information about a specific collection.
	GetCollection(ctx context.Context, name string) (*CollectionInfo, error)
	// DeleteCollection deletes a collection.
	DeleteCollection(ctx context.Context, name string) error

	// Vector operations

	// InsertRecords inserts canonical records into a collection.
	InsertRecords(ctx context.Context, collection string, records []*ProximaRecord) error
	// UpsertRecords inserts or updates canonical records in a collection.
	UpsertRecords(ctx context.Context, collection string, records []*ProximaRecord) error
	// Insert inserts vectors into a collection.
	//
	// Deprecated: use InsertRecords with ProximaRecord.
	Insert(ctx context.Context, collection string, records []*VectorRecord) error
	// Upsert inserts or updates vectors in a collection.
	//
	// Deprecated: use UpsertRecords with ProximaRecord.
	Upsert(ctx context.Context, collection string, records []*VectorRecord) error
	// Search performs a vector similarity search.
	Search(ctx context.Context, collection string, query *SearchQuery) (*SearchResponse, error)
	// Get retrieves vectors by their IDs.
	Get(ctx context.Context, collection string, ids []string) ([]*VectorRecord, error)
	// Delete removes vectors by their IDs.
	Delete(ctx context.Context, collection string, ids []string) error

	// Batch operations

	// BatchInsert inserts vectors in batches with progress tracking.
	//
	// Deprecated: use InsertRecords with ProximaRecord batches.
	BatchInsert(ctx context.Context, collection string, records []*VectorRecord, opts *BatchOptions) (*BatchResult, error)
	// BatchSearch performs multiple searches in parallel.
	BatchSearch(ctx context.Context, collection string, queries []*SearchQuery, opts *BatchOptions) ([]*SearchResponse, error)

	// Streaming operations

	// StreamInsert returns a channel-based inserter for streaming inserts.
	StreamInsert(ctx context.Context, collection string, opts *StreamOptions) (Inserter, error)
	// StreamSearch returns a channel-based searcher for streaming searches.
	StreamSearch(ctx context.Context, collection string, opts *StreamOptions) (Searcher, error)

	// Health and lifecycle

	// Health checks the server health.
	Health(ctx context.Context) (*HealthStatus, error)
	// Close closes the client and releases resources.
	Close() error

	// Metrics returns client metrics.
	Metrics() *ClientMetrics
}

// BatchOptions configures batch operations.
type BatchOptions struct {
	// BatchSize is the number of records per batch (default: 1000).
	BatchSize int
	// Concurrency is the number of parallel workers (default: 4).
	Concurrency int
	// OnProgress is called after each batch completes.
	OnProgress func(processed, total int)
	// ContinueOnError continues processing even if a batch fails.
	ContinueOnError bool
}

// DefaultBatchOptions returns default batch options.
func DefaultBatchOptions() *BatchOptions {
	return &BatchOptions{
		BatchSize:       1000,
		Concurrency:     4,
		ContinueOnError: false,
	}
}

// BatchResult contains the result of a batch operation.
type BatchResult struct {
	// TotalProcessed is the total number of records processed.
	TotalProcessed int
	// SuccessCount is the number of successful operations.
	SuccessCount int
	// FailedCount is the number of failed operations.
	FailedCount int
	// Errors contains errors for failed operations.
	Errors []BatchError
	// Duration is the total time taken.
	Duration time.Duration
}

// StreamOptions configures streaming operations.
type StreamOptions struct {
	// BufferSize is the channel buffer size (default: 100).
	BufferSize int
	// FlushInterval is the interval to flush pending items (default: 100ms).
	FlushInterval time.Duration
	// MaxPending is the maximum number of pending items before blocking.
	MaxPending int
}

// DefaultStreamOptions returns default stream options.
func DefaultStreamOptions() *StreamOptions {
	return &StreamOptions{
		BufferSize:    100,
		FlushInterval: 100 * time.Millisecond,
		MaxPending:    1000,
	}
}

// Inserter is a streaming interface for inserting vectors.
type Inserter interface {
	// Send sends a vector record for insertion.
	Send(record *VectorRecord) error
	// SendBatch sends multiple vector records.
	SendBatch(records []*VectorRecord) error
	// Close closes the inserter and waits for all pending inserts.
	Close() (*BatchResult, error)
	// Errors returns a channel of insertion errors.
	Errors() <-chan error
}

// Searcher is a streaming interface for searching vectors.
type Searcher interface {
	// Send sends a search query.
	Send(query *SearchQuery) error
	// Results returns a channel of search results.
	Results() <-chan *SearchResponse
	// Close closes the searcher.
	Close() error
	// Errors returns a channel of search errors.
	Errors() <-chan error
}

// ClientMetrics tracks client performance metrics.
type ClientMetrics struct {
	// RequestCount is the total number of requests.
	RequestCount int64
	// SuccessCount is the number of successful requests.
	SuccessCount int64
	// ErrorCount is the number of failed requests.
	ErrorCount int64
	// RetryCount is the number of retried requests.
	RetryCount int64
	// TotalLatencyNs is the cumulative latency in nanoseconds.
	TotalLatencyNs int64
	// LastRequestTime is the timestamp of the last request.
	LastRequestTime time.Time
}

// AverageLatency returns the average request latency.
func (m *ClientMetrics) AverageLatency() time.Duration {
	if m.RequestCount == 0 {
		return 0
	}
	return time.Duration(m.TotalLatencyNs / m.RequestCount)
}

// SuccessRate returns the success rate as a percentage.
func (m *ClientMetrics) SuccessRate() float64 {
	if m.RequestCount == 0 {
		return 0
	}
	return float64(m.SuccessCount) / float64(m.RequestCount) * 100
}

// Adapter is the interface for protocol-specific implementations.
type Adapter interface {
	// Collection operations
	CreateCollection(ctx context.Context, req *CreateCollectionRequest) (*CollectionInfo, error)
	ListCollections(ctx context.Context) ([]*CollectionInfo, error)
	GetCollection(ctx context.Context, name string) (*CollectionInfo, error)
	DeleteCollection(ctx context.Context, name string) error

	// Vector operations
	InsertRecords(ctx context.Context, collection string, records []*ProximaRecord) error
	UpsertRecords(ctx context.Context, collection string, records []*ProximaRecord) error
	Insert(ctx context.Context, collection string, records []*VectorRecord) error
	Upsert(ctx context.Context, collection string, records []*VectorRecord) error
	Search(ctx context.Context, collection string, query *SearchQuery) (*SearchResponse, error)
	Get(ctx context.Context, collection string, ids []string) ([]*VectorRecord, error)
	Delete(ctx context.Context, collection string, ids []string) error

	// Health
	Health(ctx context.Context) (*HealthStatus, error)

	// Close
	Close() error
}

// client is the default implementation of Client.
type client struct {
	config      *Config
	adapter     Adapter
	mu          sync.RWMutex
	closed      bool
	metrics     *clientMetrics
	middlewares []Middleware
}

// clientMetrics is the internal metrics implementation with atomic operations.
type clientMetrics struct {
	requestCount    int64
	successCount    int64
	errorCount      int64
	retryCount      int64
	totalLatencyNs  int64
	lastRequestTime atomic.Value // time.Time
}

// Middleware is a function that wraps an operation.
type Middleware func(next OperationFunc) OperationFunc

// OperationFunc is a function that performs an operation.
type OperationFunc func(ctx context.Context) (interface{}, error)

// OperationContext provides context about the current operation.
type OperationContext struct {
	// Operation is the name of the operation (e.g., "Insert", "Search").
	Operation string
	// Collection is the collection name (if applicable).
	Collection string
	// StartTime is when the operation started.
	StartTime time.Time
}

// NewClient creates a new ProximaDB client with the given options.
func NewClient(opts ...Option) (Client, error) {
	config := defaultConfig()
	for _, opt := range opts {
		opt(config)
	}

	if err := config.Validate(); err != nil {
		return nil, err
	}

	return newClientWithConfig(config)
}

// newClientWithConfig creates a new client with the given configuration.
func newClientWithConfig(config *Config) (Client, error) {
	var adapter Adapter
	var err error

	switch config.Protocol {
	case ProtocolGRPC:
		adapter, err = newGRPCAdapter(config)
	default:
		adapter, err = newRESTAdapter(config)
	}

	if err != nil {
		return nil, err
	}

	metrics := &clientMetrics{}
	metrics.lastRequestTime.Store(time.Time{})

	return &client{
		config:      config,
		adapter:     adapter,
		metrics:     metrics,
		middlewares: config.Middlewares,
	}, nil
}

// WithMiddleware adds a middleware to the client.
// Middlewares are executed in the order they are added.
func (c *client) WithMiddleware(m Middleware) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.middlewares = append(c.middlewares, m)
}

// CreateCollection creates a new vector collection.
func (c *client) CreateCollection(ctx context.Context, req *CreateCollectionRequest) (*CollectionInfo, error) {
	if err := c.ensureOpen(); err != nil {
		return nil, err
	}
	return withRetry(ctx, c.config, func() (*CollectionInfo, error) {
		return c.adapter.CreateCollection(ctx, req)
	})
}

// ListCollections returns all collections.
func (c *client) ListCollections(ctx context.Context) ([]*CollectionInfo, error) {
	if err := c.ensureOpen(); err != nil {
		return nil, err
	}
	return withRetry(ctx, c.config, func() ([]*CollectionInfo, error) {
		return c.adapter.ListCollections(ctx)
	})
}

// GetCollection returns information about a specific collection.
func (c *client) GetCollection(ctx context.Context, name string) (*CollectionInfo, error) {
	if err := c.ensureOpen(); err != nil {
		return nil, err
	}
	return withRetry(ctx, c.config, func() (*CollectionInfo, error) {
		return c.adapter.GetCollection(ctx, name)
	})
}

// DeleteCollection deletes a collection.
func (c *client) DeleteCollection(ctx context.Context, name string) error {
	if err := c.ensureOpen(); err != nil {
		return err
	}
	_, err := withRetry(ctx, c.config, func() (struct{}, error) {
		return struct{}{}, c.adapter.DeleteCollection(ctx, name)
	})
	return err
}

// InsertRecords inserts canonical records into a collection.
func (c *client) InsertRecords(ctx context.Context, collection string, records []*ProximaRecord) error {
	if err := c.ensureOpen(); err != nil {
		return err
	}
	_, err := withRetry(ctx, c.config, func() (struct{}, error) {
		return struct{}{}, c.adapter.InsertRecords(ctx, collection, records)
	})
	return err
}

// UpsertRecords inserts or updates canonical records in a collection.
func (c *client) UpsertRecords(ctx context.Context, collection string, records []*ProximaRecord) error {
	if err := c.ensureOpen(); err != nil {
		return err
	}
	_, err := withRetry(ctx, c.config, func() (struct{}, error) {
		return struct{}{}, c.adapter.UpsertRecords(ctx, collection, records)
	})
	return err
}

// Insert inserts vectors into a collection.
//
// Deprecated: use InsertRecords with ProximaRecord.
func (c *client) Insert(ctx context.Context, collection string, records []*VectorRecord) error {
	if err := c.ensureOpen(); err != nil {
		return err
	}
	_, err := withRetry(ctx, c.config, func() (struct{}, error) {
		return struct{}{}, c.adapter.Insert(ctx, collection, records)
	})
	return err
}

// Upsert inserts or updates vectors in a collection.
//
// Deprecated: use UpsertRecords with ProximaRecord.
func (c *client) Upsert(ctx context.Context, collection string, records []*VectorRecord) error {
	if err := c.ensureOpen(); err != nil {
		return err
	}
	_, err := withRetry(ctx, c.config, func() (struct{}, error) {
		return struct{}{}, c.adapter.Upsert(ctx, collection, records)
	})
	return err
}

// Search performs a vector similarity search.
func (c *client) Search(ctx context.Context, collection string, query *SearchQuery) (*SearchResponse, error) {
	if err := c.ensureOpen(); err != nil {
		return nil, err
	}
	return withRetry(ctx, c.config, func() (*SearchResponse, error) {
		return c.adapter.Search(ctx, collection, query)
	})
}

// Get retrieves vectors by their IDs.
func (c *client) Get(ctx context.Context, collection string, ids []string) ([]*VectorRecord, error) {
	if err := c.ensureOpen(); err != nil {
		return nil, err
	}
	return withRetry(ctx, c.config, func() ([]*VectorRecord, error) {
		return c.adapter.Get(ctx, collection, ids)
	})
}

// Delete removes vectors by their IDs.
func (c *client) Delete(ctx context.Context, collection string, ids []string) error {
	if err := c.ensureOpen(); err != nil {
		return err
	}
	_, err := withRetry(ctx, c.config, func() (struct{}, error) {
		return struct{}{}, c.adapter.Delete(ctx, collection, ids)
	})
	return err
}

// Health checks the server health.
func (c *client) Health(ctx context.Context) (*HealthStatus, error) {
	if err := c.ensureOpen(); err != nil {
		return nil, err
	}
	return withRetry(ctx, c.config, func() (*HealthStatus, error) {
		return c.adapter.Health(ctx)
	})
}

// Close closes the client and releases resources.
func (c *client) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	if c.closed {
		return nil
	}

	c.closed = true
	return c.adapter.Close()
}

// ensureOpen checks if the client is still open.
func (c *client) ensureOpen() error {
	c.mu.RLock()
	defer c.mu.RUnlock()

	if c.closed {
		return NewError(ErrCodeInvalidArgument, "client is closed")
	}
	return nil
}

// withRetry executes a function with retry logic.
func withRetry[T any](ctx context.Context, config *Config, fn func() (T, error)) (T, error) {
	var result T
	var err error

	delay := config.RetryDelay
	for attempt := 0; attempt <= config.MaxRetries; attempt++ {
		result, err = fn()
		if err == nil {
			return result, nil
		}

		// Don't retry if the error is not retryable
		if !IsRetryable(err) {
			return result, err
		}

		// Don't retry if context is done
		select {
		case <-ctx.Done():
			return result, WrapError(ErrCodeTimeout, "context cancelled", ctx.Err())
		default:
		}

		// Don't sleep on the last attempt
		if attempt < config.MaxRetries {
			timer := time.NewTimer(delay)
			select {
			case <-ctx.Done():
				timer.Stop()
				return result, WrapError(ErrCodeTimeout, "context cancelled", ctx.Err())
			case <-timer.C:
				// Exponential backoff
				delay *= 2
			}
		}
	}

	return result, err
}

// BatchInsert inserts vectors in batches with progress tracking.
func (c *client) BatchInsert(ctx context.Context, collection string, records []*VectorRecord, opts *BatchOptions) (*BatchResult, error) {
	if err := c.ensureOpen(); err != nil {
		return nil, err
	}

	if opts == nil {
		opts = DefaultBatchOptions()
	}

	startTime := time.Now()
	total := len(records)
	result := &BatchResult{}

	// Create batches
	var batches [][]*VectorRecord
	for i := 0; i < total; i += opts.BatchSize {
		end := i + opts.BatchSize
		if end > total {
			end = total
		}
		batches = append(batches, records[i:end])
	}

	// Process batches with concurrency control
	sem := make(chan struct{}, opts.Concurrency)
	var wg sync.WaitGroup
	var mu sync.Mutex
	processed := 0

	for batchIdx, batch := range batches {
		// Check context
		select {
		case <-ctx.Done():
			result.Duration = time.Since(startTime)
			return result, WrapError(ErrCodeTimeout, "batch insert cancelled", ctx.Err())
		default:
		}

		sem <- struct{}{}
		wg.Add(1)

		go func(idx int, b []*VectorRecord) {
			defer wg.Done()
			defer func() { <-sem }()

			err := c.Insert(ctx, collection, b)

			mu.Lock()
			if err != nil {
				result.FailedCount += len(b)
				result.Errors = append(result.Errors, BatchError{
					ID:    "", // Batch error
					Error: err.Error(),
				})
				if !opts.ContinueOnError {
					mu.Unlock()
					return
				}
			} else {
				result.SuccessCount += len(b)
			}
			processed += len(b)
			result.TotalProcessed = processed

			if opts.OnProgress != nil {
				opts.OnProgress(processed, total)
			}
			mu.Unlock()
		}(batchIdx, batch)
	}

	wg.Wait()
	result.Duration = time.Since(startTime)
	return result, nil
}

// BatchSearch performs multiple searches in parallel.
func (c *client) BatchSearch(ctx context.Context, collection string, queries []*SearchQuery, opts *BatchOptions) ([]*SearchResponse, error) {
	if err := c.ensureOpen(); err != nil {
		return nil, err
	}

	if opts == nil {
		opts = DefaultBatchOptions()
	}

	total := len(queries)
	results := make([]*SearchResponse, total)

	// Process searches with concurrency control
	sem := make(chan struct{}, opts.Concurrency)
	var wg sync.WaitGroup
	var mu sync.Mutex
	var firstErr error
	processed := 0

	for i, query := range queries {
		// Check context
		select {
		case <-ctx.Done():
			return results, WrapError(ErrCodeTimeout, "batch search cancelled", ctx.Err())
		default:
		}

		sem <- struct{}{}
		wg.Add(1)

		go func(idx int, q *SearchQuery) {
			defer wg.Done()
			defer func() { <-sem }()

			resp, err := c.Search(ctx, collection, q)

			mu.Lock()
			if err != nil {
				if firstErr == nil {
					firstErr = err
				}
				if !opts.ContinueOnError {
					mu.Unlock()
					return
				}
			} else {
				results[idx] = resp
			}
			processed++

			if opts.OnProgress != nil {
				opts.OnProgress(processed, total)
			}
			mu.Unlock()
		}(i, query)
	}

	wg.Wait()
	return results, firstErr
}

// StreamInsert returns a channel-based inserter for streaming inserts.
func (c *client) StreamInsert(ctx context.Context, collection string, opts *StreamOptions) (Inserter, error) {
	if err := c.ensureOpen(); err != nil {
		return nil, err
	}

	if opts == nil {
		opts = DefaultStreamOptions()
	}

	return newStreamInserter(ctx, c, collection, opts), nil
}

// StreamSearch returns a channel-based searcher for streaming searches.
func (c *client) StreamSearch(ctx context.Context, collection string, opts *StreamOptions) (Searcher, error) {
	if err := c.ensureOpen(); err != nil {
		return nil, err
	}

	if opts == nil {
		opts = DefaultStreamOptions()
	}

	return newStreamSearcher(ctx, c, collection, opts), nil
}

// Metrics returns a snapshot of client metrics.
func (c *client) Metrics() *ClientMetrics {
	lastReq := c.metrics.lastRequestTime.Load()
	var lastTime time.Time
	if lastReq != nil {
		lastTime = lastReq.(time.Time)
	}

	return &ClientMetrics{
		RequestCount:    atomic.LoadInt64(&c.metrics.requestCount),
		SuccessCount:    atomic.LoadInt64(&c.metrics.successCount),
		ErrorCount:      atomic.LoadInt64(&c.metrics.errorCount),
		RetryCount:      atomic.LoadInt64(&c.metrics.retryCount),
		TotalLatencyNs:  atomic.LoadInt64(&c.metrics.totalLatencyNs),
		LastRequestTime: lastTime,
	}
}

// streamInserter implements the Inserter interface.
type streamInserter struct {
	ctx        context.Context
	client     *client
	collection string
	opts       *StreamOptions
	records    chan *VectorRecord
	errors     chan error
	done       chan struct{}
	wg         sync.WaitGroup
	result     *BatchResult
	resultMu   sync.Mutex
}

func newStreamInserter(ctx context.Context, c *client, collection string, opts *StreamOptions) *streamInserter {
	si := &streamInserter{
		ctx:        ctx,
		client:     c,
		collection: collection,
		opts:       opts,
		records:    make(chan *VectorRecord, opts.BufferSize),
		errors:     make(chan error, opts.BufferSize),
		done:       make(chan struct{}),
		result:     &BatchResult{},
	}

	si.wg.Add(1)
	go si.worker()

	return si
}

func (si *streamInserter) worker() {
	defer si.wg.Done()

	batch := make([]*VectorRecord, 0, si.opts.BufferSize)
	ticker := time.NewTicker(si.opts.FlushInterval)
	defer ticker.Stop()

	flush := func() {
		if len(batch) == 0 {
			return
		}

		err := si.client.Insert(si.ctx, si.collection, batch)
		si.resultMu.Lock()
		if err != nil {
			si.result.FailedCount += len(batch)
			select {
			case si.errors <- err:
			default:
			}
		} else {
			si.result.SuccessCount += len(batch)
		}
		si.result.TotalProcessed += len(batch)
		si.resultMu.Unlock()

		batch = batch[:0]
	}

	for {
		select {
		case <-si.ctx.Done():
			flush()
			return
		case <-si.done:
			flush()
			return
		case record := <-si.records:
			batch = append(batch, record)
			if len(batch) >= si.opts.BufferSize {
				flush()
			}
		case <-ticker.C:
			flush()
		}
	}
}

func (si *streamInserter) Send(record *VectorRecord) error {
	select {
	case <-si.ctx.Done():
		return si.ctx.Err()
	case si.records <- record:
		return nil
	}
}

func (si *streamInserter) SendBatch(records []*VectorRecord) error {
	for _, r := range records {
		if err := si.Send(r); err != nil {
			return err
		}
	}
	return nil
}

func (si *streamInserter) Close() (*BatchResult, error) {
	close(si.done)
	si.wg.Wait()
	close(si.errors)
	return si.result, nil
}

func (si *streamInserter) Errors() <-chan error {
	return si.errors
}

// streamSearcher implements the Searcher interface.
type streamSearcher struct {
	ctx        context.Context
	client     *client
	collection string
	opts       *StreamOptions
	queries    chan *SearchQuery
	results    chan *SearchResponse
	errors     chan error
	done       chan struct{}
	wg         sync.WaitGroup
}

func newStreamSearcher(ctx context.Context, c *client, collection string, opts *StreamOptions) *streamSearcher {
	ss := &streamSearcher{
		ctx:        ctx,
		client:     c,
		collection: collection,
		opts:       opts,
		queries:    make(chan *SearchQuery, opts.BufferSize),
		results:    make(chan *SearchResponse, opts.BufferSize),
		errors:     make(chan error, opts.BufferSize),
		done:       make(chan struct{}),
	}

	ss.wg.Add(1)
	go ss.worker()

	return ss
}

func (ss *streamSearcher) worker() {
	defer ss.wg.Done()

	for {
		select {
		case <-ss.ctx.Done():
			return
		case <-ss.done:
			return
		case query := <-ss.queries:
			if query == nil {
				continue
			}
			resp, err := ss.client.Search(ss.ctx, ss.collection, query)
			if err != nil {
				select {
				case ss.errors <- err:
				default:
				}
			} else {
				select {
				case ss.results <- resp:
				default:
				}
			}
		}
	}
}

func (ss *streamSearcher) Send(query *SearchQuery) error {
	select {
	case <-ss.ctx.Done():
		return ss.ctx.Err()
	case ss.queries <- query:
		return nil
	}
}

func (ss *streamSearcher) Results() <-chan *SearchResponse {
	return ss.results
}

func (ss *streamSearcher) Close() error {
	close(ss.done)
	ss.wg.Wait()
	close(ss.results)
	close(ss.errors)
	return nil
}

func (ss *streamSearcher) Errors() <-chan error {
	return ss.errors
}
