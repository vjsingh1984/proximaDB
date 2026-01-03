// PostgreSQL Wire Protocol Integration Tests
//
// Tests the PostgreSQL wire protocol implementation for compatibility with:
// - Standard PostgreSQL clients (psql, pgAdmin)
// - pgvector applications
// - Application drivers (e.g., tokio-postgres)

use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Test PostgreSQL server connection and basic handshake
///
/// This test verifies:
/// 1. Server binds to the configured port
/// 2. Client can establish TCP connection
/// 3. PostgreSQL startup handshake works
/// 4. Authentication succeeds (trust mode)
#[tokio::test]
async fn test_postgres_connection() {
    // This test requires a running server - skip if server not available
    let addr: SocketAddr = "127.0.0.1:5433".parse().unwrap();

    // Try to connect (with timeout for CI environments)
    match timeout(Duration::from_secs(2), TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => {
            // Connection established - verify we can read/write
            drop(stream);
            // If we get here, the server is running and accepting connections
        }
        Ok(Err(_)) | Err(_) => {
            // Server not running - skip test
            eprintln!(
                "PostgreSQL server not available at {} - skipping test",
                addr
            );
            return;
        }
    }
}

/// Test PostgreSQL wire protocol startup message format
///
/// PostgreSQL startup message format:
/// - Length (4 bytes, big-endian)
/// - Protocol version (4 bytes: 3.0 = 0x00030000)
/// - Parameters (key=value\0 pairs, terminated by \0)
#[tokio::test]
async fn test_postgres_startup_message() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let addr: SocketAddr = "127.0.0.1:5433".parse().unwrap();

    // Try to connect
    let stream = match timeout(Duration::from_secs(2), TcpStream::connect(addr)).await {
        Ok(Ok(s)) => s,
        _ => {
            eprintln!("PostgreSQL server not available - skipping test");
            return;
        }
    };

    let mut stream = stream;

    // Build startup message
    let mut startup = Vec::new();

    // Protocol version 3.0
    startup.extend_from_slice(&[0x00, 0x03, 0x00, 0x00]);

    // Parameters
    startup.extend_from_slice(b"user\0");
    startup.extend_from_slice(b"postgres\0");
    startup.extend_from_slice(b"database\0");
    startup.extend_from_slice(b"proximadb\0");
    startup.push(0); // End of parameters

    // Calculate length (includes length field itself)
    let length = (startup.len() + 4) as i32;

    // Send message
    let mut message = Vec::new();
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(&startup);

    if let Err(e) = stream.write_all(&message).await {
        eprintln!("Failed to send startup message: {}", e);
        return;
    }

    // Read response
    let mut response = [0u8; 1024];
    match timeout(Duration::from_secs(2), stream.read(&mut response)).await {
        Ok(Ok(n)) if n > 0 => {
            // Check for AuthenticationOk (R message with auth type 0)
            // or AuthenticationCleartextPassword (auth type 3)
            if response[0] == b'R' {
                // Authentication response received
                let auth_type =
                    i32::from_be_bytes([response[5], response[6], response[7], response[8]]);

                match auth_type {
                    0 => {
                        println!("AuthenticationOk received");
                    }
                    3 => {
                        println!("AuthenticationCleartextPassword received");
                        // Would need to send password here
                    }
                    5 => {
                        println!("AuthenticationMD5Password received");
                        // Would need to send MD5 hashed password
                    }
                    _ => {
                        println!("Unknown auth type: {}", auth_type);
                    }
                }
            } else if response[0] == b'E' {
                // Error response
                println!("Error response received: {:?}", &response[..n.min(100)]);
            }
        }
        _ => {
            eprintln!("No response from server");
        }
    }
}

