//! Unified Quantization Tests

use crate::compute::quantization::types::{
    UnifiedQuantizationLevel, QuantizationLevel, 
    UniformQuantization, BinaryQuantization, 
    ProductQuantization, ScalarQuantization, NoQuantization
};
use crate::compute::quantization::unified::UnifiedQuantizationEngine;
use crate::core::hardware_capabilities::initialize_hardware_capabilities_default;
use std::sync::Once;

static INIT: Once = Once::new();

fn setup_hardware_capabilities() {
    INIT.call_once(|| {
        let _ = initialize_hardware_capabilities_default();
    });
}

#[test]
fn test_quantization_level_creation() {
    setup_hardware_capabilities();
    
    // Test PQ8 creation
    let pq8 = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Pq(ProductQuantization {
            bits_per_code: 8,
            num_subvectors: 16,
            codebook_id: None,
        })),
    };
    
    assert!(pq8.level_type.is_some());
    if let Some(QuantizationLevel::Pq(pq)) = &pq8.level_type {
        assert_eq!(pq.bits_per_code, 8);
        assert_eq!(pq.num_subvectors, 16);
    }
    
    // Test Uniform quantization
    let uniform4 = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Uniform(UniformQuantization {
            bits: 4,
            scale: None,
            offset: None,
        })),
    };
    
    assert!(uniform4.level_type.is_some());
    if let Some(QuantizationLevel::Uniform(uniform)) = &uniform4.level_type {
        assert_eq!(uniform.bits, 4);
    }
    
    // Test Binary quantization
    let binary = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Binary(BinaryQuantization {
            threshold: None,
            sign_based: false,
        })),
    };
    
    assert!(binary.level_type.is_some());
    if let Some(QuantizationLevel::Binary(bin)) = &binary.level_type {
        assert!(!bin.sign_based);
    }
}

#[test]
fn test_quantization_none() {
    setup_hardware_capabilities();
    
    let none_quant = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::None(NoQuantization {})),
    };
    
    assert!(none_quant.level_type.is_some());
    assert!(matches!(none_quant.level_type, Some(QuantizationLevel::None(_))));
}

#[test]
fn test_scalar_quantization() {
    setup_hardware_capabilities();
    
    let scalar_int8 = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Scalar(ScalarQuantization {
            bits: 8,
            signed: true,
        })),
    };
    
    assert!(scalar_int8.level_type.is_some());
    if let Some(QuantizationLevel::Scalar(scalar)) = &scalar_int8.level_type {
        assert_eq!(scalar.bits, 8);
        assert!(scalar.signed);
    }
}

#[test]
fn test_quantization_equality() {
    setup_hardware_capabilities();
    
    let pq1 = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Pq(ProductQuantization {
            bits_per_code: 8,
            num_subvectors: 16,
            codebook_id: None,
        })),
    };
    
    let pq2 = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Pq(ProductQuantization {
            bits_per_code: 8,
            num_subvectors: 16,
            codebook_id: None,
        })),
    };
    
    assert_eq!(pq1, pq2);
}

#[test]
fn test_quantization_cloning() {
    setup_hardware_capabilities();
    
    let original = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Binary(BinaryQuantization {
            threshold: Some(0.5),
            sign_based: true,
        })),
    };
    
    let cloned = original.clone();
    assert_eq!(original, cloned);
}

#[test]
fn test_quantization_hash() {
    setup_hardware_capabilities();
    
    use std::collections::HashMap;
    
    let quant = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Uniform(UniformQuantization {
            bits: 4,
            scale: Some(1.0),
            offset: Some(0.0),
        })),
    };
    
    let mut map = HashMap::new();
    map.insert(quant.clone(), "test_value");
    
    assert_eq!(map.get(&quant), Some(&"test_value"));
}

#[test]
fn test_default_creation() {
    setup_hardware_capabilities();
    
    let empty = UnifiedQuantizationLevel {
        level_type: None,
    };
    
    assert!(empty.level_type.is_none());
}

#[test]
fn test_quantization_serialization() {
    setup_hardware_capabilities();
    
    let quant = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Pq(ProductQuantization {
            bits_per_code: 8,
            num_subvectors: 32,
            codebook_id: Some("test_codebook".to_string()),
        })),
    };
    
    // Test that serialization traits are available
    let serialized = serde_json::to_string(&quant).expect("Should serialize");
    let deserialized: UnifiedQuantizationLevel = serde_json::from_str(&serialized).expect("Should deserialize");
    
    assert_eq!(quant, deserialized);
}

#[test]
fn test_different_quantization_types() {
    setup_hardware_capabilities();
    
    let binary = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Binary(BinaryQuantization {
            threshold: Some(0.0),
            sign_based: false,
        })),
    };
    
    let scalar = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Scalar(ScalarQuantization {
            bits: 8,
            signed: true,
        })),
    };
    
    let uniform = UnifiedQuantizationLevel {
        level_type: Some(QuantizationLevel::Uniform(UniformQuantization {
            bits: 4,
            scale: None,
            offset: None,
        })),
    };
    
    // All should be different
    assert_ne!(binary, scalar);
    assert_ne!(scalar, uniform);
    assert_ne!(binary, uniform);
}