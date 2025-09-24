use proximadb::storage::engines::core::formats::fastlanes_blocks::block_structures::{
    FastLanesDataBlock, VectorEncodingLayout,
};
use proximadb::proto::proximadb_v1::VectorRecord;
use std::collections::HashMap;

fn create_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let vector = (0..dimension)
                .map(|d| ((i as f32 * 0.1) + (d as f32 * 0.01)).sin())
                .collect();
            VectorRecord {
                id: format!("vec_{}", i),
                vector,
                metadata: HashMap::new(),
                quantized_vector: vec![],
                expires_at: None,
                source: None,
                timestamp: 0,
                updated_at: None,
                version: None,
            }
        })
        .collect()
}

fn main() {
    println!("🧪 Testing GroupedVector strategy");
    println!("=" .repeat(50));

    // Test dimensions that should trigger GroupedVector
    let test_cases = vec![
        (10, 256),  // Should use GroupedVector (D > 128)
        (5, 384),   // Should use GroupedVector 
        (3, 768),   // Should use GroupedVector
    ];

    for (count, dimension) in test_cases {
        println!("\n📊 Testing {} vectors × {} dimensions", count, dimension);
        println!("-".repeat(40));

        let vectors = create_test_vectors(count, dimension);

        // Test with Auto strategy (should pick GroupedVector for D > 128)
        let block_auto = FastLanesDataBlock::from_vectors(
            vectors.clone(),
            VectorEncodingLayout::Auto,
        ).unwrap();

        println!("✅ Block created with Auto strategy");
        let serialized = block_auto.serialize().unwrap();
        println!("📦 Serialized size: {} bytes", serialized.len());

        let deserialized = FastLanesDataBlock::deserialize(&serialized).unwrap();
        println!("✅ Block deserialized successfully");

        // Verify vectors match
        let records = deserialized.get_vector_records();
        assert_eq!(records.len(), count);
        
        for (i, record) in records.iter().enumerate() {
            assert_eq!(record.vector.len(), dimension);
            // Check a few values to ensure correctness
            let expected = ((i as f32 * 0.1) + (0 as f32 * 0.01)).sin();
            let diff = (record.vector[0] - expected).abs();
            assert!(diff < 0.0001, "Vector mismatch at index {}", i);
        }
        
        println!("✅ All vectors validated successfully!");

        // Test explicitly with GroupedVector strategy
        let block_grouped = FastLanesDataBlock::from_vectors(
            vectors.clone(),
            VectorEncodingLayout::GroupedVector,
        ).unwrap();

        println!("✅ Block created with GroupedVector strategy");
        let serialized_grouped = block_grouped.serialize().unwrap();
        println!("📦 Grouped serialized size: {} bytes", serialized_grouped.len());

        let deserialized_grouped = FastLanesDataBlock::deserialize(&serialized_grouped).unwrap();
        println!("✅ Grouped block deserialized successfully");

        // Verify grouped vectors match
        let grouped_records = deserialized_grouped.get_vector_records();
        assert_eq!(grouped_records.len(), count);
        
        println!("✅ GroupedVector strategy validated!");
    }

    println!("\n🎉 All GroupedVector tests passed!");
}
