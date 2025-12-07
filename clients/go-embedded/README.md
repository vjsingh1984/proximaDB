# ProximaDB Go Embedded

Go bindings for ProximaDB embedded mode with zero network overhead.

## Installation

First, build the ProximaDB native library:

```bash
cd /path/to/proximadb
cargo build --release --features c_ffi
```

Then add to your Go project:

```bash
go get github.com/vjsingh1984/proximadb/clients/go-embedded
```

## Quick Start

```go
package main

import (
    "fmt"
    "log"

    proximadb "github.com/vjsingh1984/proximadb/clients/go-embedded"
)

func main() {
    // Open database
    db, err := proximadb.Open("./my_database", nil)
    if err != nil {
        log.Fatal(err)
    }
    defer db.Close()

    // Create collection
    err = db.CreateCollection("embeddings", 128, "")
    if err != nil {
        log.Fatal(err)
    }

    // Insert vectors
    ids := []string{"vec_0", "vec_1", "vec_2"}
    vectors := [][]float32{
        make([]float32, 128),
        make([]float32, 128),
        make([]float32, 128),
    }
    // Fill vectors with data...

    err = db.Insert("embeddings", ids, vectors)
    if err != nil {
        log.Fatal(err)
    }

    // Search
    query := make([]float32, 128)
    results, err := db.Search("embeddings", query, 10)
    if err != nil {
        log.Fatal(err)
    }

    for _, r := range results {
        fmt.Printf("%s: %.4f\n", r.ID, r.Score)
    }
}
```

## Multi-Disk Configuration

```go
disks := []proximadb.DiskConfig{
    {Path: "/nvme/data", Weight: 2},  // Fast SSD
    {Path: "/hdd/data", Weight: 1},   // Slower HDD
}

config := &proximadb.Config{
    MetadataDir:   "/nvme/metadata",
    CacheSizeMB:   2048,
    DefaultEngine: "sst",
}

db, err := proximadb.OpenMultiDisk(disks, config)
```

## API Reference

### Types

- `DB` - Database handle
- `Config` - Configuration options
- `DiskConfig` - Disk configuration for multi-disk setups
- `SearchResult` - Search result containing ID and score

### Functions

- `Open(dataDir string, config *Config) (*DB, error)`
- `OpenMultiDisk(disks []DiskConfig, config *Config) (*DB, error)`
- `Version() string`

### DB Methods

- `Close()`
- `CreateCollection(name string, dimension int, engine string) error`
- `DeleteCollection(name string) error`
- `Insert(collection string, ids []string, vectors [][]float32) error`
- `Search(collection string, query []float32, topK int) ([]SearchResult, error)`
- `Flush() error`

## Building from Source

### Prerequisites

- Go 1.21+
- Rust 1.88+
- CGO enabled

### Build Steps

```bash
# Build Rust library
cd /path/to/proximadb
cargo build --release --features c_ffi

# Build Go package
cd clients/go-embedded
go build

# Run tests
go test -v
```

## Environment Variables

Set the library path before running:

```bash
# Linux
export LD_LIBRARY_PATH=/path/to/proximadb/target/release:$LD_LIBRARY_PATH

# macOS
export DYLD_LIBRARY_PATH=/path/to/proximadb/target/release:$DYLD_LIBRARY_PATH
```

## License

Apache License 2.0
