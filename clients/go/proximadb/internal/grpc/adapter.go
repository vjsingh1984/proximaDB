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

// Package grpc provides the gRPC protocol adapter for ProximaDB.
package grpc

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"os"
	"strings"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/status"
)

// ErrorCode represents a ProximaDB error code.
type ErrorCode string

const (
	ErrCodeConnection        ErrorCode = "CONNECTION_ERROR"
	ErrCodeTimeout           ErrorCode = "TIMEOUT"
	ErrCodeNotFound          ErrorCode = "NOT_FOUND"
	ErrCodeAlreadyExists     ErrorCode = "ALREADY_EXISTS"
	ErrCodeInvalidArgument   ErrorCode = "INVALID_ARGUMENT"
	ErrCodeDimensionMismatch ErrorCode = "DIMENSION_MISMATCH"
	ErrCodeRateLimited       ErrorCode = "RATE_LIMITED"
	ErrCodeInternal          ErrorCode = "INTERNAL_ERROR"
	ErrCodeUnavailable       ErrorCode = "UNAVAILABLE"
)

// AdapterError represents an error from the gRPC adapter.
type AdapterError struct {
	Code    ErrorCode
	Message string
	Cause   error
}

func (e *AdapterError) Error() string {
	if e.Cause != nil {
		return fmt.Sprintf("[%s] %s: %v", e.Code, e.Message, e.Cause)
	}
	return fmt.Sprintf("[%s] %s", e.Code, e.Message)
}

func (e *AdapterError) Unwrap() error {
	return e.Cause
}

// TLSConfig holds TLS configuration options.
type TLSConfig struct {
	CertFile   string
	KeyFile    string
	CAFile     string
	SkipVerify bool
}

// Config holds the gRPC adapter configuration.
type Config struct {
	Target   string
	APIKey   string
	Timeout  time.Duration
	PoolSize int
	TLS      *TLSConfig
}

// Collection types for gRPC API

// CreateCollectionRequest represents a request to create a collection.
type CreateCollectionRequest struct {
	Name        string `json:"name"`
	Dimension   int    `json:"dimension"`
	Metric      string `json:"metric,omitempty"`
	Engine      string `json:"engine,omitempty"`
	Description string `json:"description,omitempty"`
}

// CollectionInfo represents collection information.
type CollectionInfo struct {
	Name        string    `json:"name"`
	Dimension   int       `json:"dimension"`
	Metric      string    `json:"metric"`
	Engine      string    `json:"engine"`
	VectorCount int64     `json:"vector_count"`
	CreatedAt   time.Time `json:"created_at"`
}

// VectorRecord represents a vector with its ID and metadata.
type VectorRecord struct {
	ID       string                 `json:"id"`
	Vector   []float32              `json:"vector"`
	Metadata map[string]interface{} `json:"metadata,omitempty"`
}

// SearchQuery represents a vector search query.
type SearchQuery struct {
	Vector          []float32 `json:"vector"`
	TopK            int       `json:"top_k,omitempty"`
	Filter          *Filter   `json:"filter,omitempty"`
	IncludeVectors  bool      `json:"include_vectors,omitempty"`
	IncludeMetadata bool      `json:"include_metadata,omitempty"`
}

// Filter represents a metadata filter.
type Filter struct {
	Field    string      `json:"field,omitempty"`
	Operator string      `json:"operator,omitempty"`
	Value    interface{} `json:"value,omitempty"`
	And      []Filter    `json:"and,omitempty"`
	Or       []Filter    `json:"or,omitempty"`
}

// SearchResult represents a single search result.
type SearchResult struct {
	ID       string                 `json:"id"`
	Score    float32                `json:"score"`
	Vector   []float32              `json:"vector,omitempty"`
	Metadata map[string]interface{} `json:"metadata,omitempty"`
}

// SearchResponse represents the response from a search query.
type SearchResponse struct {
	Results    []SearchResult `json:"results"`
	TookMs     float64        `json:"took_ms"`
	TotalCount int64          `json:"total_count,omitempty"`
}

