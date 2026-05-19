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

// Package rest provides the REST protocol adapter for ProximaDB.
package rest

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"
)

// ErrorCode represents a ProximaDB error code.
type ErrorCode string

const (
	ErrCodeConnection       ErrorCode = "CONNECTION_ERROR"
	ErrCodeTimeout          ErrorCode = "TIMEOUT"
	ErrCodeNotFound         ErrorCode = "NOT_FOUND"
	ErrCodeAlreadyExists    ErrorCode = "ALREADY_EXISTS"
	ErrCodeInvalidArgument  ErrorCode = "INVALID_ARGUMENT"
	ErrCodeDimensionMismatch ErrorCode = "DIMENSION_MISMATCH"
	ErrCodeRateLimited      ErrorCode = "RATE_LIMITED"
	ErrCodeInternal         ErrorCode = "INTERNAL_ERROR"
	ErrCodeUnavailable      ErrorCode = "UNAVAILABLE"
)

// AdapterError represents an error from the REST adapter.
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

// Config holds the REST adapter configuration.
type Config struct {
	BaseURL    string
	APIKey     string
	Timeout    time.Duration
	TLS        *TLSConfig
}

// Adapter implements the REST protocol for ProximaDB.
type Adapter struct {
	client  *http.Client
	baseURL string
	headers map[string]string
}

