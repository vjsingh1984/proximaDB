// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Arrow Flight server implementation
//!
//! Wraps ProximaFlightService in a Tonic gRPC server on port 5680.

use anyhow::Result;
use arrow_flight::flight_service_server::FlightServiceServer;
use std::net::SocketAddr;
use std::sync::Arc;
use tonic::transport::Server;
use tracing::info;

use crate::api_handlers::unified_handlers::UnifiedHandlers;

use super::service::ProximaFlightService;

/// Arrow Flight server for ProximaDB
pub struct ArrowFlightServer {
    bind_addr: SocketAddr,
    unified_handlers: Arc<UnifiedHandlers>,
    max_message_size: usize,
}

impl ArrowFlightServer {
    /// Create new Arrow Flight server
    pub fn new(bind_addr: SocketAddr, unified_handlers: Arc<UnifiedHandlers>) -> Self {
        Self {
            bind_addr,
            unified_handlers,
            max_message_size: 512 * 1024 * 1024, // 512MB default
        }
    }

    /// Set maximum message size
    pub fn with_max_message_size(mut self, size: usize) -> Self {
        self.max_message_size = size;
        self
    }

    /// Start the Arrow Flight server
    ///
    /// This starts a Tonic gRPC server with the FlightService implementation.
    /// The server runs until an error occurs or the task is cancelled.
    pub async fn start(self) -> Result<()> {
        info!(
            bind_addr = %self.bind_addr,
            max_message_size = self.max_message_size,
            "Starting Arrow Flight server"
        );

        // Create Flight service
        let flight_service = ProximaFlightService::new(self.unified_handlers.clone());

        // Wrap in FlightServiceServer with size limits
        let flight_server = FlightServiceServer::new(flight_service)
            .max_encoding_message_size(self.max_message_size)
            .max_decoding_message_size(self.max_message_size);

        // Start Tonic server
        Server::builder()
            .add_service(flight_server)
            .serve(self.bind_addr)
            .await?;

        info!("Arrow Flight server stopped");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_creation() {
        let addr = "127.0.0.1:5680".parse().unwrap();
        let server = ArrowFlightServer::new(addr, Arc::new(todo!()));
        assert_eq!(server.bind_addr, addr);
        assert_eq!(server.max_message_size, 512 * 1024 * 1024);
    }

    #[test]
    fn test_custom_message_size() {
        let addr = "127.0.0.1:5680".parse().unwrap();
        let size = 1024 * 1024 * 1024; // 1GB
        let server = ArrowFlightServer::new(addr, Arc::new(todo!())).with_max_message_size(size);
        assert_eq!(server.max_message_size, size);
    }
}
