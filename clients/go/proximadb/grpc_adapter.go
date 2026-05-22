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

	igrpc "github.com/proximadb/proximadb-go/proximadb/internal/grpc"
)

// grpcAdapter wraps the internal gRPC adapter to implement the Adapter interface.
type grpcAdapter struct {
	inner *igrpc.Adapter
}

// newGRPCAdapter creates a new gRPC adapter from the client configuration.
func newGRPCAdapter(config *Config) (Adapter, error) {
	var tlsConfig *igrpc.TLSConfig
	if config.TLS != nil {
		tlsConfig = &igrpc.TLSConfig{
			CertFile:   config.TLS.CertFile,
			KeyFile:    config.TLS.KeyFile,
			CAFile:     config.TLS.CAFile,
			SkipVerify: config.TLS.SkipVerify,
		}
	}

	innerConfig := &igrpc.Config{
		Target:   config.URL,
		APIKey:   config.APIKey,
		Timeout:  config.Timeout,
		PoolSize: config.PoolSize,
		TLS:      tlsConfig,
	}

	inner, err := igrpc.NewAdapter(innerConfig)
	if err != nil {
		return nil, convertGRPCError(err)
	}

	return &grpcAdapter{inner: inner}, nil
}

// CreateCollection creates a new vector collection.
func (a *grpcAdapter) CreateCollection(ctx context.Context, req *CreateCollectionRequest) (*CollectionInfo, error) {
	innerReq := &igrpc.CreateCollectionRequest{
		Name:        req.Name,
		Dimension:   req.Dimension,
		Metric:      string(req.Metric),
		Engine:      string(req.Engine),
		Description: req.Description,
	}

	result, err := a.inner.CreateCollection(ctx, innerReq)
	if err != nil {
		return nil, convertGRPCError(err)
	}

	return convertGRPCCollectionInfo(result), nil
}

// ListCollections returns all collections.
func (a *grpcAdapter) ListCollections(ctx context.Context) ([]*CollectionInfo, error) {
	results, err := a.inner.ListCollections(ctx)
	if err != nil {
		return nil, convertGRPCError(err)
	}

	collections := make([]*CollectionInfo, len(results))
	for i, r := range results {
		collections[i] = convertGRPCCollectionInfo(r)
	}
	return collections, nil
}

// GetCollection returns information about a specific collection.
func (a *grpcAdapter) GetCollection(ctx context.Context, name string) (*CollectionInfo, error) {
	result, err := a.inner.GetCollection(ctx, name)
	if err != nil {
		return nil, convertGRPCError(err)
	}
	return convertGRPCCollectionInfo(result), nil
}

// DeleteCollection deletes a collection.
func (a *grpcAdapter) DeleteCollection(ctx context.Context, name string) error {
	if err := a.inner.DeleteCollection(ctx, name); err != nil {
		return convertGRPCError(err)
	}
	return nil
}

// InsertRecords inserts canonical records into a collection.
func (a *grpcAdapter) InsertRecords(ctx context.Context, collection string, records []*ProximaRecord) error {
	innerRecords := convertProximaToGRPCRecords(records)
	if err := a.inner.Insert(ctx, collection, innerRecords); err != nil {
		return convertGRPCError(err)
	}
	return nil
}

// UpsertRecords inserts or updates canonical records in a collection.
func (a *grpcAdapter) UpsertRecords(ctx context.Context, collection string, records []*ProximaRecord) error {
	innerRecords := convertProximaToGRPCRecords(records)
	if err := a.inner.Upsert(ctx, collection, innerRecords); err != nil {
		return convertGRPCError(err)
	}
	return nil
}

// Insert inserts vectors into a collection.
//
// Deprecated: use InsertRecords with ProximaRecord.
func (a *grpcAdapter) Insert(ctx context.Context, collection string, records []*VectorRecord) error {
	innerRecords := convertToGRPCRecords(records)
	if err := a.inner.Insert(ctx, collection, innerRecords); err != nil {
		return convertGRPCError(err)
	}
	return nil
}

// Upsert inserts or updates vectors in a collection.
//
// Deprecated: use UpsertRecords with ProximaRecord.
func (a *grpcAdapter) Upsert(ctx context.Context, collection string, records []*VectorRecord) error {
	innerRecords := convertToGRPCRecords(records)
	if err := a.inner.Upsert(ctx, collection, innerRecords); err != nil {
		return convertGRPCError(err)
	}
	return nil
}

