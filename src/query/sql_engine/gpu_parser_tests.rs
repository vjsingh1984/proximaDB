#[cfg(test)]
mod tests {
    use super::super::gpu_parser::*;
    use super::super::parser::{QueryType, VectorQuery};
    use crate::compute::distance_computation::DistanceMetric;
    use std::time::Instant;
    
    #[test]
    fn test_gpu_backend_detection_and_info() {
        let backend = GpuSqlParser::detect_gpu_backend();
        debug!("\n=== GPU Backend Detection ===");
        debug!("Detected backend: {}", backend);
        
        match backend {
            GpuBackend::Cuda => {
                info!("✅ NVIDIA CUDA GPU detected");
                debug!("   Optimal for large-scale vector operations");
            }
            GpuBackend::Rocm => {
                info!("✅ AMD ROCm GPU detected");
                debug!("   Good performance for parallel parsing");
            }
            GpuBackend::Mps => {
                info!("✅ Apple Metal Performance Shaders detected");
                debug!("   Optimized for Apple Silicon");
            }
            GpuBackend::OpenCL => {
                info!("✅ OpenCL GPU detected");
                debug!("   Cross-platform GPU acceleration");
            }
            GpuBackend::None => {
                debug!("ℹ️  No GPU detected - CPU fallback will be used");
            }
        }
    }
    
    #[test]
    fn test_token_classification() {
        let tokenizer = GpuTokenizer {
            backend: GpuBackend::None,
            device: None,
            token_buffer_size: 1024,
        };
        
        // Test keyword classification
        assert_eq!(tokenizer.classify_token("SELECT"), Token::Keyword("SELECT".to_string()));
        assert_eq!(tokenizer.classify_token("select"), Token::Keyword("SELECT".to_string()));
        assert_eq!(tokenizer.classify_token("WHERE"), Token::Keyword("WHERE".to_string()));
        
        // Test identifier classification
        assert_eq!(tokenizer.classify_token("table_name"), Token::Identifier("table_name".to_string()));
        assert_eq!(tokenizer.classify_token("column1"), Token::Identifier("column1".to_string()));
        
        // Test number classification
        assert_eq!(tokenizer.classify_token("123"), Token::Number("123".to_string()));
        assert_eq!(tokenizer.classify_token("45.67"), Token::Number("45.67".to_string()));
        assert_eq!(tokenizer.classify_token("0.001"), Token::Number("0.001".to_string()));
    }
    
    #[test]
    fn test_cpu_fallback_tokenization() {
        let tokenizer = GpuTokenizer {
            backend: GpuBackend::None,
            device: None,
            token_buffer_size: 1024,
        };
        
        // Test simple SELECT query
        let sql = "SELECT * FROM products WHERE price > 100";
        let tokens = tokenizer.tokenize_cpu_fallback(sql).unwrap();
        
        assert_eq!(tokens[0], Token::Keyword("SELECT".to_string()));
        assert_eq!(tokens[1], Token::Operator("*".to_string()));
        assert_eq!(tokens[2], Token::Keyword("FROM".to_string()));
        assert_eq!(tokens[3], Token::Identifier("products".to_string()));
        assert_eq!(tokens[4], Token::Keyword("WHERE".to_string()));
        assert_eq!(tokens[5], Token::Identifier("price".to_string()));
        assert_eq!(tokens[6], Token::Operator(">".to_string()));
        assert_eq!(tokens[7], Token::Number("100".to_string()));
    }
    
    #[test]
    fn test_string_tokenization() {
        let tokenizer = GpuTokenizer {
            backend: GpuBackend::None,
            device: None,
            token_buffer_size: 1024,
        };
        
        // Test string handling
        let sql = "SELECT * FROM users WHERE name = 'John Doe'";
        let tokens = tokenizer.tokenize_cpu_fallback(sql).unwrap();
        
        // Find the string token
        let string_token = tokens.iter().find(|t| {
            if let Token::String(s) = t {
                s.contains("John Doe")
            } else {
                false
            }
        });
        
        assert!(string_token.is_some());
    }
    
