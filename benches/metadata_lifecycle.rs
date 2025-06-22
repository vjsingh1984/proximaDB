//! Metadata lifecycle benchmarks

use std::collections::HashMap;

fn create_test_metadata(index: usize) -> HashMap<String, serde_json::Value> {
    let mut meta = HashMap::new();
    
    // Filterable fields
    meta.insert("category".to_string(), 
        serde_json::Value::String(format!("category_{}", index % 10)));
    meta.insert("author".to_string(), 
        serde_json::Value::String(format!("author_{}", index % 50)));
    meta.insert("doc_type".to_string(), 
        serde_json::Value::String("research_paper".to_string()));
    meta.insert("year".to_string(), 
        serde_json::Value::Number(serde_json::Number::from(2020 + (index % 5))));
    meta.insert("length".to_string(), 
        serde_json::Value::Number(serde_json::Number::from(1000 + (index % 5000))));
    
    // Extra metadata fields
    meta.insert("title".to_string(), 
        serde_json::Value::String(format!("Document Title {}", index)));
    meta.insert("abstract".to_string(), 
        serde_json::Value::String(format!("Abstract for document {}", index)));
    meta.insert("keywords".to_string(), 
        serde_json::Value::Array(vec![
            serde_json::Value::String("keyword1".to_string()),
            serde_json::Value::String("keyword2".to_string()),
        ]));
    meta.insert("citation_count".to_string(), 
        serde_json::Value::Number(serde_json::Number::from(index % 100)));
    meta.insert("download_count".to_string(), 
        serde_json::Value::Number(serde_json::Number::from(index * 10)));
    meta.insert("quality_score".to_string(), 
        serde_json::Value::Number(serde_json::Number::from_f64(0.5 + (index % 50) as f64 / 100.0).unwrap()));
    meta.insert("processing_timestamp".to_string(), 
        serde_json::Value::String("2024-01-15T10:30:00Z".to_string()));
    meta.insert("content_hash".to_string(), 
        serde_json::Value::String(format!("sha256:hash_{}", index)));
    meta.insert("language".to_string(), 
        serde_json::Value::String("english".to_string()));
    meta.insert("region".to_string(), 
        serde_json::Value::String("north_america".to_string()));
    
    meta
}

fn transform_metadata(
    metadata: &HashMap<String, serde_json::Value>,
    filterable_columns: &[&str]
) -> (HashMap<String, serde_json::Value>, HashMap<String, serde_json::Value>) {
    let mut filterable = HashMap::new();
    let mut extra_meta = HashMap::new();
    
    for (key, value) in metadata {
        if filterable_columns.contains(&key.as_str()) {
            filterable.insert(key.clone(), value.clone());
        } else {
            extra_meta.insert(key.clone(), value.clone());
        }
    }
    
    (filterable, extra_meta)
}

fn benchmark_metadata_lifecycle() {
    println!("🚀 METADATA LIFECYCLE BENCHMARK");
    println!("===============================================");
    
    let filterable_columns = vec!["category", "author", "doc_type", "year", "length"];
    let record_counts = [100, 500, 1000, 5000];
    
    for count in record_counts {
        println!("\n📊 Record count: {}", count);
        
        // Phase 1: Insert (as-is storage) - should be very fast
        let start = std::time::Instant::now();
        let metadata_records: Vec<_> = (0..count)
            .map(|i| create_test_metadata(i))
            .collect();
        let insert_duration = start.elapsed();
        let insert_ops_per_sec = count as f64 / insert_duration.as_secs_f64();
        
        println!("   Insert (as-is): {:.1} records/sec", insert_ops_per_sec);
        
        // Phase 2: Flush (metadata transformation)
        let start = std::time::Instant::now();
        let transformed_records: Vec<_> = metadata_records
            .iter()
            .map(|meta| transform_metadata(meta, &filterable_columns))
            .collect();
        let transform_duration = start.elapsed();
        let transform_ops_per_sec = count as f64 / transform_duration.as_secs_f64();
        
        println!("   Transform: {:.1} records/sec", transform_ops_per_sec);
        
        // Phase 3: Search simulation (memtable vs VIPER)
        
        // Memtable search (linear scan)
        let start = std::time::Instant::now();
        let mut memtable_matches = 0;
        for metadata in &metadata_records {
            if let Some(category) = metadata.get("category") {
                if category.as_str() == Some("category_1") {
                    memtable_matches += 1;
                }
            }
        }
        let memtable_duration = start.elapsed();
        let memtable_ops_per_sec = count as f64 / memtable_duration.as_secs_f64();
        
        // VIPER search (indexed lookup simulation)
        let start = std::time::Instant::now();
        let mut viper_matches = 0;
        for (filterable, _) in &transformed_records {
            if let Some(category) = filterable.get("category") {
                if category.as_str() == Some("category_1") {
                    viper_matches += 1;
                }
            }
        }
        let viper_duration = start.elapsed();
        let viper_ops_per_sec = count as f64 / viper_duration.as_secs_f64();
        
        println!("   Memtable search: {:.1} records/sec", memtable_ops_per_sec);
        println!("   VIPER search: {:.1} records/sec", viper_ops_per_sec);
        
        let speedup = viper_ops_per_sec / memtable_ops_per_sec;
        println!("   VIPER speedup: {:.1}x", speedup);
        
        // Verify data integrity
        assert_eq!(memtable_matches, viper_matches);
        assert_eq!(transformed_records.len(), count);
    }
    
    println!("\n✅ Metadata lifecycle benchmark completed");
}

fn main() {
    benchmark_metadata_lifecycle();
}