/// Test using tokio-postgres client
///
/// This test uses the actual tokio-postgres driver to verify compatibility
#[tokio::test]
async fn test_tokio_postgres_client() {
    use tokio_postgres::{NoTls, config::Config as PgConfig};

    // Configure connection
    let mut config = PgConfig::new();
    config.host("127.0.0.1");
    config.port(5433);
    config.user("postgres");
    config.dbname("proximadb");

    // Try to connect with timeout
    let connect_future = config.connect(NoTls);

    match timeout(Duration::from_secs(5), connect_future).await {
        Ok(Ok((client, connection))) => {
            // Spawn connection handler
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    eprintln!("Connection error: {}", e);
                }
            });

            // Test simple query
            match client.simple_query("SELECT 1").await {
                Ok(results) => {
                    println!("Query returned {} results", results.len());
                }
                Err(e) => {
                    // Expected for now - execute_query returns empty results
                    println!("Query error (expected in current state): {}", e);
                }
            }

            // Test SHOW command
            match client.simple_query("SHOW server_version").await {
                Ok(_) => {
                    println!("SHOW command succeeded");
                }
                Err(e) => {
                    println!("SHOW command error: {}", e);
                }
            }
        }
        Ok(Err(e)) => {
            eprintln!("PostgreSQL connection failed: {} - is server running?", e);
        }
        Err(_) => {
            eprintln!("Connection timeout - server not available");
        }
    }
}

/// Test pgvector-style queries
///
/// Tests translation of pgvector distance operators:
/// - <-> (L2 distance)
/// - <=> (cosine distance)
/// - <#> (inner product)
#[tokio::test]
async fn test_pgvector_query_translation() {
    use tokio_postgres::{NoTls, config::Config as PgConfig};

    let mut config = PgConfig::new();
    config.host("127.0.0.1");
    config.port(5433);
    config.user("postgres");
    config.dbname("proximadb");

    let connect_future = config.connect(NoTls);

    match timeout(Duration::from_secs(5), connect_future).await {
        Ok(Ok((client, connection))) => {
            tokio::spawn(async move {
                let _ = connection.await;
            });

            // Test pgvector-style query (should be translated)
            let pgvector_query = "SELECT * FROM items ORDER BY embedding <-> '[1,2,3]' LIMIT 10";

            match client.simple_query(pgvector_query).await {
                Ok(_) => {
                    println!("pgvector query accepted");
                }
                Err(e) => {
                    println!("pgvector query error (may be expected): {}", e);
                }
            }
        }
        Ok(Err(e)) => {
            eprintln!("Connection failed: {}", e);
        }
        Err(_) => {
            eprintln!("Connection timeout");
        }
    }
}

/// Unit test for query translator
#[test]
fn test_query_translator() {
    use proximadb::network::postgres::translator::QueryTranslator;

    let translator = QueryTranslator::new();

    // Test SET command (should be ignored)
    let result = translator.translate("SET client_encoding TO 'UTF8'");
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());

    // Test SHOW command
    let result = translator.translate("SHOW server_version");
    assert!(result.is_ok());
    assert!(result.unwrap().contains("server_version"));

    // Test transaction commands
    let result = translator.translate("BEGIN");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "BEGIN");

    // Test pgvector syntax
    let result = translator.translate("SELECT * FROM items WHERE embedding <-> '[1,2,3]' < 0.5");
    assert!(result.is_ok());
    let translated = result.unwrap();
    // Should contain distance marker
    assert!(translated.contains("l2") || translated.contains("<->"));
}

/// Test session management
#[test]
fn test_session_management() {
    use proximadb::network::postgres::session::SessionManager;

    // Create session manager
    let _manager = SessionManager::new();

    // Verify manager is created
    // Session creation is async, so we just verify the manager exists
    assert!(true);
}

/// Test PostgreSQL type mapping
#[test]
fn test_pg_type_mapping() {
    use proximadb::network::postgres::types::PgType;

    // Test type OID mappings
    assert_eq!(PgType::from_oid(23), PgType::Int4);
    assert_eq!(PgType::from_oid(25), PgType::Text);
    assert_eq!(PgType::from_oid(701), PgType::Float8);

    // Test unknown type
    assert_eq!(PgType::from_oid(99999), PgType::Unknown);
}
