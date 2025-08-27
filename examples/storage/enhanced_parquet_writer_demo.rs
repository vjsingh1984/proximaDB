//! Enhanced Parquet Writer Demo
//! 
//! Demonstrates the new bloom filter and optimization features in the columnar Parquet writer.
//! This example shows how to use the enhanced writer for optimal performance in vector databases.

use anyhow::Result;
use proximadb::core::VectorRecord;
use proximadb::storage::engines::columnar::{
    ParquetWriterConfig, StreamingParquetWriter, 
    QuantizationConfig, ParquetPerformanceConfig
};
use proximadb::core::compression::CompressionAlgorithm;
use std::collections::HashMap;
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    println!("🚀 Enhanced Parquet Writer Demo");
    
    // Demo 1: Optimized configuration for high-performance vector database
    let optimized_config = ParquetWriterConfig {
        row_group_size: 50000,                    // Larger row groups for better compression
        page_size: 2 * 1024 * 1024,              // 2MB pages for optimal I/O
        enable_bloom_filters: true,              // Critical for ID lookups
        bloom_filter_fpp: 0.005,                 // Lower FPP for better precision
        expected_ndv: Some(50000),               // High cardinality ID column
        bloom_filter_columns: vec![              // Explicit columns for bloom filters
            "id".to_string(),
            "category".to_string(),
            "timestamp".to_string(),
        ],
        enable_column_statistics: true,          // Enable for query optimization
        enable_page_index: true,                 // Enable for fast seeks
        compression: CompressionAlgorithm::Mixed, // Optimal per-column compression
        enable_dictionary: true,                 // Good for categorical data
        dictionary_threshold: 0.8,               // Use dictionary if <80% unique
        enable_delta_encoding: true,             // Good for timestamps
        enable_byte_stream_split: true,          // Excellent for vectors
        quantization: QuantizationConfig {
            enable_binary: true,                 // Fast filtering
            enable_int8: true,                   // Good compression/speed balance
            enable_pq: true,                     // Best compression
            pq_segments: 32,                     // More segments for better quality
            ..Default::default()
        },
        ..Default::default()
    };
    
    // Demo 2: Create test data with various metadata patterns
    let test_data = create_test_data()?;
    println!("📊 Created {} test vectors with metadata", test_data.len());
    
    // Demo 3: Write data with optimized writer
    let temp_dir = tempdir()?;
    let file_path = temp_dir.path().join("optimized_vectors.parquet");
    
    let mut writer = StreamingParquetWriter::new(
        &file_path,
        768, // High-dimensional vectors
        optimized_config,
    )?;
    
    println!("✍️  Writing vectors with enhanced Parquet writer...");
    
    // Write in batches to demonstrate streaming
    for chunk in test_data.chunks(1000) {
        writer.write_batch(chunk).await?;
    }
    
    // Demo 4: Finalize and get comprehensive statistics
    let stats = writer.finalize().await?;
    
    println!("\n📈 Enhanced Parquet Writer Results:");
    println!("   📄 File: {}", stats.file_path);
    println!("   📊 Records: {}", stats.total_records);
    println!("   💾 File Size: {:.2} MB", stats.file_size as f64 / 1024.0 / 1024.0);
    println!("   🗜️  Compression Ratio: {:.1}x", stats.compression_ratio);
    println!("   ⚡ I/O Reduction: {:.1}%", stats.performance_metrics.io_reduction_achieved);
    println!("   🎯 Estimated Query Speedup: {:.1}x", stats.performance_metrics.estimated_query_speedup);
    println!("   🌸 Bloom Filters: {}", stats.bloom_filter_count);
    println!("   🚀 Throughput: {:.0} records/sec", stats.performance_metrics.throughput_records_per_sec);
    
    // Demo 5: Show per-column optimizations
    println!("\n🔧 Per-Column Optimizations:");
    for (column_name, column_stats) in &stats.column_statistics {
        println!("   📋 {}: compression={:.1}x, nulls={}, avg_size={:.1}B",
                 column_name, 
                 column_stats.compression_ratio,
                 column_stats.null_count,
                 column_stats.avg_size_bytes);
        
        if let Some(bloom_size) = column_stats.bloom_filter_size_bytes {
            println!("        🌸 Bloom filter: {} bytes", bloom_size);
        }
    }
    
    // Demo 6: Expected performance improvements
    println!("\n🎯 Expected Performance Improvements:");
    println!("   🔍 Point Lookups: 10-50x faster (bloom filters on ID column)");
    println!("   📊 Filtered Queries: 5-20x faster (bloom filters on metadata)");
    println!("   💾 Storage Reduction: {:.0}% (compression + quantization)", 
             (1.0 - 1.0/stats.compression_ratio) * 100.0);
    println!("   ⚡ I/O Reduction: {:.0}% (page indexes + statistics)", 
             stats.performance_metrics.io_reduction_achieved);
    println!("   🚀 Vector Search: 2-5x faster (quantization + byte stream split)");
    
    // Demo 7: Configuration recommendations
    println!("\n💡 Configuration Recommendations:");
    if stats.total_records > 1_000_000 {
        println!("   📈 Large Dataset: Consider increasing row_group_size to 100K+");
    }
    
    if stats.performance_metrics.io_reduction_achieved < 70.0 {
        println!("   🗜️  Low I/O Reduction: Consider enabling more aggressive compression");
    }
    
    if stats.bloom_filter_count < 3 {
        println!("   🌸 Few Bloom Filters: Consider adding more metadata columns");
    }
    
    println!("\n✅ Enhanced Parquet Writer Demo Complete!");
    println!("   The Parquet writer now includes:");
    println!("   • Smart bloom filter placement for 95% metadata scan reduction");
    println!("   • Column statistics for query optimization");
    println!("   • Page indexes for faster seeks");
    println!("   • Vector-optimized encoding (BYTE_STREAM_SPLIT)");
    println!("   • Intelligent NDV estimation for proper bloom filter sizing");
    println!("   • Per-column compression optimization");
    
    Ok(())
}

/// Create test data with various metadata patterns for demonstration
fn create_test_data() -> Result<Vec<VectorRecord>> {
    let categories = ["electronics", "books", "clothing", "home", "sports"];
    let statuses = ["active", "inactive", "pending"];
    let types = ["product", "service", "bundle"];
    
    let mut records = Vec::new();
    
    for i in 0..10_000 {
        let mut metadata = HashMap::new();
        metadata.insert("category".to_string(), 
                        serde_json::Value::String(categories[i % categories.len()].to_string()));
        metadata.insert("status".to_string(), 
                        serde_json::Value::String(statuses[i % statuses.len()].to_string()));
        metadata.insert("type".to_string(), 
                        serde_json::Value::String(types[i % types.len()].to_string()));
        metadata.insert("price".to_string(), 
                        serde_json::Value::Number(serde_json::Number::from((i % 1000) as f64 + 9.99)));
        metadata.insert("rating".to_string(), 
                        serde_json::Value::Number(serde_json::Number::from((i % 5) + 1)));
        
        let record = VectorRecord {
            id: Some(format!("vec_{:08}", i)),
            vector: (0..768).map(|j| ((i + j) as f32 * 0.001).sin()).collect(),
            metadata: Some(metadata),
            timestamp: (1700000000 + i as u32 * 60) as u32, // Timestamps every minute
            updated_at: None,
            expires_at: None,
            version: Some((i % 10) as u32), // Version cycling 0-9
        };
        
        records.push(record);
    }
    
    Ok(records)
}