    #[test]
    fn test_gpu_parser_creation_and_fallback() {
        let parser = GpuSqlParser::new();
        assert!(parser.is_ok());
        
        let mut parser = parser.unwrap();
        
        // Test simple query parsing
        let sql = "SELECT id, name FROM users LIMIT 10";
        let result = parser.parse(sql);
        
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.query_type, QueryType::Select);
        
        // Check statistics
        let stats = parser.stats();
        assert_eq!(stats.total_queries_parsed, 1);
        assert!(stats.total_parse_time_ms >= 0.0);
    }
    
    #[test]
    fn test_vector_similarity_detection() {
        let mut parser = GpuSqlParser::new().unwrap();
        
        // Query with VECTOR_SIMILARITY
        let sql = "SELECT id, VECTOR_SIMILARITY(embedding, [0.1, 0.2, 0.3], 'cosine') as score FROM documents";
        let result = parser.parse(sql);
        
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert!(parsed.vector_query.is_some());
        
        // Query without VECTOR_SIMILARITY
        let sql2 = "SELECT id, title FROM documents WHERE category = 'tech'";
        let result2 = parser.parse(sql2);
        
        assert!(result2.is_ok());
        let parsed2 = result2.unwrap();
        assert!(parsed2.vector_query.is_none());
    }
    
    #[test]
    fn test_complex_sql_tokenization() {
        let tokenizer = GpuTokenizer {
            backend: GpuBackend::None,
            device: None,
            token_buffer_size: 1024,
        };
        
        let sql = "SELECT p.id, p.name, VECTOR_SIMILARITY(p.vector, [1.0, 2.0], 'euclidean') as dist \
                   FROM products p \
                   WHERE p.category IN ('electronics', 'computers') \
                   AND p.price BETWEEN 100 AND 500 \
                   ORDER BY dist \
                   LIMIT 20";
        
        let tokens = tokenizer.tokenize_cpu_fallback(sql).unwrap();
        
        // Verify we have all major components
        assert!(tokens.contains(&Token::Keyword("SELECT".to_string())));
        assert!(tokens.contains(&Token::Keyword("FROM".to_string())));
        assert!(tokens.contains(&Token::Keyword("WHERE".to_string())));
        assert!(tokens.contains(&Token::Keyword("IN".to_string())));
        assert!(tokens.contains(&Token::Keyword("AND".to_string())));
        assert!(tokens.contains(&Token::Keyword("BETWEEN".to_string())));
        assert!(tokens.contains(&Token::Keyword("ORDER".to_string())));
        assert!(tokens.contains(&Token::Keyword("BY".to_string())));
        assert!(tokens.contains(&Token::Keyword("LIMIT".to_string())));
    }
    
    #[test]
    fn test_parser_performance_tracking() {
        let mut parser = GpuSqlParser::new().unwrap();
        
        // Parse multiple queries to build statistics
        let queries = vec![
            "SELECT * FROM table1",
            "SELECT id FROM table2 WHERE status = 'active'",
            "SELECT * FROM table3 LIMIT 100",
            "SELECT id, VECTOR_SIMILARITY(vec, [0.5, 0.5], 'cosine') FROM table4",
            "SELECT COUNT(*) FROM table5 WHERE created_at > '2024-01-01'",
        ];
        
        for sql in &queries {
            let _ = parser.parse(sql);
        }
        
        let stats = parser.stats();
        assert_eq!(stats.total_queries_parsed, queries.len() as u64);
        
        // Either all GPU or all CPU (depending on availability)
        assert!(
            stats.gpu_accelerated_count == queries.len() as u64 ||
            stats.cpu_fallback_count == queries.len() as u64
        );
        
        debug!("\n=== Parser Performance Stats ===");
        debug!("Total queries parsed: {}", stats.total_queries_parsed);
        debug!("GPU accelerated: {}", stats.gpu_accelerated_count);
        debug!("CPU fallback: {}", stats.cpu_fallback_count);
        debug!("Total parse time: {:.2}ms", stats.total_parse_time_ms);
        if stats.gpu_accelerated_count > 0 {
            debug!("Avg GPU parse time: {:.2}ms", 
                stats.gpu_parse_time_ms / stats.gpu_accelerated_count as f64);
        }
        if stats.cpu_fallback_count > 0 {
            debug!("Avg CPU parse time: {:.2}ms", 
                stats.cpu_parse_time_ms / stats.cpu_fallback_count as f64);
        }
    }
    
    #[test]
    fn test_concurrent_gpu_parser_access() {
        use std::sync::Arc;
        use std::thread;
use tracing::{debug, error, info};
        
        let parser_mutex = get_global_gpu_parser();
        let handles: Vec<_> = (0..5).map(|i| {
            let parser_mutex_clone = Arc::clone(&parser_mutex);
            thread::spawn(move || {
                let mut parser = parser_mutex_clone.lock().unwrap();
                let sql = format!("SELECT * FROM table{} WHERE id = {}", i, i * 100);
                let result = parser.parse(&sql);
                assert!(result.is_ok());
            })
        }).collect();
        
        for handle in handles {
            handle.join().unwrap();
        }
        
        // Check final stats
        let parser = parser_mutex.lock().unwrap();
        let stats = parser.stats();
        assert_eq!(stats.total_queries_parsed, 5);
    }
    
    #[test]
    fn test_global_parse_sql_gpu_function() {
        // Test the convenience function
        let sql = "SELECT id, metadata FROM vectors WHERE category = 'test'";
        let result = parse_sql_gpu(sql);
        
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.query_type, QueryType::Select);
    }
    
    #[test]
    #[ignore] // Run with --ignored to benchmark
    fn benchmark_gpu_vs_cpu_parsing() {
        let mut gpu_parser = GpuSqlParser::new().unwrap();
        let cpu_parser = super::super::parser::SqlParser::new();
        
        let queries: Vec<String> = (0..1000).map(|i| {
            format!(
                "SELECT id, name, VECTOR_SIMILARITY(embedding, [{}, {}, {}], 'cosine') as score \
                 FROM collection_{} \
                 WHERE metadata->>'category' = 'cat_{}' \
                 AND metadata->>'price' > {} \
                 ORDER BY score DESC \
                 LIMIT {}",
                i as f32 * 0.001, 
                i as f32 * 0.002, 
                i as f32 * 0.003,
                i % 10,
                i % 5,
                i % 1000,
                (i % 50) + 10
            )
        }).collect();
        
        // Benchmark GPU parsing
        let gpu_start = Instant::now();
        for sql in &queries {
            let _ = gpu_parser.parse(sql);
        }
        let gpu_elapsed = gpu_start.elapsed();
        
        // Benchmark CPU parsing
        let cpu_start = Instant::now();
        for sql in &queries {
            let _ = cpu_parser.parse(sql);
        }
        let cpu_elapsed = cpu_start.elapsed();
        
        debug!("\n=== Parsing Benchmark Results ===");
        debug!("Queries parsed: {}", queries.len());
        debug!("GPU total time: {:.2}ms", gpu_elapsed.as_secs_f64() * 1000.0);
        debug!("CPU total time: {:.2}ms", cpu_elapsed.as_secs_f64() * 1000.0);
        
        let gpu_stats = gpu_parser.stats();
        if gpu_stats.gpu_accelerated_count > 0 {
            let speedup = cpu_elapsed.as_secs_f64() / gpu_elapsed.as_secs_f64();
            debug!("GPU speedup: {:.2}x", speedup);
        } else {
            debug!("GPU acceleration not available - both used CPU");
        }
    }
}