// Test to verify bloom filters are written to Parquet files
use arrow_array::RecordBatch;
use arrow_array::{Int32Array, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧪 Testing Parquet bloom filter support...\n");

    // Create test data
    let ids = StringArray::from(vec!["id1", "id2", "id3", "id4", "id5"]);
    let values = Int32Array::from(vec![1, 2, 3, 4, 5]);

    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("value", DataType::Int32, false),
    ]));

    let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(ids), Arc::new(values)])?;

    // Configure writer with bloom filters
    let props = WriterProperties::builder()
        .set_column_bloom_filter_enabled("id".into(), true)
        .set_column_bloom_filter_fpp("id".into(), 0.01)
        .set_column_bloom_filter_enabled("value".into(), true)
        .set_column_bloom_filter_fpp("value".into(), 0.01)
        .build();

    // Write Parquet file
    let file = File::create("/tmp/test_bloom.parquet")?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&batch)?;
    writer.close()?;

    println!("✅ Parquet file written with bloom filter configuration");

    // Read back and check metadata
    let file = File::open("/tmp/test_bloom.parquet")?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let metadata = reader.metadata();

    println!("\n📊 Parquet file metadata:");
    println!("  Row groups: {}", metadata.num_row_groups());

    let mut has_bloom = false;
    for i in 0..metadata.num_row_groups() {
        let row_group = metadata.row_group(i);
        println!("\n  Row group {}:", i);
        for col_idx in 0..row_group.num_columns() {
            let col = row_group.column(col_idx);
            let bloom_offset = col.bloom_filter_offset();
            let bloom_length = col.bloom_filter_length();

            if bloom_offset.is_some() || bloom_length.is_some() {
                has_bloom = true;
            }

            println!(
                "    Column {}: bloom_filter_offset = {:?}, bloom_filter_length = {:?}",
                col_idx, bloom_offset, bloom_length
            );
        }
    }

    if has_bloom {
        println!(
            "\n✅ BLOOM FILTERS CONFIRMED: ArrowWriter DOES write bloom filters to Parquet files!"
        );
    } else {
        println!("\n❌ WARNING: No bloom filters found in Parquet metadata!");
        println!("   This might mean:");
        println!("   1. The parquet version doesn't support bloom filters");
        println!("   2. Bloom filters are not being written despite configuration");
    }

    // Clean up
    std::fs::remove_file("/tmp/test_bloom.parquet").ok();

    Ok(())
}