// NewAdapter creates a new REST adapter.
func NewAdapter(config *Config) (*Adapter, error) {
	// Parse and validate base URL
	parsedURL, err := url.Parse(config.BaseURL)
	if err != nil {
		return nil, &AdapterError{
			Code:    ErrCodeInvalidArgument,
			Message: "invalid base URL",
			Cause:   err,
		}
	}

	// Normalize the base URL (remove trailing slash)
	baseURL := strings.TrimSuffix(parsedURL.String(), "/")

	// Build HTTP client
	transport := &http.Transport{
		MaxIdleConns:        100,
		MaxIdleConnsPerHost: 100,
		IdleConnTimeout:     90 * time.Second,
	}

	// Configure TLS if specified
	if config.TLS != nil {
		tlsConfig, err := buildTLSConfig(config.TLS)
		if err != nil {
			return nil, err
		}
		transport.TLSClientConfig = tlsConfig
	}

	client := &http.Client{
		Transport: transport,
		Timeout:   config.Timeout,
	}

	// Build default headers
	headers := map[string]string{
		"Content-Type": "application/json",
		"Accept":       "application/json",
		"User-Agent":   "proximadb-go/1.0.0",
	}

	if config.APIKey != "" {
		headers["Authorization"] = "Bearer " + config.APIKey
	}

	return &Adapter{
		client:  client,
		baseURL: baseURL,
		headers: headers,
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

// Collection types for REST API

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

// ProximaRecord represents the canonical record payload with optional vector data.
type ProximaRecord struct {
	ID     string                 `json:"id"`
	Vector []float32              `json:"vector,omitempty"`
	Props  map[string]interface{} `json:"props,omitempty"`
	Source string                 `json:"source,omitempty"`
}

// VectorRecord represents a legacy vector-shaped compatibility payload.
type VectorRecord struct {
	ID       string                 `json:"id"`
	Vector   []float32              `json:"vector"`
	Metadata map[string]interface{} `json:"metadata,omitempty"`
}

// SearchQuery represents a vector search query.
type SearchQuery struct {
	Vector          []float32    `json:"vector"`
	TopK            int          `json:"top_k,omitempty"`
	Filter          *Filter      `json:"filter,omitempty"`
	IncludeVectors  bool         `json:"include_vectors,omitempty"`
	IncludeMetadata bool         `json:"include_metadata,omitempty"`
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

// CreateCollection creates a new vector collection.
func (a *Adapter) CreateCollection(ctx context.Context, req *CreateCollectionRequest) (*CollectionInfo, error) {
	url := fmt.Sprintf("%s/api/v1/collections", a.baseURL)

	body, err := json.Marshal(req)
	if err != nil {
		return nil, &AdapterError{
			Code:    ErrCodeInvalidArgument,
			Message: "failed to marshal request",
			Cause:   err,
		}
	}

	var result CollectionInfo
	if err := a.doRequest(ctx, http.MethodPost, url, body, &result); err != nil {
		return nil, err
	}

	return &result, nil
}

// ListCollections returns all collections.
func (a *Adapter) ListCollections(ctx context.Context) ([]*CollectionInfo, error) {
	url := fmt.Sprintf("%s/api/v1/collections", a.baseURL)

	var response struct {
		Collections []*CollectionInfo `json:"collections"`
	}
	if err := a.doRequest(ctx, http.MethodGet, url, nil, &response); err != nil {
		return nil, err
	}

	return response.Collections, nil
}

// GetCollection returns information about a specific collection.
func (a *Adapter) GetCollection(ctx context.Context, name string) (*CollectionInfo, error) {
	url := fmt.Sprintf("%s/api/v1/collections/%s", a.baseURL, name)

	var result CollectionInfo
	if err := a.doRequest(ctx, http.MethodGet, url, nil, &result); err != nil {
		return nil, err
	}

	return &result, nil
}

// DeleteCollection deletes a collection.
func (a *Adapter) DeleteCollection(ctx context.Context, name string) error {
	url := fmt.Sprintf("%s/api/v1/collections/%s", a.baseURL, name)
	return a.doRequest(ctx, http.MethodDelete, url, nil, nil)
}

// Insert inserts records into a collection.
func (a *Adapter) Insert(ctx context.Context, collection string, records []*VectorRecord) error {
	url := fmt.Sprintf("%s/api/v2/collections/%s/records/batch", a.baseURL, collection)

	body, err := json.Marshal(map[string]interface{}{
		"records":         recordsToProximaRecords(records),
		"validate_schema": true,
	})
	if err != nil {
		return &AdapterError{
			Code:    ErrCodeInvalidArgument,
			Message: "failed to marshal request",
			Cause:   err,
		}
	}

	return a.doRequest(ctx, http.MethodPost, url, body, nil)
}

// Upsert inserts or updates records in a collection.
func (a *Adapter) Upsert(ctx context.Context, collection string, records []*VectorRecord) error {
	url := fmt.Sprintf("%s/api/v2/collections/%s/records/batch", a.baseURL, collection)

	body, err := json.Marshal(map[string]interface{}{
		"records":         recordsToProximaRecords(records),
		"validate_schema": true,
		"upsert":          true,
	})
	if err != nil {
		return &AdapterError{
			Code:    ErrCodeInvalidArgument,
			Message: "failed to marshal request",
			Cause:   err,
		}
	}

	return a.doRequest(ctx, http.MethodPost, url, body, nil)
}

func recordsToProximaRecords(records []*VectorRecord) []*ProximaRecord {
	result := make([]*ProximaRecord, len(records))
	for i, record := range records {
		result[i] = &ProximaRecord{
			ID:     record.ID,
			Vector: record.Vector,
			Props:  record.Metadata,
		}
	}
	return result
}

// Search performs a vector similarity search.
func (a *Adapter) Search(ctx context.Context, collection string, query *SearchQuery) (*SearchResponse, error) {
	url := fmt.Sprintf("%s/api/v1/collections/%s/search", a.baseURL, collection)

	body, err := json.Marshal(query)
	if err != nil {
		return nil, &AdapterError{
			Code:    ErrCodeInvalidArgument,
			Message: "failed to marshal request",
			Cause:   err,
		}
	}

	var result SearchResponse
	if err := a.doRequest(ctx, http.MethodPost, url, body, &result); err != nil {
		return nil, err
	}

	return &result, nil
}

// Get retrieves vectors by their IDs.
func (a *Adapter) Get(ctx context.Context, collection string, ids []string) ([]*VectorRecord, error) {
	url := fmt.Sprintf("%s/api/v1/collections/%s/vectors/fetch", a.baseURL, collection)

	body, err := json.Marshal(map[string]interface{}{
		"ids": ids,
	})
	if err != nil {
		return nil, &AdapterError{
			Code:    ErrCodeInvalidArgument,
			Message: "failed to marshal request",
			Cause:   err,
		}
	}

	var response struct {
		Vectors []*VectorRecord `json:"vectors"`
	}
	if err := a.doRequest(ctx, http.MethodPost, url, body, &response); err != nil {
		return nil, err
	}

	return response.Vectors, nil
}

// Delete removes vectors by their IDs.
func (a *Adapter) Delete(ctx context.Context, collection string, ids []string) error {
	url := fmt.Sprintf("%s/api/v1/collections/%s/vectors/delete", a.baseURL, collection)

	body, err := json.Marshal(map[string]interface{}{
		"ids": ids,
	})
	if err != nil {
		return &AdapterError{
			Code:    ErrCodeInvalidArgument,
			Message: "failed to marshal request",
			Cause:   err,
		}
	}

	return a.doRequest(ctx, http.MethodPost, url, body, nil)
}

// Health checks the server health.
func (a *Adapter) Health(ctx context.Context) (*HealthStatus, error) {
	url := fmt.Sprintf("%s/api/v1/health", a.baseURL)

	var result HealthStatus
	if err := a.doRequest(ctx, http.MethodGet, url, nil, &result); err != nil {
		return nil, err
	}

	return &result, nil
}

// Close closes the adapter and releases resources.
func (a *Adapter) Close() error {
	// HTTP client doesn't need explicit closing, but we close idle connections
	if transport, ok := a.client.Transport.(*http.Transport); ok {
		transport.CloseIdleConnections()
	}
	return nil
}

// doRequest performs an HTTP request and handles the response.
func (a *Adapter) doRequest(ctx context.Context, method, url string, body []byte, result interface{}) error {
	var bodyReader io.Reader
	if body != nil {
		bodyReader = bytes.NewReader(body)
	}

	req, err := http.NewRequestWithContext(ctx, method, url, bodyReader)
	if err != nil {
		return &AdapterError{
			Code:    ErrCodeInvalidArgument,
			Message: "failed to create request",
			Cause:   err,
		}
	}

	// Set headers
	for k, v := range a.headers {
		req.Header.Set(k, v)
	}

	// Execute request
	resp, err := a.client.Do(req)
	if err != nil {
		// Check for timeout
		if ctx.Err() == context.DeadlineExceeded {
			return &AdapterError{
				Code:    ErrCodeTimeout,
				Message: "request timed out",
				Cause:   err,
			}
		}
		return &AdapterError{
			Code:    ErrCodeConnection,
			Message: "connection error",
			Cause:   err,
		}
	}
	defer resp.Body.Close()

	// Read response body
	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return &AdapterError{
			Code:    ErrCodeInternal,
			Message: "failed to read response",
			Cause:   err,
		}
	}

	// Handle error responses
	if resp.StatusCode >= 400 {
		return a.parseErrorResponse(resp.StatusCode, respBody)
	}

	// Parse successful response
	if result != nil && len(respBody) > 0 {
		if err := json.Unmarshal(respBody, result); err != nil {
			return &AdapterError{
				Code:    ErrCodeInternal,
				Message: "failed to parse response",
				Cause:   err,
			}
		}
	}

	return nil
}

// parseErrorResponse parses an error response from the server.
func (a *Adapter) parseErrorResponse(statusCode int, body []byte) error {
	var errorResp struct {
		Error   string `json:"error"`
		Message string `json:"message"`
		Code    string `json:"code"`
	}

	if err := json.Unmarshal(body, &errorResp); err != nil {
		// Fallback to raw body if parsing fails
		errorResp.Message = string(body)
	}

	// Map HTTP status code to error code
	var code ErrorCode
	switch statusCode {
	case http.StatusNotFound:
		code = ErrCodeNotFound
	case http.StatusConflict:
		code = ErrCodeAlreadyExists
	case http.StatusBadRequest:
		code = ErrCodeInvalidArgument
	case http.StatusUnprocessableEntity:
		code = ErrCodeDimensionMismatch
	case http.StatusTooManyRequests:
		code = ErrCodeRateLimited
	case http.StatusServiceUnavailable:
		code = ErrCodeUnavailable
	default:
		code = ErrCodeInternal
	}

	// Use server-provided code if available
	if errorResp.Code != "" {
		code = ErrorCode(errorResp.Code)
	}

	message := errorResp.Message
	if message == "" {
		message = errorResp.Error
	}
	if message == "" {
		message = fmt.Sprintf("HTTP %d", statusCode)
	}

	return &AdapterError{
		Code:    code,
		Message: message,
	}
}