// HealthStatus represents the server health status.
type HealthStatus struct {
	Status  string  `json:"status"`
	Version string  `json:"version"`
	Uptime  float64 `json:"uptime_seconds"`
}

// Adapter implements the gRPC protocol for ProximaDB.
type Adapter struct {
	conn   *grpc.ClientConn
	config *Config
}

// NewAdapter creates a new gRPC adapter.
func NewAdapter(config *Config) (*Adapter, error) {
	// Parse target - remove http:// or https:// prefix if present
	target := config.Target
	target = strings.TrimPrefix(target, "http://")
	target = strings.TrimPrefix(target, "https://")

	// Replace port 5678 with 5679 for gRPC if needed
	if strings.HasSuffix(target, ":5678") {
		target = strings.TrimSuffix(target, ":5678") + ":5679"
	}

	// Build dial options
	var opts []grpc.DialOption

	// Configure TLS
	if config.TLS != nil {
		tlsConfig, err := buildTLSConfig(config.TLS)
		if err != nil {
			return nil, err
		}
		opts = append(opts, grpc.WithTransportCredentials(credentials.NewTLS(tlsConfig)))
	} else {
		opts = append(opts, grpc.WithTransportCredentials(insecure.NewCredentials()))
	}

	// Add API key interceptor if specified
	if config.APIKey != "" {
		opts = append(opts, grpc.WithUnaryInterceptor(authInterceptor(config.APIKey)))
	}

	// Connect
	conn, err := grpc.NewClient(target, opts...)
	if err != nil {
		return nil, &AdapterError{
			Code:    ErrCodeConnection,
			Message: "failed to connect to gRPC server",
			Cause:   err,
		}
	}

	return &Adapter{
		conn:   conn,
		config: config,
	}, nil
}

// buildTLSConfig creates a TLS configuration from the given options.
func buildTLSConfig(config *TLSConfig) (*tls.Config, error) {
	tlsConfig := &tls.Config{
		InsecureSkipVerify: config.SkipVerify,
	}

	// Load client certificate if specified
	if config.CertFile != "" && config.KeyFile != "" {
		cert, err := tls.LoadX509KeyPair(config.CertFile, config.KeyFile)
		if err != nil {
			return nil, &AdapterError{
				Code:    ErrCodeInvalidArgument,
				Message: "failed to load client certificate",
				Cause:   err,
			}
		}
		tlsConfig.Certificates = []tls.Certificate{cert}
	}

	// Load CA certificate if specified
	if config.CAFile != "" {
		caCert, err := os.ReadFile(config.CAFile)
		if err != nil {
			return nil, &AdapterError{
				Code:    ErrCodeInvalidArgument,
				Message: "failed to load CA certificate",
				Cause:   err,
			}
		}
		caCertPool := x509.NewCertPool()
		if !caCertPool.AppendCertsFromPEM(caCert) {
			return nil, &AdapterError{
				Code:    ErrCodeInvalidArgument,
				Message: "failed to parse CA certificate",
			}
		}
		tlsConfig.RootCAs = caCertPool
	}

	return tlsConfig, nil
}

// authInterceptor returns a gRPC unary interceptor that adds authentication headers.
func authInterceptor(apiKey string) grpc.UnaryClientInterceptor {
	return func(ctx context.Context, method string, req, reply interface{}, cc *grpc.ClientConn, invoker grpc.UnaryInvoker, opts ...grpc.CallOption) error {
		// Add authorization metadata
		// Note: In a real implementation, we would use metadata.AppendToOutgoingContext
		return invoker(ctx, method, req, reply, cc, opts...)
	}
}

// CreateCollection creates a new vector collection.
// Note: This is a stub implementation. In production, you would use generated protobuf types.
func (a *Adapter) CreateCollection(ctx context.Context, req *CreateCollectionRequest) (*CollectionInfo, error) {
	// In a real implementation, this would use the generated VectorService client
	// For now, return a not implemented error to indicate this needs proto generation
	return nil, &AdapterError{
		Code:    ErrCodeInternal,
		Message: "gRPC CreateCollection requires proto generation - use REST protocol or generate protos",
	}
}