// Search performs a vector similarity search.
func (a *grpcAdapter) Search(ctx context.Context, collection string, query *SearchQuery) (*SearchResponse, error) {
	innerQuery := &igrpc.SearchQuery{
		Vector:          query.Vector,
		TopK:            query.TopK,
		IncludeVectors:  query.IncludeVectors,
		IncludeMetadata: query.IncludeMetadata,
	}

	if query.Filter != nil {
		innerQuery.Filter = convertToGRPCFilter(query.Filter)
	}

	result, err := a.inner.Search(ctx, collection, innerQuery)
	if err != nil {
		return nil, convertGRPCError(err)
	}

	return convertGRPCSearchResponse(result), nil
}

// Get retrieves vectors by their IDs.
func (a *grpcAdapter) Get(ctx context.Context, collection string, ids []string) ([]*VectorRecord, error) {
	results, err := a.inner.Get(ctx, collection, ids)
	if err != nil {
		return nil, convertGRPCError(err)
	}

	return convertFromGRPCRecords(results), nil
}

// Delete removes vectors by their IDs.
func (a *grpcAdapter) Delete(ctx context.Context, collection string, ids []string) error {
	if err := a.inner.Delete(ctx, collection, ids); err != nil {
		return convertGRPCError(err)
	}
	return nil
}

// Health checks the server health.
func (a *grpcAdapter) Health(ctx context.Context) (*HealthStatus, error) {
	result, err := a.inner.Health(ctx)
	if err != nil {
		return nil, convertGRPCError(err)
	}

	return &HealthStatus{
		Status:  result.Status,
		Version: result.Version,
		Uptime:  result.Uptime,
	}, nil
}

// Close closes the adapter and releases resources.
func (a *grpcAdapter) Close() error {
	return a.inner.Close()
}

// Conversion helpers

func convertGRPCCollectionInfo(r *igrpc.CollectionInfo) *CollectionInfo {
	if r == nil {
		return nil
	}
	return &CollectionInfo{
		Name:        r.Name,
		Dimension:   r.Dimension,
		Metric:      DistanceMetric(r.Metric),
		Engine:      StorageEngine(r.Engine),
		VectorCount: r.VectorCount,
		CreatedAt:   r.CreatedAt,
	}
}

func convertToGRPCRecords(records []*VectorRecord) []*igrpc.VectorRecord {
	result := make([]*igrpc.VectorRecord, len(records))
	for i, r := range records {
		result[i] = &igrpc.VectorRecord{
			ID:       r.ID,
			Vector:   r.Vector,
			Metadata: r.Metadata,
		}
	}
	return result
}

func convertProximaToGRPCRecords(records []*ProximaRecord) []*igrpc.VectorRecord {
	result := make([]*igrpc.VectorRecord, len(records))
	for i, r := range records {
		result[i] = &igrpc.VectorRecord{
			ID:       r.ID,
			Vector:   r.Vector,
			Metadata: r.Props,
		}
	}
	return result
}

func convertFromGRPCRecords(records []*igrpc.VectorRecord) []*VectorRecord {
	result := make([]*VectorRecord, len(records))
	for i, r := range records {
		result[i] = &VectorRecord{
			ID:       r.ID,
			Vector:   r.Vector,
			Metadata: r.Metadata,
		}
	}
	return result
}

func convertToGRPCFilter(f *Filter) *igrpc.Filter {
	if f == nil {
		return nil
	}

	result := &igrpc.Filter{
		Field:    f.Field,
		Operator: string(f.Operator),
		Value:    f.Value,
	}

	if len(f.And) > 0 {
		result.And = make([]igrpc.Filter, len(f.And))
		for i, af := range f.And {
			result.And[i] = *convertToGRPCFilter(&af)
		}
	}

	if len(f.Or) > 0 {
		result.Or = make([]igrpc.Filter, len(f.Or))
		for i, of := range f.Or {
			result.Or[i] = *convertToGRPCFilter(&of)
		}
	}

	return result
}

func convertGRPCSearchResponse(r *igrpc.SearchResponse) *SearchResponse {
	if r == nil {
		return nil
	}
	results := make([]SearchResult, len(r.Results))
	for i, sr := range r.Results {
		results[i] = SearchResult{
			ID:       sr.ID,
			Score:    sr.Score,
			Vector:   sr.Vector,
			Metadata: sr.Metadata,
		}
	}
	return &SearchResponse{
		Results:    results,
		TookMs:     r.TookMs,
		TotalCount: r.TotalCount,
	}
}

func convertGRPCError(err error) error {
	if err == nil {
		return nil
	}

	if ae, ok := err.(*igrpc.AdapterError); ok {
		return &ProximaDBError{
			Code:    ErrorCode(ae.Code),
			Message: ae.Message,
			Cause:   ae.Cause,
		}
	}

	return WrapError(ErrCodeInternal, "gRPC adapter error", err)
}
