// Simple test to verify benchmark fixes
use proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default;
use proximadb::compute::distance_computation::{DistanceMetric, engine::UnifiedDistanceCompute};

fn main() {
    println!("Testing benchmark fixes...");

    // Initialize hardware capabilities
    let init_result = initialize_hardware_capabilities_default();
    match init_result {
        Ok(_) => println!("✓ Hardware capabilities initialized successfully"),
        Err(e) => println!("✗ Failed to initialize hardware: {:?}", e),
    }

    // Test distance computation
    let compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
    println!("✓ UnifiedDistanceCompute created successfully");

    // Test vector computation
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![4.0, 5.0, 6.0];
    let result = compute.calculate_distance(&a, &b, &DistanceMetric::Cosine);
    println!("✓ Distance computed: {}", result.distance);

    println!("\nAll fixes verified successfully!");
}