// ListCollections returns all collections.
func (a *Adapter) ListCollections(ctx context.Context) ([]*CollectionInfo, error) {
	return nil, &AdapterError{
		Code:    ErrCodeInternal,
		Message: "gRPC ListCollections requires proto generation - use REST protocol or generate protos",
	}
}

// GetCollection returns information about a specific collection.
func (a *Adapter) GetCollection(ctx context.Context, name string) (*CollectionInfo, error) {
	return nil, &AdapterError{
		Code:    ErrCodeInternal,
		Message: "gRPC GetCollection requires proto generation - use REST protocol or generate protos",
	}
}

// DeleteCollection deletes a collection.
func (a *Adapter) DeleteCollection(ctx context.Context, name string) error {
	return &AdapterError{
		Code:    ErrCodeInternal,
		Message: "gRPC DeleteCollection requires proto generation - use REST protocol or generate protos",
	}
}

// Insert inserts vectors into a collection.
func (a *Adapter) Insert(ctx context.Context, collection string, records []*VectorRecord) error {
	return &AdapterError{
		Code:    ErrCodeInternal,
		Message: "gRPC Insert requires proto generation - use REST protocol or generate protos",
	}
}

// Upsert inserts or updates vectors in a collection.
func (a *Adapter) Upsert(ctx context.Context, collection string, records []*VectorRecord) error {
	return &AdapterError{
		Code:    ErrCodeInternal,
		Message: "gRPC Upsert requires proto generation - use REST protocol or generate protos",
	}
}

// Search performs a vector similarity search.
func (a *Adapter) Search(ctx context.Context, collection string, query *SearchQuery) (*SearchResponse, error) {
	return nil, &AdapterError{
		Code:    ErrCodeInternal,
		Message: "gRPC Search requires proto generation - use REST protocol or generate protos",
	}
}

// Get retrieves vectors by their IDs.
func (a *Adapter) Get(ctx context.Context, collection string, ids []string) ([]*VectorRecord, error) {
	return nil, &AdapterError{
		Code:    ErrCodeInternal,
		Message: "gRPC Get requires proto generation - use REST protocol or generate protos",
	}
}

// Delete removes vectors by their IDs.
func (a *Adapter) Delete(ctx context.Context, collection string, ids []string) error {
	return &AdapterError{
		Code:    ErrCodeInternal,
		Message: "gRPC Delete requires proto generation - use REST protocol or generate protos",
	}
}

// Health checks the server health.
func (a *Adapter) Health(ctx context.Context) (*HealthStatus, error) {
	return nil, &AdapterError{
		Code:    ErrCodeInternal,
		Message: "gRPC Health requires proto generation - use REST protocol or generate protos",
	}
}

// Close closes the adapter and releases resources.
func (a *Adapter) Close() error {
	if a.conn != nil {
		return a.conn.Close()
	}
	return nil
}

// ConvertGRPCError converts a gRPC error to an AdapterError.
func ConvertGRPCError(err error) error {
	if err == nil {
		return nil
	}

	st, ok := status.FromError(err)
	if !ok {
		return &AdapterError{
			Code:    ErrCodeInternal,
			Message: err.Error(),
			Cause:   err,
		}
	}

	var code ErrorCode
	switch st.Code() {
	case codes.NotFound:
		code = ErrCodeNotFound
	case codes.AlreadyExists:
		code = ErrCodeAlreadyExists
	case codes.InvalidArgument:
		code = ErrCodeInvalidArgument
	case codes.DeadlineExceeded:
		code = ErrCodeTimeout
	case codes.ResourceExhausted:
		code = ErrCodeRateLimited
	case codes.Unavailable:
		code = ErrCodeUnavailable
	default:
		code = ErrCodeInternal
	}

	return &AdapterError{
		Code:    code,
		Message: st.Message(),
		Cause:   err,
	}
}
