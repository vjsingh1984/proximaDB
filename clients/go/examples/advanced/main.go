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

/*
Advanced Example - ProximaDB Go SDK

This example demonstrates advanced features of the ProximaDB Go SDK including:
- Batch operations with progress tracking
- Parallel search
- Custom middleware (logging, circuit breaker, rate limiting)
- Error handling and retries
- Performance metrics

Run with: go run main.go

Make sure ProximaDB is running at localhost:5678
*/
package main

import (
	"context"
	"fmt"
	"log"
	"math/rand"
	"os"
	"sync/atomic"
	"time"

	"github.com/proximadb/proximadb-go/proximadb"
)

func main() {
	// Create a logger for middleware
	logger := log.New(os.Stdout, "[ProximaDB] ", log.LstdFlags)

	// Create a client with advanced configuration
	client, err := proximadb.NewClient(
		proximadb.WithURL("http://localhost:5678"),
		proximadb.WithTimeout(60*time.Second),
		proximadb.WithMaxRetries(5),
		proximadb.WithRetryDelay(100*time.Millisecond),
		proximadb.WithMaxRetryDelay(5*time.Second),
		proximadb.WithPoolSize(20),
		proximadb.WithMaxIdleConns(50),
		proximadb.WithIdleConnTimeout(120*time.Second),
		proximadb.WithUserAgent("proximadb-advanced-example/1.0"),
		// Add logging middleware
		proximadb.WithMiddleware(proximadb.LoggingMiddleware(logger)),
	)
	if err != nil {
		log.Fatalf("Failed to create client: %v", err)
	}
	defer client.Close()

	ctx := context.Background()

	// Check server health
	fmt.Println("=== Server Health Check ===")
	health, err := client.Health(ctx)
	if err != nil {
		log.Fatalf("Health check failed: %v", err)
	}
	fmt.Printf("Status: %s, Version: %s\n\n", health.Status, health.Version)

	// Collection setup
	collectionName := "advanced_example"
	_ = client.DeleteCollection(ctx, collectionName)

	fmt.Println("=== Creating Collection ===")
	_, err = client.CreateCollection(ctx, &proximadb.CreateCollectionRequest{
		Name:      collectionName,
		Dimension: 256,
		Metric:    proximadb.Cosine,
		Engine:    proximadb.EngineSst,
	})
	if err != nil {
		log.Fatalf("Failed to create collection: %v", err)
	}
	fmt.Println("Collection created successfully")

	// Demonstrate batch insert with progress tracking
	fmt.Println("=== Batch Insert with Progress ===")
	records := generateLargeDataset(5000, 256)

	var insertedCount int64

	batchResult, err := client.BatchInsert(ctx, collectionName, records, &proximadb.BatchOptions{
		BatchSize:   500,
		Concurrency: 4,
		OnProgress: func(processed, total int) {
			atomic.StoreInt64(&insertedCount, int64(processed))
			percent := float64(processed) / float64(total) * 100
			fmt.Printf("\rProgress: %d/%d (%.1f%%)", processed, total, percent)
		},
		ContinueOnError: true,
	})
	if err != nil {
		log.Printf("Batch insert completed with error: %v", err)
	}

	fmt.Printf("\n\nBatch Insert Summary:\n")
	fmt.Printf("  Total Processed: %d\n", batchResult.TotalProcessed)
	fmt.Printf("  Successful: %d\n", batchResult.SuccessCount)
	fmt.Printf("  Failed: %d\n", batchResult.FailedCount)
	fmt.Printf("  Duration: %v\n", batchResult.Duration)
	fmt.Printf("  Throughput: %.0f vectors/sec\n\n",
		float64(batchResult.SuccessCount)/batchResult.Duration.Seconds())

	// Demonstrate batch search (parallel queries)
	fmt.Println("=== Batch Search (Parallel Queries) ===")
	queries := make([]*proximadb.SearchQuery, 20)
	for i := 0; i < 20; i++ {
		queries[i] = &proximadb.SearchQuery{
			Vector:          generateRandomVector(256),
			TopK:            10,
			IncludeMetadata: true,
		}
	}

	searchStart := time.Now()
	searchResults, err := client.BatchSearch(ctx, collectionName, queries, &proximadb.BatchOptions{
		Concurrency: 8,
		OnProgress: func(processed, total int) {
			fmt.Printf("\rSearches completed: %d/%d", processed, total)
		},
	})
	if err != nil {
		log.Printf("Batch search completed with error: %v", err)
	}
	searchDuration := time.Since(searchStart)

	fmt.Printf("\n\nBatch Search Summary:\n")
	fmt.Printf("  Queries Executed: %d\n", len(queries))
	fmt.Printf("  Successful Results: %d\n", countNonNil(searchResults))
	fmt.Printf("  Total Duration: %v\n", searchDuration)
	fmt.Printf("  Avg Query Time: %v\n", searchDuration/time.Duration(len(queries)))
	fmt.Printf("  Queries/sec: %.0f\n\n", float64(len(queries))/searchDuration.Seconds())

	// Demonstrate complex filters
	fmt.Println("=== Complex Filter Queries ===")

	// Filter: category = "A" AND price > 50
	complexFilter := proximadb.And(
		proximadb.Eq("category", "A"),
		proximadb.Gt("price", 50),
	)

	resp, err := client.Search(ctx, collectionName, &proximadb.SearchQuery{
		Vector:          generateRandomVector(256),
		TopK:            10,
		Filter:          &complexFilter,
		IncludeMetadata: true,
	})
	if err != nil {
		log.Printf("Complex filter search failed: %v", err)
	} else {
		fmt.Printf("Complex filter (category=A AND price>50): %d results\n", len(resp.Results))
	}

	// Filter: category IN ("A", "B") OR rating >= 4
	orFilter := proximadb.Or(
		proximadb.In("category", "A", "B"),
		proximadb.Gte("rating", 4),
	)

	resp2, err := client.Search(ctx, collectionName, &proximadb.SearchQuery{
		Vector:          generateRandomVector(256),
		TopK:            10,
		Filter:          &orFilter,
		IncludeMetadata: true,
	})
	if err != nil {
		log.Printf("OR filter search failed: %v", err)
	} else {
		fmt.Printf("OR filter (category IN [A,B] OR rating>=4): %d results\n\n", len(resp2.Results))
	}

	// Demonstrate error handling
	fmt.Println("=== Error Handling Examples ===")

	// Try to get a non-existent collection
	_, err = client.GetCollection(ctx, "nonexistent_collection")
	if err != nil {
		if proximadb.IsNotFound(err) {
			fmt.Println("Correctly caught NotFound error for non-existent collection")
		} else {
			fmt.Printf("Unexpected error type: %v\n", err)
		}
	}

	// Try to search in a non-existent collection
	_, err = client.Search(ctx, "nonexistent_collection", &proximadb.SearchQuery{
		Vector: generateRandomVector(256),
		TopK:   10,
	})
	if err != nil {
		if proximadb.IsNotFound(err) {
			fmt.Println("Correctly caught NotFound error for search")
		} else if proximadb.IsRetryable(err) {
			fmt.Println("Error is retryable")
		} else {
			fmt.Printf("Search error: %v\n", err)
		}
	}
	fmt.Println()

	// Demonstrate upsert
	fmt.Println("=== Upsert Operation ===")
	upsertRecords := []*proximadb.VectorRecord{
		{
			ID:       "vec_0",
			Vector:   generateRandomVector(256),
			Metadata: map[string]interface{}{"updated": true, "version": 2},
		},
		{
			ID:       "new_vec_1",
			Vector:   generateRandomVector(256),
			Metadata: map[string]interface{}{"new": true},
		},
	}

	err = client.Upsert(ctx, collectionName, upsertRecords)
	if err != nil {
		log.Printf("Upsert failed: %v", err)
	} else {
		fmt.Println("Upsert completed: updated vec_0, inserted new_vec_1")
	}

	// Verify upsert
	vectors, err := client.Get(ctx, collectionName, []string{"vec_0", "new_vec_1"})
	if err != nil {
		log.Printf("Get after upsert failed: %v", err)
	} else {
		fmt.Printf("Retrieved %d vectors after upsert:\n", len(vectors))
		for _, v := range vectors {
			fmt.Printf("  - %s: %v\n", v.ID, v.Metadata)
		}
	}
	fmt.Println()

	// Print final client metrics
	fmt.Println("=== Client Metrics Summary ===")
	metrics := client.Metrics()
	fmt.Printf("Total Requests: %d\n", metrics.RequestCount)
	fmt.Printf("Successful: %d (%.1f%%)\n", metrics.SuccessCount, metrics.SuccessRate())
	fmt.Printf("Failed: %d\n", metrics.ErrorCount)
	fmt.Printf("Retries: %d\n", metrics.RetryCount)
	fmt.Printf("Average Latency: %v\n", metrics.AverageLatency())
	fmt.Printf("Last Request: %v\n", metrics.LastRequestTime.Format(time.RFC3339))
	fmt.Println()

	// Cleanup
	fmt.Println("=== Cleanup ===")
	err = client.DeleteCollection(ctx, collectionName)
	if err != nil {
		log.Printf("Cleanup failed: %v", err)
	} else {
		fmt.Println("Collection deleted successfully")
	}
}

