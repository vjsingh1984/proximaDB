//! # Arrow Flight Service
//!
//! Placeholder implementation of the Arrow Flight service for ProximaDB.
//!
//! ## Migration Status
//!
//! **TEMPORARY PLACEHOLDER**: This module establishes the service boundary in the API
//! crate. The full implementation exists in `src/network/arrow_ipc/service.rs`.

use std::pin::Pin;
use std::sync::Arc;

use arrow_flight::{
    Action, ActionType, Criteria, Empty, FlightData, FlightDescriptor, FlightInfo,
    HandshakeRequest, HandshakeResponse, PutResult, SchemaResult, Ticket,
    flight_service_server::{FlightService, FlightServiceServer},
};
use futures::Stream;
use tonic::{Request, Response, Status, Streaming};

use proximadb_runtime::UnifiedHandlers;

type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

/// ProximaDB Arrow Flight service implementation
///
/// Thin wrapper around UnifiedHandlers that converts Arrow Flight messages
/// to/from ProximaDB record types for high-throughput bulk ingestion.
pub struct ProximaFlightService {
    _handlers: Arc<UnifiedHandlers>,
}

impl ProximaFlightService {
    /// Create a new Flight service backed by unified handlers
    pub fn new(handlers: Arc<UnifiedHandlers>) -> Self {
        Self {
            _handlers: handlers,
        }
    }

    /// Convert into a tonic service server
    pub fn into_server(self) -> FlightServiceServer<Self> {
        FlightServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl FlightService for ProximaFlightService {
    type HandshakeStream = BoxStream<HandshakeResponse>;
    type ListFlightsStream = BoxStream<FlightInfo>;
    type DoGetStream = BoxStream<FlightData>;
    type DoPutStream = BoxStream<PutResult>;
    type DoActionStream = BoxStream<arrow_flight::Result>;
    type ListActionsStream = BoxStream<ActionType>;
    type DoExchangeStream = BoxStream<FlightData>;

    async fn handshake(
        &self,
        _request: Request<Streaming<HandshakeRequest>>,
    ) -> Result<Response<Self::HandshakeStream>, Status> {
        Err(Status::unimplemented("Arrow Flight migration in progress"))
    }

    async fn list_flights(
        &self,
        _request: Request<Criteria>,
    ) -> Result<Response<Self::ListFlightsStream>, Status> {
        Err(Status::unimplemented("Arrow Flight migration in progress"))
    }

    async fn get_flight_info(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Arrow Flight migration in progress"))
    }

    async fn poll_flight_info(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<arrow_flight::PollInfo>, Status> {
        Err(Status::unimplemented("Arrow Flight migration in progress"))
    }

    async fn get_schema(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<SchemaResult>, Status> {
        Err(Status::unimplemented("Arrow Flight migration in progress"))
    }

    async fn do_get(
        &self,
        _request: Request<Ticket>,
    ) -> Result<Response<Self::DoGetStream>, Status> {
        Err(Status::unimplemented("Arrow Flight migration in progress"))
    }

    async fn do_put(
        &self,
        _request: Request<Streaming<FlightData>>,
    ) -> Result<Response<Self::DoPutStream>, Status> {
        Err(Status::unimplemented("Arrow Flight migration in progress"))
    }

    async fn do_exchange(
        &self,
        _request: Request<Streaming<FlightData>>,
    ) -> Result<Response<Self::DoExchangeStream>, Status> {
        Err(Status::unimplemented("Arrow Flight migration in progress"))
    }

    async fn do_action(
        &self,
        _request: Request<Action>,
    ) -> Result<Response<Self::DoActionStream>, Status> {
        Err(Status::unimplemented("Arrow Flight migration in progress"))
    }

    async fn list_actions(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<Self::ListActionsStream>, Status> {
        Err(Status::unimplemented("Arrow Flight migration in progress"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::noop_unified_handlers;
    use tonic::Code;

    fn assert_unimplemented<T>(result: Result<Response<T>, Status>) {
        let err = match result {
            Ok(_) => panic!("Arrow Flight placeholder should reject RPC"),
            Err(err) => err,
        };
        assert_eq!(err.code(), Code::Unimplemented);
        assert!(err.message().contains("Arrow Flight migration in progress"));
    }

    #[tokio::test]
    async fn placeholder_flight_service_rejects_unary_and_output_streaming_rpcs() {
        let service = ProximaFlightService::new(noop_unified_handlers());
        let _server = ProximaFlightService::new(noop_unified_handlers()).into_server();

        assert_unimplemented(
            service
                .list_flights(Request::new(Criteria::default()))
                .await,
        );
        assert_unimplemented(
            service
                .get_flight_info(Request::new(FlightDescriptor::default()))
                .await,
        );
        assert_unimplemented(
            service
                .poll_flight_info(Request::new(FlightDescriptor::default()))
                .await,
        );
        assert_unimplemented(
            service
                .get_schema(Request::new(FlightDescriptor::default()))
                .await,
        );
        assert_unimplemented(service.do_get(Request::new(Ticket::default())).await);
        assert_unimplemented(service.do_action(Request::new(Action::default())).await);
        assert_unimplemented(service.list_actions(Request::new(Empty {})).await);
    }
}
