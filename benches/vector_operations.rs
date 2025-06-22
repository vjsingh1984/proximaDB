//! Vector operations benchmarks

use std::collections::HashMap;

fn generate_random_vector(dimension: usize) -> Vec<f32> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..dimension).map(|_| rng.gen::<f32>()).collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot_product / (norm_a * norm_b)
    }
}

fn benchmark_vector_operations() {
    println!("🚀 VECTOR OPERATIONS BENCHMARK");
    println!("===============================================");
    
    let dimensions = [384, 768, 1536];
    let iterations = 10000;
    
    for dim in dimensions {
        println!("\n📊 Dimension: {}", dim);
        
        let vec1 = generate_random_vector(dim);
        let vec2 = generate_random_vector(dim);
        
        // Benchmark cosine similarity
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            let _ = cosine_similarity(&vec1, &vec2);
        }
        let duration = start.elapsed();
        let ops_per_sec = iterations as f64 / duration.as_secs_f64();
        
        println!("   Cosine similarity: {:.1} ops/sec", ops_per_sec);
        
        // Benchmark vector creation
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            let _ = generate_random_vector(dim);
        }
        let duration = start.elapsed();
        let creation_ops_per_sec = iterations as f64 / duration.as_secs_f64();
        
        println!("   Vector creation: {:.1} ops/sec", creation_ops_per_sec);
    }
    
    println!("\n✅ Vector operations benchmark completed");
}

fn main() {
    benchmark_vector_operations();
}