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
Streaming Example - ProximaDB Go SDK

This example demonstrates streaming operations with the ProximaDB Go SDK:
- Stream inserter for continuous vector ingestion
- Stream searcher for continuous query processing
- Handling errors in streaming operations
- Real-time data pipelines

Run with: go run main.go

Make sure ProximaDB is running at localhost:5678
*/
package main

import (
	"context"
	"fmt"
	"log"
	"math/rand"
	"sync"
	"time"

	"github.com/proximadb/proximadb-go/proximadb"
)

func main() {
	// Create client
	client, err := proximadb.NewClient(
		proximadb.WithURL("http://localhost:5678"),
		proximadb.WithTimeout(30*time.Second),
	)
	if err != nil {
		log.Fatalf("Failed to create client: %v", err)
	}
	defer client.Close()

	ctx := context.Background()

	// Setup collection
	collectionName := "streaming_example"
	_ = client.DeleteCollection(ctx, collectionName)

	_, err = client.CreateCollection(ctx, &proximadb.CreateCollectionRequest{
		Name:      collectionName,
		Dimension: 128,
		Metric:    proximadb.Cosine,
		Engine:    proximadb.EngineSst,
	})
	if err != nil {
		log.Fatalf("Failed to create collection: %v", err)
	}
	fmt.Println("Collection created successfully\n")

	// Example 1: Streaming Insert
	fmt.Println("=== Example 1: Streaming Insert ===")
	runStreamingInsertExample(ctx, client, collectionName)

	// Example 2: Streaming Search
	fmt.Println("\n=== Example 2: Streaming Search ===")
	runStreamingSearchExample(ctx, client, collectionName)

	// Example 3: Real-time Pipeline
	fmt.Println("\n=== Example 3: Real-time Pipeline ===")
	runRealTimePipelineExample(ctx, client, collectionName)

	// Cleanup
	fmt.Println("\n=== Cleanup ===")
	_ = client.DeleteCollection(ctx, collectionName)
	fmt.Println("Collection deleted")
}

// runStreamingInsertExample demonstrates continuous vector insertion.
func runStreamingInsertExample(ctx context.Context, client proximadb.Client, collection string) {
	// Create a stream inserter
	inserter, err := client.StreamInsert(ctx, collection, &proximadb.StreamOptions{
		BufferSize:    50,
		FlushInterval: 100 * time.Millisecond,
		MaxPending:    500,
	})
	if err != nil {
		log.Fatalf("Failed to create stream inserter: %v", err)
	}

	// Start error handler goroutine
	go func() {
		for err := range inserter.Errors() {
			log.Printf("Insert error: %v", err)
		}
	}()

	// Simulate streaming data source
	fmt.Println("Streaming 1000 vectors...")
	startTime := time.Now()

	for i := 0; i < 1000; i++ {
		record := &proximadb.VectorRecord{
			ID:     fmt.Sprintf("stream_vec_%d", i),
			Vector: generateRandomVector(128),
			Metadata: map[string]interface{}{
				"timestamp": time.Now().UnixNano(),
				"batch":     i / 100,
			},
		}

		if err := inserter.Send(record); err != nil {
			log.Printf("Failed to send record: %v", err)
			break
		}

		// Simulate varying ingestion rate
		if i%200 == 0 {
			time.Sleep(10 * time.Millisecond)
		}
	}

	// Close and get results
	result, err := inserter.Close()
	if err != nil {
		log.Printf("Close error: %v", err)
	}

	duration := time.Since(startTime)
	fmt.Printf("Streaming insert completed:\n")
	fmt.Printf("  Processed: %d vectors\n", result.TotalProcessed)
	fmt.Printf("  Successful: %d\n", result.SuccessCount)
	fmt.Printf("  Failed: %d\n", result.FailedCount)
	fmt.Printf("  Duration: %v\n", duration)
	fmt.Printf("  Throughput: %.0f vectors/sec\n", float64(result.TotalProcessed)/duration.Seconds())
}

// runStreamingSearchExample demonstrates continuous search processing.
func runStreamingSearchExample(ctx context.Context, client proximadb.Client, collection string) {
	// Create a stream searcher
	searcher, err := client.StreamSearch(ctx, collection, &proximadb.StreamOptions{
		BufferSize:    20,
		FlushInterval: 50 * time.Millisecond,
	})
	if err != nil {
		log.Fatalf("Failed to create stream searcher: %v", err)
	}

	// Track results
	var resultCount int
	var totalLatency time.Duration
	var mu sync.Mutex

	// Start result consumer
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		for resp := range searcher.Results() {
			mu.Lock()
			resultCount++
			totalLatency += time.Duration(resp.TookMs * float64(time.Millisecond))
			mu.Unlock()
		}
	}()

	// Start error handler
	go func() {
		for err := range searcher.Errors() {
			log.Printf("Search error: %v", err)
		}
	}()

	// Send search queries
	fmt.Println("Streaming 100 search queries...")
	startTime := time.Now()

	for i := 0; i < 100; i++ {
		query := &proximadb.SearchQuery{
			Vector:          generateRandomVector(128),
			TopK:            10,
			IncludeMetadata: true,
		}

		if err := searcher.Send(query); err != nil {
			log.Printf("Failed to send query: %v", err)
			break
		}
	}

	// Wait a bit for processing, then close
	time.Sleep(500 * time.Millisecond)
	if err := searcher.Close(); err != nil {
		log.Printf("Close error: %v", err)
	}
	wg.Wait()

	duration := time.Since(startTime)
	mu.Lock()
	avgLatency := totalLatency / time.Duration(max(resultCount, 1))
	mu.Unlock()

	fmt.Printf("Streaming search completed:\n")
	fmt.Printf("  Queries sent: 100\n")
	fmt.Printf("  Results received: %d\n", resultCount)
	fmt.Printf("  Duration: %v\n", duration)
	fmt.Printf("  Avg server latency: %v\n", avgLatency)
	fmt.Printf("  Queries/sec: %.0f\n", float64(100)/duration.Seconds())
}

