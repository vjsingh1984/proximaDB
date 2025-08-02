    /// Create test WAL entries with specified vectors and collection using modern AvroPayload format
    fn create_test_wal_entries(collection_id: &str) -> Vec<WalEntry> {
        let now = Utc::now();

        vec![
            WalEntry {
                entry_id: "entry_1".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 1,
                global_sequence: 1,
                timestamp: now as u32,
                expires_at: None,
                version: Some(1),
                batch_id: None,
                operation: WriteBufferOperation::AvroPayload {
                    operation_type: "upsert".to_string(),
                    avro_data: create_test_vector_record("vector_1", collection_id, vec![1.0, 0.0, 0.0], now)
                        .to_avro_bytes().expect("Failed to serialize test vector"),
                },
            },
            WalEntry {
                entry_id: "entry_2".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 2,
                global_sequence: 2,
                timestamp: now as u32,
                expires_at: None,
                version: Some(1),
                batch_id: None,
                operation: WriteBufferOperation::AvroPayload {
                    operation_type: "upsert".to_string(),
                    avro_data: create_test_vector_record("vector_2", collection_id, vec![0.0, 1.0, 0.0], now)
                        .to_avro_bytes().expect("Failed to serialize test vector"),
                },
            },
            WalEntry {
                entry_id: "entry_3".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 3,
                global_sequence: 3,
                timestamp: now as u32,
                expires_at: None,
                version: Some(1),
                batch_id: None,
                operation: WriteBufferOperation::AvroPayload {
                    operation_type: "upsert".to_string(),
                    avro_data: create_test_vector_record("vector_3", collection_id, vec![0.0, 0.0, 1.0], now)
                        .to_avro_bytes().expect("Failed to serialize test vector"),
                },
            },
            WalEntry {
                entry_id: "entry_4".to_string(),
                collection_id: collection_id.to_string(),
                sequence: 4,
                global_sequence: 4,
                timestamp: now as u32,
                expires_at: None,
                version: Some(1),
                batch_id: None,
                operation: WriteBufferOperation::AvroPayload {
                    operation_type: "upsert".to_string(),
                    avro_data: create_test_vector_record("vector_4", collection_id, vec![-1.0, 0.0, 0.0], now)
                        .to_avro_bytes().expect("Failed to serialize test vector"),
                },
            },
        ]
    }