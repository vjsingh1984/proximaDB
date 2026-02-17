//! Engine Safety Tests
//!
//! Verifies that experimental engines (SWIFT, RAPTOR) are blocked by default
//! and that production-ready engines (SST, VIPER, HELIX, NOVA) remain available.
//!
//! These tests run WITHOUT the `experimental-engines` feature flag to confirm
//! the safety guards are active.

#[cfg(test)]
mod engine_safety_tests {
    use proximadb::proto::proximadb_v1::StorageEngine as ProtoStorageEngine;
    use proximadb::storage::engines::factory::StorageEngineFactory;

    #[test]
    fn test_swift_blocked_without_feature() {
        let result = StorageEngineFactory::create_from_proto(ProtoStorageEngine::Swift);
        let err_msg = match result {
            Ok(_) => panic!("SWIFT should be blocked without experimental-engines feature"),
            Err(e) => e.to_string(),
        };
        assert!(
            err_msg.contains("experimental"),
            "Error should mention 'experimental', got: {err_msg}"
        );
        assert!(
            err_msg.contains("SST") || err_msg.contains("VIPER"),
            "Error should suggest alternatives, got: {err_msg}"
        );
    }

    #[test]
    fn test_raptor_blocked_without_feature() {
        let result = StorageEngineFactory::create_from_proto(ProtoStorageEngine::Raptor);
        let err_msg = match result {
            Ok(_) => panic!("RAPTOR should be blocked without experimental-engines feature"),
            Err(e) => e.to_string(),
        };
        assert!(
            err_msg.contains("experimental"),
            "Error should mention 'experimental', got: {err_msg}"
        );
        assert!(
            err_msg.contains("SST") || err_msg.contains("VIPER"),
            "Error should suggest alternatives, got: {err_msg}"
        );
    }

    #[test]
    fn test_sst_available() {
        let result = StorageEngineFactory::create_from_proto(ProtoStorageEngine::Sst);
        if let Err(e) = &result {
            panic!("SST should always be available: {e}");
        }
    }

    #[test]
    fn test_viper_available() {
        let result = StorageEngineFactory::create_from_proto(ProtoStorageEngine::Viper);
        if let Err(e) = &result {
            panic!("VIPER should always be available: {e}");
        }
    }

    #[test]
    fn test_helix_available() {
        let result = StorageEngineFactory::create_from_proto(ProtoStorageEngine::Helix);
        if let Err(e) = &result {
            panic!("HELIX should always be available: {e}");
        }
    }

    #[test]
    fn test_nova_available() {
        let result = StorageEngineFactory::create_from_proto(ProtoStorageEngine::Nova);
        if let Err(e) = &result {
            panic!("NOVA should always be available: {e}");
        }
    }
}