// runRealTimePipelineExample demonstrates a real-time insert -> search pipeline.
func runRealTimePipelineExample(ctx context.Context, client proximadb.Client, collection string) {
	// Create channels for pipeline
	vectorChan := make(chan *proximadb.VectorRecord, 100)
	queryChan := make(chan *proximadb.SearchQuery, 50)
	resultChan := make(chan *proximadb.SearchResponse, 50)
	done := make(chan struct{})

	// Create stream inserter
	inserter, err := client.StreamInsert(ctx, collection, &proximadb.StreamOptions{
		BufferSize:    20,
		FlushInterval: 50 * time.Millisecond,
	})
	if err != nil {
		log.Fatalf("Failed to create inserter: %v", err)
	}

	// Create stream searcher
	searcher, err := client.StreamSearch(ctx, collection, &proximadb.StreamOptions{
		BufferSize:    10,
		FlushInterval: 50 * time.Millisecond,
	})
	if err != nil {
		log.Fatalf("Failed to create searcher: %v", err)
	}

	var wg sync.WaitGroup

	// Producer: Generate vectors
	wg.Add(1)
	go func() {
		defer wg.Done()
		defer close(vectorChan)

		for i := 0; i < 500; i++ {
			select {
			case <-done:
				return
			case vectorChan <- &proximadb.VectorRecord{
				ID:     fmt.Sprintf("pipeline_vec_%d", i),
				Vector: generateRandomVector(128),
				Metadata: map[string]interface{}{
					"pipeline_id": i,
					"created_at":  time.Now().UnixNano(),
				},
			}:
			}
			time.Sleep(2 * time.Millisecond)
		}
	}()

	// Inserter consumer
	wg.Add(1)
	go func() {
		defer wg.Done()
		insertCount := 0
		for record := range vectorChan {
			if err := inserter.Send(record); err != nil {
				log.Printf("Insert send error: %v", err)
				continue
			}
			insertCount++

			// Trigger a search every 50 inserts
			if insertCount%50 == 0 {
				select {
				case queryChan <- &proximadb.SearchQuery{
					Vector:          record.Vector, // Search for similar to last inserted
					TopK:            5,
					IncludeMetadata: true,
				}:
				default:
				}
			}
		}
		close(queryChan)
	}()

	// Query sender
	wg.Add(1)
	go func() {
		defer wg.Done()
		for query := range queryChan {
			if err := searcher.Send(query); err != nil {
				log.Printf("Search send error: %v", err)
			}
		}
	}()

	// Result collector
	wg.Add(1)
	go func() {
		defer wg.Done()
		for resp := range searcher.Results() {
			select {
			case resultChan <- resp:
			default:
			}
		}
		close(resultChan)
	}()

	// Result processor
	wg.Add(1)
	go func() {
		defer wg.Done()
		searchCount := 0
		for result := range resultChan {
			searchCount++
			fmt.Printf("\rProcessed search %d: found %d results in %.2fms",
				searchCount, len(result.Results), result.TookMs)
		}
		fmt.Println()
	}()

	// Wait for completion
	startTime := time.Now()
	wg.Wait()

	// Close streamers
	insertResult, _ := inserter.Close()
	_ = searcher.Close()

	duration := time.Since(startTime)
	fmt.Printf("\nPipeline completed:\n")
	fmt.Printf("  Vectors inserted: %d\n", insertResult.TotalProcessed)
	fmt.Printf("  Duration: %v\n", duration)
	fmt.Printf("  Pipeline throughput: %.0f items/sec\n", float64(insertResult.TotalProcessed)/duration.Seconds())
}

// generateRandomVector generates a random normalized vector.
func generateRandomVector(dimension int) []float32 {
	vector := make([]float32, dimension)
	var sum float32

	for i := 0; i < dimension; i++ {
		vector[i] = rand.Float32()*2 - 1
		sum += vector[i] * vector[i]
	}

	norm := float32(1.0) / float32(sum)
	for i := 0; i < dimension; i++ {
		vector[i] *= norm
	}

	return vector
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}
