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
Basic Example - ProximaDB Go SDK

This example demonstrates the basic usage of the ProximaDB Go SDK including:
- Creating a client
- Managing collections
- Inserting vectors
- Performing searches
- Error handling

Run with: go run main.go

Make sure ProximaDB is running at localhost:5678
*/
package main

import (
	"context"
	"fmt"
	"log"
	"math/rand"
	"time"

	"github.com/proximadb/proximadb-go/proximadb"
)

func main() {
	// Create a client with configuration options
	client, err := proximadb.NewClient(
		proximadb.WithURL("http://localhost:5678"),
		proximadb.WithTimeout(30*time.Second),
		proximadb.WithMaxRetries(3),
	)
	if err != nil {
		log.Fatalf("Failed to create client: %v", err)
	}
	defer client.Close()

	ctx := context.Background()

	// Check server health
	fmt.Println("Checking server health...")
	health, err := client.Health(ctx)
	if err != nil {
		log.Fatalf("Health check failed: %v", err)
	}
	fmt.Printf("Server is %s (version: %s, uptime: %.2fs)\n\n",
		health.Status, health.Version, health.Uptime)

	// Collection name
	collectionName := "example_vectors"

	// Delete collection if it exists (for clean demo)
	_ = client.DeleteCollection(ctx, collectionName)

	// Create a new collection
	fmt.Println("Creating collection...")
	collection, err := client.CreateCollection(ctx, &proximadb.CreateCollectionRequest{
		Name:        collectionName,
		Dimension:   128,
		Metric:      proximadb.Cosine,
		Engine:      proximadb.EngineSst,
		Description: "Example vector collection",
	})
	if err != nil {
		log.Fatalf("Failed to create collection: %v", err)
	}
	fmt.Printf("Created collection: %s (dimension: %d)\n\n", collection.Name, collection.Dimension)

	// Generate and insert sample records with embeddings
	fmt.Println("Inserting records...")
	records := generateSampleRecords(100, 128)
	err = client.InsertRecords(ctx, collectionName, records)
	if err != nil {
		log.Fatalf("Failed to insert records: %v", err)
	}
	fmt.Printf("Inserted %d records\n\n", len(records))

	// Perform a vector search
	fmt.Println("Performing vector search...")
	queryVector := generateRandomVector(128)
	searchResp, err := client.Search(ctx, collectionName, &proximadb.SearchQuery{
		Vector:          queryVector,
		TopK:            5,
		IncludeMetadata: true,
	})
	if err != nil {
		log.Fatalf("Search failed: %v", err)
	}

	fmt.Printf("Search completed in %.2fms, found %d results:\n", searchResp.TookMs, len(searchResp.Results))
	for i, result := range searchResp.Results {
		fmt.Printf("  %d. ID: %s, Score: %.4f, Category: %v\n",
			i+1, result.ID, result.Score, result.Metadata["category"])
	}
	fmt.Println()

	// Search with filter
	fmt.Println("Searching with filter (category = 'A')...")
	filter := proximadb.Eq("category", "A")
	filteredResp, err := client.Search(ctx, collectionName, &proximadb.SearchQuery{
		Vector:          queryVector,
		TopK:            5,
		Filter:          &filter,
		IncludeMetadata: true,
	})
	if err != nil {
		log.Fatalf("Filtered search failed: %v", err)
	}

	fmt.Printf("Filtered search found %d results:\n", len(filteredResp.Results))
	for i, result := range filteredResp.Results {
		fmt.Printf("  %d. ID: %s, Score: %.4f, Category: %v\n",
			i+1, result.ID, result.Score, result.Metadata["category"])
	}
	fmt.Println()

	// Get vectors by ID
	fmt.Println("Fetching vectors by ID...")
	ids := []string{"vec_0", "vec_1", "vec_2"}
	vectors, err := client.Get(ctx, collectionName, ids)
	if err != nil {
		log.Fatalf("Get failed: %v", err)
	}
	fmt.Printf("Retrieved %d vectors:\n", len(vectors))
	for _, v := range vectors {
		fmt.Printf("  ID: %s, Vector length: %d\n", v.ID, len(v.Vector))
	}
	fmt.Println()

	// List all collections
	fmt.Println("Listing collections...")
	collections, err := client.ListCollections(ctx)
	if err != nil {
		log.Fatalf("List collections failed: %v", err)
	}
	fmt.Printf("Found %d collection(s):\n", len(collections))
	for _, c := range collections {
		fmt.Printf("  - %s (dimension: %d, vectors: %d)\n", c.Name, c.Dimension, c.VectorCount)
	}
	fmt.Println()

	// Delete some vectors
	fmt.Println("Deleting vectors...")
	err = client.Delete(ctx, collectionName, []string{"vec_0", "vec_1"})
	if err != nil {
		log.Fatalf("Delete failed: %v", err)
	}
	fmt.Println("Deleted 2 vectors")

	// Get collection info
	info, err := client.GetCollection(ctx, collectionName)
	if err != nil {
		log.Fatalf("Get collection failed: %v", err)
	}
	fmt.Printf("Collection %s now has %d vectors\n\n", info.Name, info.VectorCount)

	// Cleanup: delete the collection
	fmt.Println("Cleaning up...")
	err = client.DeleteCollection(ctx, collectionName)
	if err != nil {
		log.Fatalf("Delete collection failed: %v", err)
	}
	fmt.Println("Collection deleted successfully")

	// Print client metrics
	metrics := client.Metrics()
	fmt.Printf("\nClient Metrics:\n")
	fmt.Printf("  Total Requests: %d\n", metrics.RequestCount)
	fmt.Printf("  Successful: %d\n", metrics.SuccessCount)
	fmt.Printf("  Failed: %d\n", metrics.ErrorCount)
	fmt.Printf("  Success Rate: %.2f%%\n", metrics.SuccessRate())
	fmt.Printf("  Average Latency: %v\n", metrics.AverageLatency())
}

// generateSampleRecords generates sample records with embeddings.
func generateSampleRecords(count, dimension int) []*proximadb.ProximaRecord {
	categories := []string{"A", "B", "C", "D"}
	records := make([]*proximadb.ProximaRecord, count)

	for i := 0; i < count; i++ {
		records[i] = &proximadb.ProximaRecord{
			ID:     fmt.Sprintf("vec_%d", i),
			Vector: generateRandomVector(dimension),
			Props: map[string]interface{}{
				"category": categories[i%len(categories)],
				"index":    i,
				"created":  time.Now().Format(time.RFC3339),
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
		vector[i] = rand.Float32()*2 - 1 // Values between -1 and 1
		sum += vector[i] * vector[i]
	}

	// Normalize
	norm := float32(1.0) / float32(sum)
	for i := 0; i < dimension; i++ {
		vector[i] *= norm
	}

	return vector
}
