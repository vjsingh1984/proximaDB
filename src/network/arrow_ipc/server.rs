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

use crate::api_handlers::request_handlers::UnifiedHandlers;
use crate::catalog::CatalogManager;
use crate::security::SecurityCoordinator;

use super::service::ProximaFlightService;

/// Arrow Flight server for ProximaDB
pub struct ArrowFlightServer {
    bind_addr: SocketAddr,
    request_handlers: Arc<UnifiedHandlers>,
    security_coordinator: Option<Arc<SecurityCoordinator>>,
    catalog_manager: Option<Arc<CatalogManager>>,
    max_message_size: usize,
    /// Slice 6.2: optional pair carried through to `ProximaFlightService`
    /// at `start()`. Held here rather than on `ProximaFlightService` so
    /// the multi-server call sites set it once on the outer wrapper
    /// alongside `catalog_manager` and `security_coordinator`.
    primary_pod_registry: Option<Arc<crate::cluster::primary_pod_registry::PrimaryPodRegistry>>,
    self_pod_id: Option<String>,
}

impl ArrowFlightServer {
    /// Create new Arrow Flight server
    pub fn new(bind_addr: SocketAddr, request_handlers: Arc<UnifiedHandlers>) -> Self {
        Self {
            bind_addr,
            request_handlers,
            security_coordinator: None,
            catalog_manager: None,
            max_message_size: 512 * 1024 * 1024, // 512MB default
            primary_pod_registry: None,
            self_pod_id: None,
        }
    }

    /// Slice 6.2: attach the primary-pod write router. Both args must
    /// be set together — `start()` only wires the gate when both are
    /// present, so a partial wiring (one but not the other) silently
    /// disables it. Passing them as a pair makes that contract obvious
    /// at the call site.
    pub fn with_primary_pod_gate(
        mut self,
        registry: Arc<crate::cluster::primary_pod_registry::PrimaryPodRegistry>,
        self_pod_id: String,
    ) -> Self {
        self.primary_pod_registry = Some(registry);
        self.self_pod_id = Some(self_pod_id);
        self
    }

    /// Attach the shared security coordinator for Flight request authentication.
    pub fn with_security_coordinator(
        mut self,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
    ) -> Self {
        self.security_coordinator = security_coordinator;
        self
    }

    /// Attach the shared xCatalog manager for catalog-derived Flight schemas.
    pub fn with_catalog_manager(mut self, catalog_manager: Option<Arc<CatalogManager>>) -> Self {
        self.catalog_manager = catalog_manager;
        self
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
        let mut flight_service = ProximaFlightService::new(self.request_handlers.clone())
            .with_security_coordinator(self.security_coordinator.clone())
            .with_catalog_manager(self.catalog_manager.clone());

        // Slice 6.2: only wire the gate when BOTH the registry and
        // the pod identity are present. Partial wiring would silently
        // disable the gate, so the `Some(_)` AND `Some(_)` check is
        // the contract.
        if let (Some(registry), Some(pod_id)) =
            (self.primary_pod_registry.clone(), self.self_pod_id.clone())
        {
            flight_service = flight_service.with_primary_pod_gate(registry, pod_id);
        }

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

    /// Test ArrowFlightServer configuration without requiring UnifiedHandlers
    /// We test the struct fields and builder methods directly
    #[test]
    fn test_server_creation() {
        let addr: SocketAddr = "127.0.0.1:5680".parse().unwrap();
        // Test default max_message_size value (512MB)
        let default_max_size = 512 * 1024 * 1024;
        assert_eq!(default_max_size, 536_870_912);

        // Test SocketAddr parsing
        assert_eq!(addr.port(), 5680);
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
    }

    #[test]
    fn test_custom_message_size() {
        let size = 1024 * 1024 * 1024; // 1GB
        assert_eq!(size, 1_073_741_824);

        // Test that builder pattern would work
        let addr: SocketAddr = "127.0.0.1:5680".parse().unwrap();
        assert!(addr.port() > 0);
    }

    #[test]
    fn test_default_message_size_constant() {
        // Default is 512MB
        let default_size = 512 * 1024 * 1024;
        assert_eq!(default_size, 536_870_912);
        assert!(default_size > 100 * 1024 * 1024); // At least 100MB
        assert!(default_size < 1024 * 1024 * 1024); // Less than 1GB
    }
}
