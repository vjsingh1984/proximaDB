use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::storage::engines::core::formats::proximablocks::{
    BlockCompressionConfig, ProximaDataBlock, VectorEncodingLayout,
};
use std::collections::HashMap;

fn create_simple_vectors() -> Vec<VectorRecord> {
    let mut vectors = Vec::new();
    for i in 0..2 {
        vectors.push(VectorRecord {
            id: format!("v{}", i),
            vector: vec![1.0, 2.0],
            metadata: HashMap::new(),
            expires_at: None,
            source: None,
            timestamp: Some(100 + i),
            updated_at: None,
            version: None,
        });
    }
    vectors
}

fn analyze_layout() -> anyhow::Result<()> {
    println!("🔍 PROXIMA LAYOUT ANALYSIS");
    println!("============================");

    let vectors = create_simple_vectors();
    println!(
        "📊 Test data: {} vectors × {} dimensions",
        vectors.len(),
        vectors[0].vector.len()
    );

    let config = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::FullVector,
        algorithm: CompressionAlgorithm::None, // Uncompressed for clearer analysis
        compression_level: 1,
        enable_vector_compression: false,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
        metadata_algorithm: None,
    };

    let proxima_records: Vec<_> = vectors
        .into_iter()
        .map(proximadb::proto::defaults::vector_record_to_proxima_record)
        .collect();
    let block = ProximaDataBlock::new(proxima_records, config.clone());
    let encoded = block.serialize_with_config(&config)?;

    println!("\n📏 SERIALIZATION ANALYSIS:");
    println!("Total encoded size: {} bytes", encoded.len());

    // Manual layout parsing
    let mut pos = 0;

    // Expected layout from serialization code:
    println!("\n🔍 MANUAL BYTE ANALYSIS:");
    println!(
        "Position {}: Compression marker = 0x{:02X}",
        pos, encoded[pos]
    );
    pos += 1;

    if encoded[0] == 0x00 {
        // Uncompressed - next bytes are format version + encoding marker
        println!("Position {}: Format version = {}", pos, encoded[pos]);
        pos += 1;
        println!("Position {}: Encoding marker = 0x{:02X}", pos, encoded[pos]);
        pos += 1;

        // Record count (4 bytes, u32 LE)
        let record_count = u32::from_le_bytes([
            encoded[pos],
            encoded[pos + 1],
            encoded[pos + 2],
            encoded[pos + 3],
        ]);
        println!(
            "Position {}-{}: Record count = {} (bytes: {:02X?})",
            pos,
            pos + 3,
            record_count,
            &encoded[pos..pos + 4]
        );
        pos += 4;

        // Dimension (4 bytes, u32 LE)
        let dimension = u32::from_le_bytes([
            encoded[pos],
            encoded[pos + 1],
            encoded[pos + 2],
            encoded[pos + 3],
        ]);
        println!(
            "Position {}-{}: Dimension = {} (bytes: {:02X?})",
            pos,
            pos + 3,
            dimension,
            &encoded[pos..pos + 4]
        );
        pos += 4;

        // Vector data length (4 bytes, u32 LE)
        let vector_len = u32::from_le_bytes([
            encoded[pos],
            encoded[pos + 1],
            encoded[pos + 2],
            encoded[pos + 3],
        ]);
        println!(
            "Position {}-{}: Vector data length = {} (bytes: {:02X?})",
            pos,
            pos + 3,
            vector_len,
            &encoded[pos..pos + 4]
        );
        pos += 4;

        // Vector data
        println!(
            "Position {}-{}: Vector data = {} bytes",
            pos,
            pos + vector_len as usize - 1,
            vector_len
        );
        if vector_len >= 2 {
            println!(
                "  First 2 bytes of vector data: [0x{:02X}, 0x{:02X}]",
                encoded[pos],
                encoded[pos + 1]
            );
        }
        pos += vector_len as usize;

        // ID dictionary length (4 bytes, u32 LE)
        let id_dict_len = u32::from_le_bytes([
            encoded[pos],
            encoded[pos + 1],
            encoded[pos + 2],
            encoded[pos + 3],
        ]);
        println!(
            "Position {}-{}: ID dictionary length = {} (bytes: {:02X?})",
            pos,
            pos + 3,
            id_dict_len,
            &encoded[pos..pos + 4]
        );
        pos += 4;

        // ID dictionary entries
        for i in 0..id_dict_len {
            let id_str_len = u32::from_le_bytes([
                encoded[pos],
                encoded[pos + 1],
                encoded[pos + 2],
                encoded[pos + 3],
            ]);
            println!(
                "Position {}-{}: ID[{}] string length = {} (bytes: {:02X?})",
                pos,
                pos + 3,
                i,
                id_str_len,
                &encoded[pos..pos + 4]
            );
            pos += 4;

            let id_str = String::from_utf8(encoded[pos..pos + id_str_len as usize].to_vec())?;
            println!(
                "Position {}-{}: ID[{}] string = '{}' (bytes: {:02X?})",
                pos,
                pos + id_str_len as usize - 1,
                i,
                id_str,
                &encoded[pos..pos + id_str_len as usize]
            );
            pos += id_str_len as usize;
        }

        // ID indices length (4 bytes, u32 LE)
        let id_indices_len = u32::from_le_bytes([
            encoded[pos],
            encoded[pos + 1],
            encoded[pos + 2],
            encoded[pos + 3],
        ]);
        println!(
            "Position {}-{}: ID indices length = {} (bytes: {:02X?})",
            pos,
            pos + 3,
            id_indices_len,
            &encoded[pos..pos + 4]
        );
        pos += 4;

        // ID indices data
        println!(
            "Position {}-{}: ID indices data = {} bytes",
            pos,
            pos + id_indices_len as usize - 1,
            id_indices_len
        );
        pos += id_indices_len as usize;

        // Metadata key count (4 bytes, u32 LE)
        let metadata_count = u32::from_le_bytes([
            encoded[pos],
            encoded[pos + 1],
            encoded[pos + 2],
            encoded[pos + 3],
        ]);
        println!(
            "Position {}-{}: Metadata key count = {} (bytes: {:02X?})",
            pos,
            pos + 3,
            metadata_count,
            &encoded[pos..pos + 4]
        );
        pos += 4;

        // Metadata entries (if any)
        for i in 0..metadata_count {
            println!("Position {}: Metadata key[{}] processing...", pos, i);
            // Skip detailed metadata parsing for now
            break;
        }

        // Find timestamp length at expected position
        println!("\n🎯 SEARCHING FOR TIMESTAMP LENGTH [11, 0, 0, 0]:");
        for search_pos in pos..encoded.len().saturating_sub(4) {
            if encoded[search_pos..search_pos + 4] == [11, 0, 0, 0] {
                println!("✅ Found [11, 0, 0, 0] at position {}", search_pos);
                println!(
                    "   4 bytes before: {:02X?}",
                    &encoded[search_pos.saturating_sub(4)..search_pos]
                );
                println!(
                    "   4 bytes after:  {:02X?}",
                    &encoded[search_pos + 4..search_pos + 8.min(encoded.len())]
                );
                break;
            }
        }
    }

    println!("\n📋 FIELD SIZE SUMMARY:");
    println!("- Compression marker: 1 byte");
    println!("- Format version: 1 byte");
    println!("- Encoding marker: 1 byte");
    println!("- Record count: 4 bytes (u32)");
    println!("- Dimension: 4 bytes (u32)");
    println!("- Vector data length: 4 bytes (u32)");
    println!("- ID dictionary length: 4 bytes (u32)");
    println!("- ID string length: 4 bytes (u32) per string");
    println!("- ID indices length: 4 bytes (u32)");
    println!("- Metadata key count: 4 bytes (u32)");
    println!("- Timestamp length: 4 bytes (u32)");
    println!("- Block metadata length: 4 bytes (u32)");

    Ok(())
}

fn main() -> anyhow::Result<()> {
    analyze_layout()
}
