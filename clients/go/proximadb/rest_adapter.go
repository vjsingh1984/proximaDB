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

	"github.com/proximadb/proximadb-go/proximadb/internal/rest"
)

// restAdapter wraps the internal REST adapter to implement the Adapter interface.
type restAdapter struct {
	inner *rest.Adapter
}

// newRESTAdapter creates a new REST adapter from the client configuration.
func newRESTAdapter(config *Config) (Adapter, error) {
	var tlsConfig *rest.TLSConfig
	if config.TLS != nil {
		tlsConfig = &rest.TLSConfig{
			CertFile:   config.TLS.CertFile,
			KeyFile:    config.TLS.KeyFile,
			CAFile:     config.TLS.CAFile,
			SkipVerify: config.TLS.SkipVerify,
		}
	}

	innerConfig := &rest.Config{
		BaseURL: config.URL,
		APIKey:  config.APIKey,
		Timeout: config.Timeout,
		TLS:     tlsConfig,
	}

	inner, err := rest.NewAdapter(innerConfig)
	if err != nil {
		return nil, convertRESTError(err)
	}

	return &restAdapter{inner: inner}, nil
}

// CreateCollection creates a new vector collection.
func (a *restAdapter) CreateCollection(ctx context.Context, req *CreateCollectionRequest) (*CollectionInfo, error) {
	innerReq := &rest.CreateCollectionRequest{
		Name:        req.Name,
		Dimension:   req.Dimension,
		Metric:      string(req.Metric),
		Engine:      string(req.Engine),
		Description: req.Description,
	}

	result, err := a.inner.CreateCollection(ctx, innerReq)
	if err != nil {
		return nil, convertRESTError(err)
	}

	return convertCollectionInfo(result), nil
}

// ListCollections returns all collections.
func (a *restAdapter) ListCollections(ctx context.Context) ([]*CollectionInfo, error) {
	results, err := a.inner.ListCollections(ctx)
	if err != nil {
		return nil, convertRESTError(err)
	}

	collections := make([]*CollectionInfo, len(results))
	for i, r := range results {
		collections[i] = convertCollectionInfo(r)
	}
	return collections, nil
}

// GetCollection returns information about a specific collection.
func (a *restAdapter) GetCollection(ctx context.Context, name string) (*CollectionInfo, error) {
	result, err := a.inner.GetCollection(ctx, name)
	if err != nil {
		return nil, convertRESTError(err)
	}
	return convertCollectionInfo(result), nil
}

// DeleteCollection deletes a collection.
func (a *restAdapter) DeleteCollection(ctx context.Context, name string) error {
	if err := a.inner.DeleteCollection(ctx, name); err != nil {
		return convertRESTError(err)
	}
	return nil
}

// InsertRecords inserts canonical records into a collection.
func (a *restAdapter) InsertRecords(ctx context.Context, collection string, records []*ProximaRecord) error {
	innerRecords := convertToRESTProximaRecords(records)
	if err := a.inner.InsertRecords(ctx, collection, innerRecords); err != nil {
		return convertRESTError(err)
	}
	return nil
}

// UpsertRecords inserts or updates canonical records in a collection.
func (a *restAdapter) UpsertRecords(ctx context.Context, collection string, records []*ProximaRecord) error {
	innerRecords := convertToRESTProximaRecords(records)
	if err := a.inner.UpsertRecords(ctx, collection, innerRecords); err != nil {
		return convertRESTError(err)
	}
	return nil
}

// Insert inserts vectors into a collection.
//
// Deprecated: use InsertRecords with ProximaRecord.
func (a *restAdapter) Insert(ctx context.Context, collection string, records []*VectorRecord) error {
	innerRecords := convertToRESTRecords(records)
	if err := a.inner.Insert(ctx, collection, innerRecords); err != nil {
		return convertRESTError(err)
	}
	return nil
}

// Upsert inserts or updates vectors in a collection.
//
// Deprecated: use UpsertRecords with ProximaRecord.
func (a *restAdapter) Upsert(ctx context.Context, collection string, records []*VectorRecord) error {
	innerRecords := convertToRESTRecords(records)
	if err := a.inner.Upsert(ctx, collection, innerRecords); err != nil {
		return convertRESTError(err)
	}
	return nil
}

