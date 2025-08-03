#!/bin/bash

# Convert println! to appropriate log levels in critical source files

echo "Converting println! statements in critical source files..."

# Storage engines
sed -i 's/println!("🔧/debug!("🔧/g' src/storage/engines/sst/mod.rs
sed -i 's/println!("🔍/debug!("🔍/g' src/storage/engines/sst/mod.rs
sed -i 's/println!("📊/debug!("📊/g' src/storage/engines/sst/mod.rs
sed -i 's/println!("✅/info!("✅/g' src/storage/engines/sst/mod.rs
sed -i 's/println!("❌/error!("❌/g' src/storage/engines/sst/mod.rs
sed -i 's/println!("⚠️/warn!("⚠️/g' src/storage/engines/sst/mod.rs

# SST reader
sed -i 's/println!(/debug!(/g' src/storage/engines/sst/readers/unified_sstable_reader.rs

# SST search engine
sed -i 's/println!(/debug!(/g' src/storage/engines/sst/unified_search_engine.rs

# Memtable implementations
sed -i 's/println!(/debug!(/g' src/storage/memtable/implementations/global_partitioned.rs
sed -i 's/println!(/debug!(/g' src/storage/memtable/mod.rs

# Core search
sed -i 's/println!(/debug!(/g' src/core/search/mod.rs

# Services
sed -i 's/println!(/debug!(/g' src/services/comprehensive_search_tests.rs

# Memory pool
sed -i 's/println!(/debug!(/g' src/compute/memory_pool.rs

# Add tracing imports where needed
for file in src/storage/engines/sst/mod.rs \
            src/storage/engines/sst/readers/unified_sstable_reader.rs \
            src/storage/engines/sst/unified_search_engine.rs \
            src/storage/memtable/implementations/global_partitioned.rs \
            src/storage/memtable/mod.rs \
            src/core/search/mod.rs \
            src/compute/memory_pool.rs; do
    
    # Check if file exists and needs tracing import
    if [ -f "$file" ]; then
        # Check if tracing is already imported
        if ! grep -q "use tracing::" "$file"; then
            # Find the last use statement and add tracing after it
            # This is a simple approach - may need manual adjustment
            sed -i '/^use .*;$/a\use tracing::{debug, info, warn, error};' "$file"
        fi
    fi
done

echo "Conversion complete. Please review changes and adjust as needed."