// generateLargeDataset generates a large dataset for batch testing.
func generateLargeDataset(count, dimension int) []*proximadb.VectorRecord {
	categories := []string{"A", "B", "C", "D", "E"}
	records := make([]*proximadb.VectorRecord, count)

	for i := 0; i < count; i++ {
		records[i] = &proximadb.VectorRecord{
			ID:     fmt.Sprintf("vec_%d", i),
			Vector: generateRandomVector(dimension),
			Metadata: map[string]interface{}{
				"category": categories[i%len(categories)],
				"price":    rand.Float64() * 100,
				"rating":   rand.Intn(5) + 1,
				"index":    i,
			},
		}
	}

	return records
}

// generateRandomVector generates a random normalized vector.
func generateRandomVector(dimension int) []float32 {
	vector := make([]float32, dimension)
	var sum float32

	for i := 0; i < dimension; i++ {
		vector[i] = rand.Float32()*2 - 1
		sum += vector[i] * vector[i]
	}

	// Normalize
	norm := float32(1.0) / float32(sum)
	for i := 0; i < dimension; i++ {
		vector[i] *= norm
	}

	return vector
}

// countNonNil counts non-nil search responses.
func countNonNil(results []*proximadb.SearchResponse) int {
	count := 0
	for _, r := range results {
		if r != nil {
			count++
		}
	}
	return count
}