// Search performs a vector similarity search.
func (a *restAdapter) Search(ctx context.Context, collection string, query *SearchQuery) (*SearchResponse, error) {
	innerQuery := &rest.SearchQuery{
		Vector:          query.Vector,
		TopK:            query.TopK,
		IncludeVectors:  query.IncludeVectors,
		IncludeMetadata: query.IncludeMetadata,
	}

	if query.Filter != nil {
		innerQuery.Filter = convertToRESTFilter(query.Filter)
	}

	result, err := a.inner.Search(ctx, collection, innerQuery)
	if err != nil {
		return nil, convertRESTError(err)
	}

	return convertSearchResponse(result), nil
}

// Get retrieves vectors by their IDs.
func (a *restAdapter) Get(ctx context.Context, collection string, ids []string) ([]*VectorRecord, error) {
	results, err := a.inner.Get(ctx, collection, ids)
	if err != nil {
		return nil, convertRESTError(err)
	}

	return convertFromRESTRecords(results), nil
}

// Delete removes vectors by their IDs.
func (a *restAdapter) Delete(ctx context.Context, collection string, ids []string) error {
	if err := a.inner.Delete(ctx, collection, ids); err != nil {
		return convertRESTError(err)
	}
	return nil
}

// Health checks the server health.
func (a *restAdapter) Health(ctx context.Context) (*HealthStatus, error) {
	result, err := a.inner.Health(ctx)
	if err != nil {
		return nil, convertRESTError(err)
	}

	return &HealthStatus{
		Status:  result.Status,
		Version: result.Version,
		Uptime:  result.Uptime,
	}, nil
}

// Close closes the adapter and releases resources.
func (a *restAdapter) Close() error {
	return a.inner.Close()
}

// Conversion helpers

func convertCollectionInfo(r *rest.CollectionInfo) *CollectionInfo {
	return &CollectionInfo{
		Name:        r.Name,
		Dimension:   r.Dimension,
		Metric:      DistanceMetric(r.Metric),
		Engine:      StorageEngine(r.Engine),
		VectorCount: r.VectorCount,
		CreatedAt:   r.CreatedAt,
	}
}

func convertToRESTRecords(records []*VectorRecord) []*rest.VectorRecord {
	result := make([]*rest.VectorRecord, len(records))
	for i, r := range records {
		result[i] = &rest.VectorRecord{
			ID:       r.ID,
			Vector:   r.Vector,
			Metadata: r.Metadata,
		}
	}
	return result
}

func convertToRESTProximaRecords(records []*ProximaRecord) []*rest.ProximaRecord {
	result := make([]*rest.ProximaRecord, len(records))
	for i, r := range records {
		result[i] = &rest.ProximaRecord{
			ID:     r.ID,
			Vector: r.Vector,
			Props:  r.Props,
			Source: r.Source,
		}
	}
	return result
}

func convertFromRESTRecords(records []*rest.VectorRecord) []*VectorRecord {
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

func convertToRESTFilter(f *Filter) *rest.Filter {
	if f == nil {
		return nil
	}

	result := &rest.Filter{
		Field:    f.Field,
		Operator: string(f.Operator),
		Value:    f.Value,
	}

	if len(f.And) > 0 {
		result.And = make([]rest.Filter, len(f.And))
		for i, af := range f.And {
			result.And[i] = *convertToRESTFilter(&af)
		}
	}

	if len(f.Or) > 0 {
		result.Or = make([]rest.Filter, len(f.Or))
		for i, of := range f.Or {
			result.Or[i] = *convertToRESTFilter(&of)
		}
	}

	return result
}

func convertSearchResponse(r *rest.SearchResponse) *SearchResponse {
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

func convertRESTError(err error) error {
	if err == nil {
		return nil
	}

	if ae, ok := err.(*rest.AdapterError); ok {
		return &ProximaDBError{
			Code:    ErrorCode(ae.Code),
			Message: ae.Message,
			Cause:   ae.Cause,
		}
	}

	return WrapError(ErrCodeInternal, "REST adapter error", err)
}
